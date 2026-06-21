use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use crate::RomInfo;
use crate::disasm::{DecodedInstruction, DisasmResult, get_abs_addr, FlowControl};
use anyhow::Result;

pub fn generate_c_project(rom: &RomInfo, disasm: &DisasmResult, output_dir: &Path) -> Result<()> {
    // Group instructions by their closest preceding function entry point
    let mut func_groups: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut current_func = 0x0100; // Default fallback function

    for &abs_addr in disasm.instructions.keys() {
        if disasm.function_entries.contains(&abs_addr) {
            current_func = abs_addr;
        }
        func_groups.entry(current_func).or_default().push(abs_addr);
    }

    // 1. Generate game.h
    let game_h_path = output_dir.join("game.h");
    let mut game_h = File::create(&game_h_path)?;
    write_game_h(&mut game_h, rom, &func_groups)?;

    // 2. Generate game.c (the translated functions)
    let game_c_path = output_dir.join("game.c");
    let mut game_c = File::create(&game_c_path)?;
    write_game_c(&mut game_c, &func_groups, disasm)?;

    // 3. Generate dispatcher.c (the jump table)
    let disp_c_path = output_dir.join("dispatcher.c");
    let mut disp_c = File::create(&disp_c_path)?;
    write_dispatcher_c(&mut disp_c, &func_groups)?;

    // 4. Generate ROM binary header/data arrays so we can link it
    let rom_data_c_path = output_dir.join("rom_data.c");
    let mut rom_data_c = File::create(&rom_data_c_path)?;
    write_rom_data_c(&mut rom_data_c, rom)?;

    Ok(())
}

fn write_game_h(w: &mut File, rom: &RomInfo, func_groups: &BTreeMap<u32, Vec<u32>>) -> Result<()> {
    writeln!(w, "#ifndef GAME_H")?;
    writeln!(w, "#define GAME_H")?;
    writeln!(w, "#include \"runtime.h\"")?;
    writeln!(w, "")?;
    writeln!(w, "// Game Name: {}", rom.title)?;
    writeln!(w, "// Mode: {}", if rom.is_gbc { "GBC" } else { "DMG" })?;
    writeln!(w, "")?;

    // Declare all recompiled functions
    writeln!(w, "// --- Subroutine Declarations ---")?;
    for &func_abs in func_groups.keys() {
        let bank = (func_abs / 16384) as u16;
        let pc = (if func_abs < 0x4000 { func_abs } else { (func_abs % 16384) + 0x4000 }) as u16;
        writeln!(w, "void fn_{:02X}_{:04X}(CPUState* cpu);", bank, pc)?;
    }
    writeln!(w, "")?;

    writeln!(w, "#endif // GAME_H")?;
    Ok(())
}

fn write_game_c(w: &mut File, func_groups: &BTreeMap<u32, Vec<u32>>, disasm: &DisasmResult) -> Result<()> {
    writeln!(w, "#include \"game.h\"")?;
    writeln!(w, "#include \"runtime.h\"")?;
    writeln!(w, "")?;

    for (&func_abs, instrs) in func_groups {
        let bank = (func_abs / 16384) as u16;
        let pc = (if func_abs < 0x4000 { func_abs } else { (func_abs % 16384) + 0x4000 }) as u16;

        writeln!(w, "void fn_{:02X}_{:04X}(CPUState* cpu) {{", bank, pc)?;

        let mut next_expected_pc = pc;

        for &abs_addr in instrs {
            let inst = &disasm.instructions[&abs_addr];
            
            // Check if we need to emit a label for local jumps
            if disasm.jump_targets.contains(&abs_addr) || inst.pc != next_expected_pc {
                writeln!(w, "loc_{:02X}_{:04X}:", bank, inst.pc)?;
            }

            // Sync with other hardware component cycles and interrupts periodically
            // We do it before instructions that could change control flow or write memory
            let needs_sync = inst.flow != FlowControl::Normal || inst.bytes[0] == 0xEA || inst.bytes[0] == 0xE0 || inst.bytes[0] == 0xE2;
            if needs_sync {
                writeln!(w, "    cpu->pc = 0x{:04X};", inst.pc)?;
                writeln!(w, "    sync_hardware(cpu);")?;
                writeln!(w, "    if (cpu->pc != 0x{:04X}) return;", inst.pc)?;
            }

            // Write C equivalent code
            let c_code = translate_to_c(inst, bank, instrs);
            writeln!(w, "    {} // {}", c_code, inst.mnemonic)?;

            next_expected_pc = inst.pc + inst.bytes.len() as u16;
        }

        writeln!(w, "    cpu->pc = 0x{:04X};", next_expected_pc)?;
        writeln!(w, "}}")?;
        writeln!(w, "")?;
    }

    Ok(())
}

fn write_dispatcher_c(w: &mut File, func_groups: &BTreeMap<u32, Vec<u32>>) -> Result<()> {
    writeln!(w, "#include \"game.h\"")?;
    writeln!(w, "#include \"runtime.h\"")?;
    writeln!(w, "")?;
    writeln!(w, "void dispatch(CPUState* cpu, uint16_t bank, uint16_t pc) {{")?;
    writeln!(w, "    uint32_t abs_addr = get_abs_addr(bank, pc);")?;
    writeln!(w, "    switch (abs_addr) {{")?;

    for &func_abs in func_groups.keys() {
        let bank = (func_abs / 16384) as u16;
        let pc = (if func_abs < 0x4000 { func_abs } else { (func_abs % 16384) + 0x4000 }) as u16;
        writeln!(w, "        case 0x{:08X}: fn_{:02X}_{:04X}(cpu); break;", func_abs, bank, pc)?;
    }

    writeln!(w, "        default:")?;
    writeln!(w, "            break;")?;
    writeln!(w, "    }}")?;
    writeln!(w, "}}")?;
    Ok(())
}

fn write_rom_data_c(w: &mut File, rom: &RomInfo) -> Result<()> {
    writeln!(w, "#include <stdint.h>")?;
    writeln!(w, "#include <stddef.h>")?;
    writeln!(w, "")?;
    writeln!(w, "const uint8_t g_rom_data[] = {{")?;

    for (i, &b) in rom.rom_data.iter().enumerate() {
        if i % 16 == 0 {
            write!(w, "    ")?;
        }
        write!(w, "0x{:02X}, ", b)?;
        if i % 16 == 15 {
            writeln!(w, "")?;
        }
    }
    writeln!(w, "\n}};")?;
    writeln!(w, "const size_t g_rom_size = {};", rom.rom_data.len())?;
    writeln!(w, "const char* g_rom_title = \"{}\";", rom.title)?;
    writeln!(w, "const uint8_t g_rom_is_gbc = {};", if rom.is_gbc { 1 } else { 0 })?;
    writeln!(w, "const uint8_t g_rom_cartridge_type = {};", rom.cartridge_type)?;
    writeln!(w, "const size_t g_rom_ram_size = {};", rom.ram_size)?;

    Ok(())
}

fn translate_to_c(inst: &DecodedInstruction, current_bank: u16, instrs: &[u32]) -> String {
    let is_local = |target_pc: u16| -> bool {
        instrs.contains(&get_abs_addr(current_bank, target_pc))
    };
    let op = inst.bytes[0];
    let next_pc = inst.pc + inst.bytes.len() as u16;

    match op {
        0x00 => format!("cpu->cycles += 4;"),
        0x01 => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            format!("cpu->bc = 0x{:04X}; cpu->cycles += 12;", val)
        }
        0x02 => format!("write8(cpu, cpu->bc, cpu->a); cpu->cycles += 8;"),
        0x03 => format!("cpu->bc++; cpu->cycles += 8;"),
        0x04 => format!("cpu->b = cpu_inc8(cpu, cpu->b); cpu->cycles += 4;"),
        0x05 => format!("cpu->b = cpu_dec8(cpu, cpu->b); cpu->cycles += 4;"),
        0x06 => format!("cpu->b = 0x{:02X}; cpu->cycles += 8;", inst.bytes[1]),
        0x07 => format!("cpu_rlca(cpu); cpu->cycles += 4;"),
        0x08 => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            format!("write16(cpu, 0x{:04X}, cpu->sp); cpu->cycles += 20;", val)
        }
        0x09 => format!("cpu_add_hl(cpu, cpu->bc); cpu->cycles += 8;"),
        0x0A => format!("cpu->a = read8(cpu, cpu->bc); cpu->cycles += 8;"),
        0x0B => format!("cpu->bc--; cpu->cycles += 8;"),
        0x0C => format!("cpu->c = cpu_inc8(cpu, cpu->c); cpu->cycles += 4;"),
        0x0D => format!("cpu->c = cpu_dec8(cpu, cpu->c); cpu->cycles += 4;"),
        0x0E => format!("cpu->c = 0x{:02X}; cpu->cycles += 8;", inst.bytes[1]),
        0x0F => format!("cpu_rrca(cpu); cpu->cycles += 4;"),

        0x10 => format!("cpu->halted = true; cpu->cycles += 4; cpu->pc = 0x{:04X}; return;", next_pc), // STOP
        0x11 => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            format!("cpu->de = 0x{:04X}; cpu->cycles += 12;", val)
        }
        0x12 => format!("write8(cpu, cpu->de, cpu->a); cpu->cycles += 8;"),
        0x13 => format!("cpu->de++; cpu->cycles += 8;"),
        0x14 => format!("cpu->d = cpu_inc8(cpu, cpu->d); cpu->cycles += 4;"),
        0x15 => format!("cpu->d = cpu_dec8(cpu, cpu->d); cpu->cycles += 4;"),
        0x16 => format!("cpu->d = 0x{:02X}; cpu->cycles += 8;", inst.bytes[1]),
        0x17 => format!("cpu_rla(cpu); cpu->cycles += 4;"),
        0x18 => {
            let offset = inst.bytes[1] as i8;
            let target = ((inst.pc as i32) + 2 + (offset as i32)) as u16;
            if is_local(target) {
                format!("cpu->cycles += 12; goto loc_{:02X}_{:04X};", current_bank, target)
            } else {
                format!("cpu->cycles += 12; goto_pc(cpu, 0x{:04X}); return;", target)
            }
        }
        0x19 => format!("cpu_add_hl(cpu, cpu->de); cpu->cycles += 8;"),
        0x1A => format!("cpu->a = read8(cpu, cpu->de); cpu->cycles += 8;"),
        0x1B => format!("cpu->de--; cpu->cycles += 8;"),
        0x1C => format!("cpu->e = cpu_inc8(cpu, cpu->e); cpu->cycles += 4;"),
        0x1D => format!("cpu->e = cpu_dec8(cpu, cpu->e); cpu->cycles += 4;"),
        0x1E => format!("cpu->e = 0x{:02X}; cpu->cycles += 8;", inst.bytes[1]),
        0x1F => format!("cpu_rra(cpu); cpu->cycles += 4;"),

        0x20 => {
            let offset = inst.bytes[1] as i8;
            let target = ((inst.pc as i32) + 2 + (offset as i32)) as u16;
            if is_local(target) {
                format!("if (!(cpu->f & FLAG_Z)) {{ cpu->cycles += 12; goto loc_{:02X}_{:04X}; }} else {{ cpu->cycles += 8; }}", current_bank, target)
            } else {
                format!("if (!(cpu->f & FLAG_Z)) {{ cpu->cycles += 12; goto_pc(cpu, 0x{:04X}); return; }} else {{ cpu->cycles += 8; }}", target)
            }
        }
        0x21 => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            format!("cpu->hl = 0x{:04X}; cpu->cycles += 12;", val)
        }
        0x22 => format!("write8(cpu, cpu->hl, cpu->a); cpu->hl++; cpu->cycles += 8;"),
        0x23 => format!("cpu->hl++; cpu->cycles += 8;"),
        0x24 => format!("cpu->h = cpu_inc8(cpu, cpu->h); cpu->cycles += 4;"),
        0x25 => format!("cpu->h = cpu_dec8(cpu, cpu->h); cpu->cycles += 4;"),
        0x26 => format!("cpu->h = 0x{:02X}; cpu->cycles += 8;", inst.bytes[1]),
        0x27 => format!("cpu_daa(cpu); cpu->cycles += 4;"),
        0x28 => {
            let offset = inst.bytes[1] as i8;
            let target = ((inst.pc as i32) + 2 + (offset as i32)) as u16;
            if is_local(target) {
                format!("if (cpu->f & FLAG_Z) {{ cpu->cycles += 12; goto loc_{:02X}_{:04X}; }} else {{ cpu->cycles += 8; }}", current_bank, target)
            } else {
                format!("if (cpu->f & FLAG_Z) {{ cpu->cycles += 12; goto_pc(cpu, 0x{:04X}); return; }} else {{ cpu->cycles += 8; }}", target)
            }
        }
        0x29 => format!("cpu_add_hl(cpu, cpu->hl); cpu->cycles += 8;"),
        0x2A => format!("cpu->a = read8(cpu, cpu->hl); cpu->hl++; cpu->cycles += 8;"),
        0x2B => format!("cpu->hl--; cpu->cycles += 8;"),
        0x2C => format!("cpu->l = cpu_inc8(cpu, cpu->l); cpu->cycles += 4;"),
        0x2D => format!("cpu->l = cpu_dec8(cpu, cpu->l); cpu->cycles += 4;"),
        0x2E => format!("cpu->l = 0x{:02X}; cpu->cycles += 8;", inst.bytes[1]),
        0x2F => format!("cpu->a = ~cpu->a; cpu->f |= FLAG_N | FLAG_H; cpu->cycles += 4;"),

        0x30 => {
            let offset = inst.bytes[1] as i8;
            let target = ((inst.pc as i32) + 2 + (offset as i32)) as u16;
            if is_local(target) {
                format!("if (!(cpu->f & FLAG_C)) {{ cpu->cycles += 12; goto loc_{:02X}_{:04X}; }} else {{ cpu->cycles += 8; }}", current_bank, target)
            } else {
                format!("if (!(cpu->f & FLAG_C)) {{ cpu->cycles += 12; goto_pc(cpu, 0x{:04X}); return; }} else {{ cpu->cycles += 8; }}", target)
            }
        }
        0x31 => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            format!("cpu->sp = 0x{:04X}; cpu->cycles += 12;", val)
        }
        0x32 => format!("write8(cpu, cpu->hl, cpu->a); cpu->hl--; cpu->cycles += 8;"),
        0x33 => format!("cpu->sp++; cpu->cycles += 8;"),
        0x34 => format!("write8(cpu, cpu->hl, cpu_inc8(cpu, read8(cpu, cpu->hl))); cpu->cycles += 12;"),
        0x35 => format!("write8(cpu, cpu->hl, cpu_dec8(cpu, read8(cpu, cpu->hl))); cpu->cycles += 12;"),
        0x36 => format!("write8(cpu, cpu->hl, 0x{:02X}); cpu->cycles += 12;", inst.bytes[1]),
        0x37 => format!("cpu->f &= ~(FLAG_N | FLAG_H); cpu->f |= FLAG_C; cpu->cycles += 4;"), // SCF
        0x38 => {
            let offset = inst.bytes[1] as i8;
            let target = ((inst.pc as i32) + 2 + (offset as i32)) as u16;
            if is_local(target) {
                format!("if (cpu->f & FLAG_C) {{ cpu->cycles += 12; goto loc_{:02X}_{:04X}; }} else {{ cpu->cycles += 8; }}", current_bank, target)
            } else {
                format!("if (cpu->f & FLAG_C) {{ cpu->cycles += 12; goto_pc(cpu, 0x{:04X}); return; }} else {{ cpu->cycles += 8; }}", target)
            }
        }
        0x39 => format!("cpu_add_hl(cpu, cpu->sp); cpu->cycles += 8;"),
        0x3A => format!("cpu->a = read8(cpu, cpu->hl); cpu->hl--; cpu->cycles += 8;"),
        0x3B => format!("cpu->sp--; cpu->cycles += 8;"),
        0x3C => format!("cpu->a = cpu_inc8(cpu, cpu->a); cpu->cycles += 4;"),
        0x3D => format!("cpu->a = cpu_dec8(cpu, cpu->a); cpu->cycles += 4;"),
        0x3E => format!("cpu->a = 0x{:02X}; cpu->cycles += 8;", inst.bytes[1]),
        0x3F => format!("cpu->f &= ~FLAG_N; cpu->f ^= FLAG_C; cpu->f &= ~FLAG_H; cpu->cycles += 4;"), // CCF

        0x76 => format!("cpu->halted = true; cpu->cycles += 4; cpu->pc = 0x{:04X}; return;", next_pc), // HALT

        // LD instructions 0x40 - 0x7F
        0x40..=0x7F => {
            let regs = ["b", "c", "d", "e", "h", "l", "hl", "a"];
            let dest_idx = ((op - 0x40) >> 3) as usize;
            let src_idx = ((op - 0x40) & 0x07) as usize;
            
            let dest = regs[dest_idx];
            let src = regs[src_idx];

            if dest == "hl" {
                format!("write8(cpu, cpu->hl, cpu->{}); cpu->cycles += 8;", src)
            } else if src == "hl" {
                format!("cpu->{} = read8(cpu, cpu->hl); cpu->cycles += 8;", dest)
            } else {
                format!("cpu->{} = cpu->{}; cpu->cycles += 4;", dest, src)
            }
        }

        // ADD, ADC, SUB, SBC, AND, XOR, OR, CP 0x80 - 0xBF
        0x80..=0xBF => {
            let regs = ["b", "c", "d", "e", "h", "l", "hl", "a"];
            let src = regs[(op & 0x07) as usize];
            let alu_op = (op - 0x80) >> 3;

            let read_val = if src == "hl" { "read8(cpu, cpu->hl)" } else { &format!("cpu->{}", src) };
            let val_cycles = if src == "hl" { 8 } else { 4 };

            match alu_op {
                0 => format!("cpu_add_a(cpu, {}); cpu->cycles += {};", read_val, val_cycles),
                1 => format!("cpu_adc_a(cpu, {}); cpu->cycles += {};", read_val, val_cycles),
                2 => format!("cpu_sub_a(cpu, {}); cpu->cycles += {};", read_val, val_cycles),
                3 => format!("cpu_sbc_a(cpu, {}); cpu->cycles += {};", read_val, val_cycles),
                4 => format!("cpu_and_a(cpu, {}); cpu->cycles += {};", read_val, val_cycles),
                5 => format!("cpu_xor_a(cpu, {}); cpu->cycles += {};", read_val, val_cycles),
                6 => format!("cpu_or_a(cpu, {}); cpu->cycles += {};", read_val, val_cycles),
                7 => format!("cpu_cp_a(cpu, {}); cpu->cycles += {};", read_val, val_cycles),
                _ => unreachable!(),
            }
        }

        0xC0 => format!("if (!(cpu->f & FLAG_Z)) {{ cpu->cycles += 20; cpu_ret(cpu); return; }} else {{ cpu->cycles += 8; }}"),
        0xC1 => format!("cpu->bc = cpu_pop16(cpu); cpu->cycles += 12;"),
        0xC2 => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            if is_local(val) {
                format!("if (!(cpu->f & FLAG_Z)) {{ cpu->cycles += 12; goto loc_{:02X}_{:04X}; }} else {{ cpu->cycles += 12; }}", current_bank, val)
            } else {
                format!("if (!(cpu->f & FLAG_Z)) {{ cpu->cycles += 12; goto_pc(cpu, 0x{:04X}); return; }} else {{ cpu->cycles += 12; }}", val)
            }
        }
        0xC3 => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            if is_local(val) {
                format!("cpu->cycles += 16; goto loc_{:02X}_{:04X};", current_bank, val)
            } else {
                format!("cpu->cycles += 16; goto_pc(cpu, 0x{:04X}); return;", val)
            }
        }
        0xC4 => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            format!("if (!(cpu->f & FLAG_Z)) {{ cpu->cycles += 24; cpu_call(cpu, 0x{:04X}, 0x{:04X}); return; }} else {{ cpu->cycles += 12; }}", val, next_pc)
        }
        0xC5 => format!("cpu_push16(cpu, cpu->bc); cpu->cycles += 16;"),
        0xC6 => format!("cpu_add_a(cpu, 0x{:02X}); cpu->cycles += 8;", inst.bytes[1]),
        0xC7 => format!("cpu->cycles += 16; cpu_call(cpu, 0x0000, 0x{:04X}); return;", next_pc),
        0xC8 => format!("if (cpu->f & FLAG_Z) {{ cpu->cycles += 20; cpu_ret(cpu); return; }} else {{ cpu->cycles += 8; }}"),
        0xC9 => format!("cpu->cycles += 16; cpu_ret(cpu); return;"),
        0xCA => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            if is_local(val) {
                format!("if (cpu->f & FLAG_Z) {{ cpu->cycles += 12; goto loc_{:02X}_{:04X}; }} else {{ cpu->cycles += 12; }}", current_bank, val)
            } else {
                format!("if (cpu->f & FLAG_Z) {{ cpu->cycles += 12; goto_pc(cpu, 0x{:04X}); return; }} else {{ cpu->cycles += 12; }}", val)
            }
        }
        0xCB => {
            let cb_op = inst.bytes[1];
            translate_cb_to_c(cb_op)
        }
        0xCC => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            format!("if (cpu->f & FLAG_Z) {{ cpu->cycles += 24; cpu_call(cpu, 0x{:04X}, 0x{:04X}); return; }} else {{ cpu->cycles += 12; }}", val, next_pc)
        }
        0xCD => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            format!("cpu->cycles += 24; cpu_call(cpu, 0x{:04X}, 0x{:04X}); return;", val, next_pc)
        }
        0xCE => format!("cpu_adc_a(cpu, 0x{:02X}); cpu->cycles += 8;", inst.bytes[1]),
        0xCF => format!("cpu->cycles += 16; cpu_call(cpu, 0x0008, 0x{:04X}); return;", next_pc),

        0xD0 => format!("if (!(cpu->f & FLAG_C)) {{ cpu->cycles += 20; cpu_ret(cpu); return; }} else {{ cpu->cycles += 8; }}"),
        0xD1 => format!("cpu->de = cpu_pop16(cpu); cpu->cycles += 12;"),
        0xD2 => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            if is_local(val) {
                format!("if (!(cpu->f & FLAG_C)) {{ cpu->cycles += 12; goto loc_{:02X}_{:04X}; }} else {{ cpu->cycles += 12; }}", current_bank, val)
            } else {
                format!("if (!(cpu->f & FLAG_C)) {{ cpu->cycles += 12; goto_pc(cpu, 0x{:04X}); return; }} else {{ cpu->cycles += 12; }}", val)
            }
        }
        0xD4 => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            format!("if (!(cpu->f & FLAG_C)) {{ cpu->cycles += 24; cpu_call(cpu, 0x{:04X}, 0x{:04X}); return; }} else {{ cpu->cycles += 12; }}", val, next_pc)
        }
        0xD5 => format!("cpu_push16(cpu, cpu->de); cpu->cycles += 16;"),
        0xD6 => format!("cpu_sub_a(cpu, 0x{:02X}); cpu->cycles += 8;", inst.bytes[1]),
        0xD7 => format!("cpu->cycles += 16; cpu_call(cpu, 0x0010, 0x{:04X}); return;", next_pc),
        0xD8 => format!("if (cpu->f & FLAG_C) {{ cpu->cycles += 20; cpu_ret(cpu); return; }} else {{ cpu->cycles += 8; }}"),
        0xD9 => format!("cpu->cycles += 16; cpu->ime = true; cpu_ret(cpu); return;"), // RETI
        0xDA => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            if is_local(val) {
                format!("if (cpu->f & FLAG_C) {{ cpu->cycles += 12; goto loc_{:02X}_{:04X}; }} else {{ cpu->cycles += 12; }}", current_bank, val)
            } else {
                format!("if (cpu->f & FLAG_C) {{ cpu->cycles += 12; goto_pc(cpu, 0x{:04X}); return; }} else {{ cpu->cycles += 12; }}", val)
            }
        }
        0xDC => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            format!("if (cpu->f & FLAG_C) {{ cpu->cycles += 24; cpu_call(cpu, 0x{:04X}, 0x{:04X}); return; }} else {{ cpu->cycles += 12; }}", val, next_pc)
        }
        0xDE => format!("cpu_sbc_a(cpu, 0x{:02X}); cpu->cycles += 8;", inst.bytes[1]),
        0xDF => format!("cpu->cycles += 16; cpu_call(cpu, 0x0018, 0x{:04X}); return;", next_pc),

        0xE0 => format!("write8(cpu, 0xFF00 + 0x{:02X}, cpu->a); cpu->cycles += 12;", inst.bytes[1]),
        0xE1 => format!("cpu->hl = cpu_pop16(cpu); cpu->cycles += 12;"),
        0xE2 => format!("write8(cpu, 0xFF00 + cpu->c, cpu->a); cpu->cycles += 8;"),
        0xE5 => format!("cpu_push16(cpu, cpu->hl); cpu->cycles += 16;"),
        0xE6 => format!("cpu_and_a(cpu, 0x{:02X}); cpu->cycles += 8;", inst.bytes[1]),
        0xE7 => format!("cpu->cycles += 16; cpu_call(cpu, 0x0020, 0x{:04X}); return;", next_pc),
        0xE8 => format!("cpu->sp = cpu_add_sp(cpu, {}); cpu->cycles += 16;", inst.bytes[1] as i8),
        0xE9 => format!("cpu->cycles += 4; goto_pc(cpu, cpu->hl); return;"), // JP HL (indirect)
        0xEA => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            format!("write8(cpu, 0x{:04X}, cpu->a); cpu->cycles += 16;", val)
        }
        0xEE => format!("cpu_xor_a(cpu, 0x{:02X}); cpu->cycles += 8;", inst.bytes[1]),
        0xEF => format!("cpu->cycles += 16; cpu_call(cpu, 0x0028, 0x{:04X}); return;", next_pc),

        0xF0 => format!("cpu->a = read8(cpu, 0xFF00 + 0x{:02X}); cpu->cycles += 12;", inst.bytes[1]),
        0xF1 => format!("cpu->af = cpu_pop16(cpu) & 0xFFF0; cpu->cycles += 12;"),
        0xF2 => format!("cpu->a = read8(cpu, 0xFF00 + cpu->c); cpu->cycles += 8;"),
        0xF3 => format!("cpu->ime = false; cpu->cycles += 4;"), // DI
        0xF5 => format!("cpu_push16(cpu, cpu->af); cpu->cycles += 16;"),
        0xF6 => format!("cpu_or_a(cpu, 0x{:02X}); cpu->cycles += 8;", inst.bytes[1]),
        0xF7 => format!("cpu->cycles += 16; cpu_call(cpu, 0x0030, 0x{:04X}); return;", next_pc),
        0xF8 => {
            let val = inst.bytes[1] as i8;
            format!("cpu->hl = cpu_add_sp(cpu, {}); cpu->cycles += 12;", val)
        }
        0xF9 => format!("cpu->sp = cpu->hl; cpu->cycles += 8;"),
        0xFA => {
            let val = u16::from_le_bytes([inst.bytes[1], inst.bytes[2]]);
            format!("cpu->a = read8(cpu, 0x{:04X}); cpu->cycles += 16;", val)
        }
        0xFB => format!("cpu->ime = true; cpu->cycles += 4;"), // EI
        0xFE => format!("cpu_cp_a(cpu, 0x{:02X}); cpu->cycles += 8;", inst.bytes[1]),
        0xFF => format!("cpu->cycles += 16; cpu_call(cpu, 0x0038, 0x{:04X}); return;", next_pc),

        _ => format!("// Unimplemented opcode 0x{:02X}", op),
    }
}

fn translate_cb_to_c(op: u8) -> String {
    let regs = ["b", "c", "d", "e", "h", "l", "hl", "a"];
    let reg_name = regs[(op & 0x07) as usize];
    let bit = (op >> 3) & 0x07;
    let alu_group = op >> 6;

    let read_expr = if reg_name == "hl" { "read8(cpu, cpu->hl)" } else { &format!("cpu->{}", reg_name) };
    let write_expr = |val| {
        if reg_name == "hl" {
            format!("write8(cpu, cpu->hl, {}); cpu->cycles += 16;", val)
        } else {
            format!("cpu->{} = {}; cpu->cycles += 8;", reg_name, val)
        }
    };

    match alu_group {
        0 => { // Rotates & Shifts
            let subtype = (op >> 3) & 0x07;
            match subtype {
                0 => write_expr(&format!("cpu_rlc(cpu, {})", read_expr)),
                1 => write_expr(&format!("cpu_rrc(cpu, {})", read_expr)),
                2 => write_expr(&format!("cpu_rl(cpu, {})", read_expr)),
                3 => write_expr(&format!("cpu_rr(cpu, {})", read_expr)),
                4 => write_expr(&format!("cpu_sla(cpu, {})", read_expr)),
                5 => write_expr(&format!("cpu_sra(cpu, {})", read_expr)),
                6 => write_expr(&format!("cpu_swap(cpu, {})", read_expr)),
                7 => write_expr(&format!("cpu_srl(cpu, {})", read_expr)),
                _ => unreachable!(),
            }
        }
        1 => { // BIT
            let cycles = if reg_name == "hl" { 12 } else { 8 };
            format!("cpu_bit(cpu, {}, {}); cpu->cycles += {};", bit, read_expr, cycles)
        }
        2 => { // RES
            write_expr(&format!("({} & ~(1 << {}))", read_expr, bit))
        }
        3 => { // SET
            write_expr(&format!("({} | (1 << {}))", read_expr, bit))
        }
        _ => unreachable!(),
    }
}
