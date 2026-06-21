use std::collections::{BTreeMap, BTreeSet, VecDeque};
use crate::RomInfo;
use anyhow::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowControl {
    Normal,
    Jump(u16),          // Direct jump to 16-bit PC (in same bank or Bank 0)
    JumpIndirect,       // Jump to (HL)
    Branch(u16),        // Conditional branch to 16-bit PC
    Call(u16),          // Direct subroutine call to 16-bit PC
    Return,             // RET, RETI
    Halt,               // HALT, STOP
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DecodedInstruction {
    pub pc: u16,
    pub bank: u16,
    pub bytes: Vec<u8>,
    pub mnemonic: String,
    pub flow: FlowControl,
}

pub struct DisasmResult {
    /// Maps absolute ROM address -> DecodedInstruction
    pub instructions: BTreeMap<u32, DecodedInstruction>,
    /// Set of absolute ROM addresses that are function entry points
    pub function_entries: BTreeSet<u32>,
    /// Set of absolute ROM addresses that are targets of local jumps/branches
    pub jump_targets: BTreeSet<u32>,
}

/// Helper to get absolute ROM address from a bank and a 16-bit PC
pub fn get_abs_addr(bank: u16, pc: u16) -> u32 {
    if pc < 0x4000 {
        pc as u32
    } else if pc < 0x8000 {
        let b = if bank == 0 { 1 } else { bank };
        (b as u32) * 16384 + (pc - 0x4000) as u32
    } else {
        // High/RAM address
        pc as u32 | 0x80000000
    }
}

pub fn analyze_rom(rom: &RomInfo) -> Result<DisasmResult> {
    let mut instructions = BTreeMap::new();
    let mut function_entries = BTreeSet::new();
    let mut jump_targets = BTreeSet::new();

    // Queue of absolute ROM addresses to analyze
    // Each entry is (bank, pc, is_function_start)
    let mut queue = VecDeque::new();
    let total_banks = rom.rom_data.len() / 16384;

    // Add entry point (0x0100 is always Bank 0)
    queue.push_back((0, 0x0100, true));
    function_entries.insert(0x0100);

    // Add Interrupt Vectors (always Bank 0)
    let vectors = [0x0040, 0x0048, 0x0050, 0x0058, 0x0060];
    for &vec in &vectors {
        queue.push_back((0, vec, true));
        function_entries.insert(vec as u32);
    }

    // Keep track of visited absolute addresses to avoid infinite loops
    let mut visited = BTreeSet::new();

    while let Some((bank, start_pc, is_func)) = queue.pop_front() {
        let mut pc = start_pc;
        let start_abs = get_abs_addr(bank, pc);

        if visited.contains(&start_abs) {
            if is_func && start_pc < 0x8000 {
                function_entries.insert(start_abs);
            }
            continue;
        }

        if is_func && start_pc < 0x8000 {
            function_entries.insert(start_abs);
        }

        // Trace instruction flow
        loop {
            let abs_addr = get_abs_addr(bank, pc);
            if visited.contains(&abs_addr) {
                // We met already-analyzed code, link and stop tracing this path
                jump_targets.insert(abs_addr);
                break;
            }

            // Read instruction bytes from ROM
            let rom_offset = if pc < 0x4000 {
                pc as usize
            } else if pc < 0x8000 {
                let b = if bank == 0 { 1 } else { bank };
                (b as usize) * 16384 + (pc - 0x4000) as usize
            } else {
                // PC is in RAM/high memory, can't statically disassemble
                break;
            };

            if rom_offset >= rom.rom_data.len() {
                // Out of ROM bounds
                break;
            }

            let normalized_bank = if pc >= 0x4000 && pc < 0x8000 && bank == 0 { 1 } else { bank };
            let instr = match decode_instruction(&rom.rom_data[rom_offset..], pc, normalized_bank) {
                Some(inst) => inst,
                None => {
                    // Invalid/unknown instruction
                    break;
                }
            };

            let instr_len = instr.bytes.len();
            visited.insert(abs_addr);
            instructions.insert(abs_addr, instr.clone());

            // Process control flow
            match instr.flow {
                FlowControl::Normal => {
                    pc += instr_len as u16;
                }
                 FlowControl::Jump(target_pc) => {
                    if target_pc < 0x8000 {
                        if bank == 0 && target_pc >= 0x4000 {
                            for b in 1..total_banks as u16 {
                                let target_abs = get_abs_addr(b, target_pc);
                                jump_targets.insert(target_abs);
                                queue.push_back((b, target_pc, false));
                            }
                        } else {
                            let target_abs = get_abs_addr(bank, target_pc);
                            jump_targets.insert(target_abs);
                            queue.push_back((bank, target_pc, false));
                        }
                    }
                    break; // Unconditional jump terminates current linear tracing
                }
                FlowControl::JumpIndirect => {
                    break; // Cannot trace statically
                }
                FlowControl::Branch(target_pc) => {
                    if target_pc < 0x8000 {
                        if bank == 0 && target_pc >= 0x4000 {
                            for b in 1..total_banks as u16 {
                                let target_abs = get_abs_addr(b, target_pc);
                                jump_targets.insert(target_abs);
                                queue.push_back((b, target_pc, false));
                            }
                        } else {
                            let target_abs = get_abs_addr(bank, target_pc);
                            jump_targets.insert(target_abs);
                            queue.push_back((bank, target_pc, false));
                        }
                    }
                    pc += instr_len as u16; // Continue tracing next instruction
                }
                FlowControl::Call(target_pc) => {
                    if target_pc < 0x8000 {
                        if bank == 0 && target_pc >= 0x4000 {
                            for b in 1..total_banks as u16 {
                                queue.push_back((b, target_pc, true));
                            }
                        } else {
                            queue.push_back((bank, target_pc, true));
                        }
                    }
                    let return_pc = pc + instr_len as u16;
                    if return_pc < 0x8000 {
                        queue.push_back((bank, return_pc, true));
                    }
                    pc = return_pc; // Continue tracing after the call returns
                }
                FlowControl::Return => {
                    break; // Return terminates linear tracing
                }
                FlowControl::Halt => {
                    pc += instr_len as u16; // Continue after halt (interrupt wakes it up)
                }
            }
        }
    }

    // Also disassemble other banks sequentially to find unreferenced code
    // (useful for bank switching or jump tables that weren't resolved).
    for b in 0..total_banks as u16 {
        let min_pc = if b == 0 { 0x0150 } else { 0x4000 };
        let max_pc = if b == 0 { 0x4000 } else { 0x8000 };
        let mut pc = min_pc;
        while pc < max_pc {
            let abs_addr = get_abs_addr(b, pc);
            if !visited.contains(&abs_addr) {
                // Check if it looks like a valid instruction stream
                let rom_offset = (abs_addr & 0x7FFFFFFF) as usize;
                if rom_offset < rom.rom_data.len() {
                    // Peek if it's not all zeros/FFs
                    let val = rom.rom_data[rom_offset];
                    if val != 0x00 && val != 0xFF {
                        // Queue this as a speculative function
                        queue.push_back((b, pc, true));
                        // Run a mini trace
                        while let Some((sub_b, sub_pc, is_sub_f)) = queue.pop_front() {
                            let sub_abs = get_abs_addr(sub_b, sub_pc);
                             if visited.contains(&sub_abs) {
                                 if is_sub_f && sub_pc < 0x8000 {
                                     function_entries.insert(sub_abs);
                                 }
                                 continue;
                             }
                             if is_sub_f && sub_pc < 0x8000 { function_entries.insert(sub_abs); }
                            let mut t_pc = sub_pc;
                            loop {
                                let t_abs = get_abs_addr(sub_b, t_pc);
                                if visited.contains(&t_abs) { break; }
                                 let sub_offset = if t_pc < 0x4000 { t_pc as usize } else {
                                     let b = if sub_b == 0 { 1 } else { sub_b };
                                     (b as usize) * 16384 + (t_pc - 0x4000) as usize
                                 };
                                 if sub_offset >= rom.rom_data.len() { break; }
                                 let normalized_sub_b = if t_pc >= 0x4000 && t_pc < 0x8000 && sub_b == 0 { 1 } else { sub_b };
                                 if let Some(inst) = decode_instruction(&rom.rom_data[sub_offset..], t_pc, normalized_sub_b) {
                                    let l = inst.bytes.len() as u16;
                                    visited.insert(t_abs);
                                    instructions.insert(t_abs, inst.clone());
                                    match inst.flow {
                                        FlowControl::Normal => t_pc += l,
                                         FlowControl::Jump(tgt) => {
                                             if tgt < 0x8000 {
                                                 if sub_b == 0 && tgt >= 0x4000 {
                                                     for b in 1..total_banks as u16 {
                                                         jump_targets.insert(get_abs_addr(b, tgt));
                                                         queue.push_back((b, tgt, false));
                                                     }
                                                 } else {
                                                     jump_targets.insert(get_abs_addr(sub_b, tgt));
                                                     queue.push_back((sub_b, tgt, false));
                                                 }
                                             }
                                             break;
                                         }
                                         FlowControl::JumpIndirect => break,
                                         FlowControl::Branch(tgt) => {
                                             if tgt < 0x8000 {
                                                 if sub_b == 0 && tgt >= 0x4000 {
                                                     for b in 1..total_banks as u16 {
                                                         jump_targets.insert(get_abs_addr(b, tgt));
                                                         queue.push_back((b, tgt, false));
                                                     }
                                                 } else {
                                                     jump_targets.insert(get_abs_addr(sub_b, tgt));
                                                     queue.push_back((sub_b, tgt, false));
                                                 }
                                             }
                                             t_pc += l;
                                         }
                                         FlowControl::Call(tgt) => {
                                             if tgt < 0x8000 {
                                                 if sub_b == 0 && tgt >= 0x4000 {
                                                     for b in 1..total_banks as u16 {
                                                         queue.push_back((b, tgt, true));
                                                     }
                                                 } else {
                                                     queue.push_back((sub_b, tgt, true));
                                                 }
                                             }
                                             let return_pc = t_pc + l;
                                             if return_pc < 0x8000 {
                                                 queue.push_back((sub_b, return_pc, true));
                                             }
                                             t_pc = return_pc;
                                         }
                                        FlowControl::Return => break,
                                        FlowControl::Halt => t_pc += l,
                                    }
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            // Move forward based on disassembled instruction if available, otherwise 1 byte
            let abs_addr = get_abs_addr(b, pc);
            if let Some(inst) = instructions.get(&abs_addr) {
                pc += inst.bytes.len() as u16;
            } else {
                pc += 1;
            }
        }
    }

    // Promote cross-function jump/branch targets to function entry points
    loop {
        // Group instructions by their closest preceding function entry point
        let mut func_groups: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        let mut current_func = 0x0100; // Default fallback function

        for &abs_addr in instructions.keys() {
            if function_entries.contains(&abs_addr) {
                current_func = abs_addr;
            }
            func_groups.entry(current_func).or_default().push(abs_addr);
        }

        // Map instruction absolute address -> its function group entry
        let mut addr_to_func = BTreeMap::new();
        for (&func_abs, instrs) in &func_groups {
            for &abs in instrs {
                addr_to_func.insert(abs, func_abs);
            }
        }

        let mut newly_added = false;
        // Check all jump/branch instructions
        for (&abs_addr, inst) in &instructions {
            if let Some(&src_func) = addr_to_func.get(&abs_addr) {
                let targets = match inst.flow {
                    FlowControl::Jump(tgt) | FlowControl::Branch(tgt) => {
                        if tgt < 0x8000 {
                            Some(get_abs_addr(inst.bank, tgt))
                        } else {
                            None
                        }
                    }
                    _ => None,
                };

                if let Some(tgt_abs) = targets {
                    if let Some(&tgt_func) = addr_to_func.get(&tgt_abs) {
                        if tgt_func != src_func {
                            // The jump crosses a function boundary! Promote target to function entry
                            if function_entries.insert(tgt_abs) {
                                newly_added = true;
                            }
                        }
                    }
                }
            }
        }

        if !newly_added {
            break;
        }
    }

    Ok(DisasmResult {
        instructions,
        function_entries,
        jump_targets,
    })
}

/// Decode a single LR35902 CPU instruction
fn decode_instruction(data: &[u8], pc: u16, bank: u16) -> Option<DecodedInstruction> {
    if data.is_empty() {
        return None;
    }

    let op = data[0];
    let mut len = 1;
    let mut flow = FlowControl::Normal;
    let mnemonic;

    match op {
        // --- 0x00 to 0x0F ---
        0x00 => mnemonic = "NOP".to_string(),
        0x01 => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("LD BC, 0x{:04X}", val);
            len = 3;
        }
        0x02 => mnemonic = "LD (BC), A".to_string(),
        0x03 => mnemonic = "INC BC".to_string(),
        0x04 => mnemonic = "INC B".to_string(),
        0x05 => mnemonic = "DEC B".to_string(),
        0x06 => {
            if data.len() < 2 { return None; }
            mnemonic = format!("LD B, 0x{:02X}", data[1]);
            len = 2;
        }
        0x07 => mnemonic = "RLCA".to_string(),
        0x08 => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("LD (0x{:04X}), SP", val);
            len = 3;
        }
        0x09 => mnemonic = "ADD HL, BC".to_string(),
        0x0A => mnemonic = "LD A, (BC)".to_string(),
        0x0B => mnemonic = "DEC BC".to_string(),
        0x0C => mnemonic = "INC C".to_string(),
        0x0D => mnemonic = "DEC C".to_string(),
        0x0E => {
            if data.len() < 2 { return None; }
            mnemonic = format!("LD C, 0x{:02X}", data[1]);
            len = 2;
        }
        0x0F => mnemonic = "RRCA".to_string(),

        // --- 0x10 to 0x1F ---
        0x10 => {
            if data.len() < 2 { return None; }
            mnemonic = "STOP".to_string();
            flow = FlowControl::Halt;
            len = 2;
        }
        0x11 => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("LD DE, 0x{:04X}", val);
            len = 3;
        }
        0x12 => mnemonic = "LD (DE), A".to_string(),
        0x13 => mnemonic = "INC DE".to_string(),
        0x14 => mnemonic = "INC D".to_string(),
        0x15 => mnemonic = "DEC D".to_string(),
        0x16 => {
            if data.len() < 2 { return None; }
            mnemonic = format!("LD D, 0x{:02X}", data[1]);
            len = 2;
        }
        0x17 => mnemonic = "RLA".to_string(),
        0x18 => {
            if data.len() < 2 { return None; }
            let offset = data[1] as i8;
            let target = ((pc as i32) + 2 + (offset as i32)) as u16;
            mnemonic = format!("JR 0x{:04X}", target);
            flow = FlowControl::Jump(target);
            len = 2;
        }
        0x19 => mnemonic = "ADD HL, DE".to_string(),
        0x1A => mnemonic = "LD A, (DE)".to_string(),
        0x1B => mnemonic = "DEC DE".to_string(),
        0x1C => mnemonic = "INC E".to_string(),
        0x1D => mnemonic = "DEC E".to_string(),
        0x1E => {
            if data.len() < 2 { return None; }
            mnemonic = format!("LD E, 0x{:02X}", data[1]);
            len = 2;
        }
        0x1F => mnemonic = "RRA".to_string(),

        // --- 0x20 to 0x2F ---
        0x20 => {
            if data.len() < 2 { return None; }
            let offset = data[1] as i8;
            let target = ((pc as i32) + 2 + (offset as i32)) as u16;
            mnemonic = format!("JR NZ, 0x{:04X}", target);
            flow = FlowControl::Branch(target);
            len = 2;
        }
        0x21 => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("LD HL, 0x{:04X}", val);
            len = 3;
        }
        0x22 => mnemonic = "LD (HL+), A".to_string(),
        0x23 => mnemonic = "INC HL".to_string(),
        0x24 => mnemonic = "INC H".to_string(),
        0x25 => mnemonic = "DEC H".to_string(),
        0x26 => {
            if data.len() < 2 { return None; }
            mnemonic = format!("LD H, 0x{:02X}", data[1]);
            len = 2;
        }
        0x27 => mnemonic = "DAA".to_string(),
        0x28 => {
            if data.len() < 2 { return None; }
            let offset = data[1] as i8;
            let target = ((pc as i32) + 2 + (offset as i32)) as u16;
            mnemonic = format!("JR Z, 0x{:04X}", target);
            flow = FlowControl::Branch(target);
            len = 2;
        }
        0x29 => mnemonic = "ADD HL, HL".to_string(),
        0x2A => mnemonic = "LD A, (HL+)".to_string(),
        0x2B => mnemonic = "DEC HL".to_string(),
        0x2C => mnemonic = "INC L".to_string(),
        0x2D => mnemonic = "DEC L".to_string(),
        0x2E => {
            if data.len() < 2 { return None; }
            mnemonic = format!("LD L, 0x{:02X}", data[1]);
            len = 2;
        }
        0x2F => mnemonic = "CPL".to_string(),

        // --- 0x30 to 0x3F ---
        0x30 => {
            if data.len() < 2 { return None; }
            let offset = data[1] as i8;
            let target = ((pc as i32) + 2 + (offset as i32)) as u16;
            mnemonic = format!("JR NC, 0x{:04X}", target);
            flow = FlowControl::Branch(target);
            len = 2;
        }
        0x31 => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("LD SP, 0x{:04X}", val);
            len = 3;
        }
        0x32 => mnemonic = "LD (HL-), A".to_string(),
        0x33 => mnemonic = "INC SP".to_string(),
        0x34 => mnemonic = "INC (HL)".to_string(),
        0x35 => mnemonic = "DEC (HL)".to_string(),
        0x36 => {
            if data.len() < 2 { return None; }
            mnemonic = format!("LD (HL), 0x{:02X}", data[1]);
            len = 2;
        }
        0x37 => mnemonic = "SCF".to_string(),
        0x38 => {
            if data.len() < 2 { return None; }
            let offset = data[1] as i8;
            let target = ((pc as i32) + 2 + (offset as i32)) as u16;
            mnemonic = format!("JR C, 0x{:04X}", target);
            flow = FlowControl::Branch(target);
            len = 2;
        }
        0x39 => mnemonic = "ADD HL, SP".to_string(),
        0x3A => mnemonic = "LD A, (HL-)".to_string(),
        0x3B => mnemonic = "DEC SP".to_string(),
        0x3C => mnemonic = "INC A".to_string(),
        0x3D => mnemonic = "DEC A".to_string(),
        0x3E => {
            if data.len() < 2 { return None; }
            mnemonic = format!("LD A, 0x{:02X}", data[1]);
            len = 2;
        }
        0x3F => mnemonic = "CCF".to_string(),

        // --- 0x40 to 0x7F: LD instructions ---
        0x40 => mnemonic = "LD B, B".to_string(),
        0x41 => mnemonic = "LD B, C".to_string(),
        0x42 => mnemonic = "LD B, D".to_string(),
        0x43 => mnemonic = "LD B, E".to_string(),
        0x44 => mnemonic = "LD B, H".to_string(),
        0x45 => mnemonic = "LD B, L".to_string(),
        0x46 => mnemonic = "LD B, (HL)".to_string(),
        0x47 => mnemonic = "LD B, A".to_string(),
        0x48 => mnemonic = "LD C, B".to_string(),
        0x49 => mnemonic = "LD C, C".to_string(),
        0x4A => mnemonic = "LD C, D".to_string(),
        0x4B => mnemonic = "LD C, E".to_string(),
        0x4C => mnemonic = "LD C, H".to_string(),
        0x4D => mnemonic = "LD C, L".to_string(),
        0x4E => mnemonic = "LD C, (HL)".to_string(),
        0x4F => mnemonic = "LD C, A".to_string(),

        0x50 => mnemonic = "LD D, B".to_string(),
        0x51 => mnemonic = "LD D, C".to_string(),
        0x52 => mnemonic = "LD D, D".to_string(),
        0x53 => mnemonic = "LD D, E".to_string(),
        0x54 => mnemonic = "LD D, H".to_string(),
        0x55 => mnemonic = "LD D, L".to_string(),
        0x56 => mnemonic = "LD D, (HL)".to_string(),
        0x57 => mnemonic = "LD D, A".to_string(),
        0x58 => mnemonic = "LD E, B".to_string(),
        0x59 => mnemonic = "LD E, C".to_string(),
        0x5A => mnemonic = "LD E, D".to_string(),
        0x5B => mnemonic = "LD E, E".to_string(),
        0x5C => mnemonic = "LD E, H".to_string(),
        0x5D => mnemonic = "LD E, L".to_string(),
        0x5E => mnemonic = "LD E, (HL)".to_string(),
        0x5F => mnemonic = "LD E, A".to_string(),

        0x60 => mnemonic = "LD H, B".to_string(),
        0x61 => mnemonic = "LD H, C".to_string(),
        0x62 => mnemonic = "LD H, D".to_string(),
        0x63 => mnemonic = "LD H, E".to_string(),
        0x64 => mnemonic = "LD H, H".to_string(),
        0x65 => mnemonic = "LD H, L".to_string(),
        0x66 => mnemonic = "LD H, (HL)".to_string(),
        0x67 => mnemonic = "LD H, A".to_string(),
        0x68 => mnemonic = "LD L, B".to_string(),
        0x69 => mnemonic = "LD L, C".to_string(),
        0x6A => mnemonic = "LD L, D".to_string(),
        0x6B => mnemonic = "LD L, E".to_string(),
        0x6C => mnemonic = "LD L, H".to_string(),
        0x6D => mnemonic = "LD L, L".to_string(),
        0x6E => mnemonic = "LD L, (HL)".to_string(),
        0x6F => mnemonic = "LD L, A".to_string(),

        0x70 => mnemonic = "LD (HL), B".to_string(),
        0x71 => mnemonic = "LD (HL), C".to_string(),
        0x72 => mnemonic = "LD (HL), D".to_string(),
        0x73 => mnemonic = "LD (HL), E".to_string(),
        0x74 => mnemonic = "LD (HL), H".to_string(),
        0x75 => mnemonic = "LD (HL), L".to_string(),
        0x76 => {
            mnemonic = "HALT".to_string();
            flow = FlowControl::Halt;
        }
        0x77 => mnemonic = "LD (HL), A".to_string(),
        0x78 => mnemonic = "LD A, B".to_string(),
        0x79 => mnemonic = "LD A, C".to_string(),
        0x7A => mnemonic = "LD A, D".to_string(),
        0x7B => mnemonic = "LD A, E".to_string(),
        0x7C => mnemonic = "LD A, H".to_string(),
        0x7D => mnemonic = "LD A, L".to_string(),
        0x7E => mnemonic = "LD A, (HL)".to_string(),
        0x7F => mnemonic = "LD A, A".to_string(),

        // --- ALU Arithmetic Operations 0x80 to 0xBF ---
        0x80 => mnemonic = "ADD A, B".to_string(),
        0x81 => mnemonic = "ADD A, C".to_string(),
        0x82 => mnemonic = "ADD A, D".to_string(),
        0x83 => mnemonic = "ADD A, E".to_string(),
        0x84 => mnemonic = "ADD A, H".to_string(),
        0x85 => mnemonic = "ADD A, L".to_string(),
        0x86 => mnemonic = "ADD A, (HL)".to_string(),
        0x87 => mnemonic = "ADD A, A".to_string(),
        0x88 => mnemonic = "ADC A, B".to_string(),
        0x89 => mnemonic = "ADC A, C".to_string(),
        0x8A => mnemonic = "ADC A, D".to_string(),
        0x8B => mnemonic = "ADC A, E".to_string(),
        0x8C => mnemonic = "ADC A, H".to_string(),
        0x8D => mnemonic = "ADC A, L".to_string(),
        0x8E => mnemonic = "ADC A, (HL)".to_string(),
        0x8F => mnemonic = "ADC A, A".to_string(),

        0x90 => mnemonic = "SUB B".to_string(),
        0x91 => mnemonic = "SUB C".to_string(),
        0x92 => mnemonic = "SUB D".to_string(),
        0x93 => mnemonic = "SUB E".to_string(),
        0x94 => mnemonic = "SUB H".to_string(),
        0x95 => mnemonic = "SUB L".to_string(),
        0x96 => mnemonic = "SUB (HL)".to_string(),
        0x97 => mnemonic = "SUB A".to_string(),
        0x98 => mnemonic = "SBC A, B".to_string(),
        0x99 => mnemonic = "SBC A, C".to_string(),
        0x9A => mnemonic = "SBC A, D".to_string(),
        0x9B => mnemonic = "SBC A, E".to_string(),
        0x9C => mnemonic = "SBC A, H".to_string(),
        0x9D => mnemonic = "SBC A, L".to_string(),
        0x9E => mnemonic = "SBC A, (HL)".to_string(),
        0x9F => mnemonic = "SBC A, A".to_string(),

        0xA0 => mnemonic = "AND B".to_string(),
        0xA1 => mnemonic = "AND C".to_string(),
        0xA2 => mnemonic = "AND D".to_string(),
        0xA3 => mnemonic = "AND E".to_string(),
        0xA4 => mnemonic = "AND H".to_string(),
        0xA5 => mnemonic = "AND L".to_string(),
        0xA6 => mnemonic = "AND (HL)".to_string(),
        0xA7 => mnemonic = "AND A".to_string(),
        0xA8 => mnemonic = "XOR B".to_string(),
        0xA9 => mnemonic = "XOR C".to_string(),
        0xAA => mnemonic = "XOR D".to_string(),
        0xAB => mnemonic = "XOR E".to_string(),
        0xAC => mnemonic = "XOR H".to_string(),
        0xAD => mnemonic = "XOR L".to_string(),
        0xAE => mnemonic = "XOR (HL)".to_string(),
        0xAF => mnemonic = "XOR A".to_string(),

        0xB0 => mnemonic = "OR B".to_string(),
        0xB1 => mnemonic = "OR C".to_string(),
        0xB2 => mnemonic = "OR D".to_string(),
        0xB3 => mnemonic = "OR E".to_string(),
        0xB4 => mnemonic = "OR H".to_string(),
        0xB5 => mnemonic = "OR L".to_string(),
        0xB6 => mnemonic = "OR (HL)".to_string(),
        0xB7 => mnemonic = "OR A".to_string(),
        0xB8 => mnemonic = "CP B".to_string(),
        0xB9 => mnemonic = "CP C".to_string(),
        0xBA => mnemonic = "CP D".to_string(),
        0xBB => mnemonic = "CP E".to_string(),
        0xBC => mnemonic = "CP H".to_string(),
        0xBD => mnemonic = "CP L".to_string(),
        0xBE => mnemonic = "CP (HL)".to_string(),
        0xBF => mnemonic = "CP A".to_string(),

        // --- 0xC0 to 0xFF ---
        0xC0 => mnemonic = "RET NZ".to_string(), // Conditional ret doesn't *always* break flow, but can. We handle it as Normal but with runtime branching. Actually, we mark ret as FlowControl::Normal and handle the jump/ret condition in C. Wait, conditional ret can return, so it splits the block. Let's make it FlowControl::Branch(next_pc) or FlowControl::Normal with C if check. Treating it as normal is easier for codegen because we can just write: `if (!(cpu->f & FLAG_Z)) { return cpu_ret(cpu); }`. Yes! Thus FlowControl::Normal is perfect.
        0xC1 => mnemonic = "POP BC".to_string(),
        0xC2 => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("JP NZ, 0x{:04X}", val);
            flow = FlowControl::Branch(val);
            len = 3;
        }
        0xC3 => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("JP 0x{:04X}", val);
            flow = FlowControl::Jump(val);
            len = 3;
        }
        0xC4 => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("CALL NZ, 0x{:04X}", val);
            flow = FlowControl::Call(val);
            len = 3;
        }
        0xC5 => mnemonic = "PUSH BC".to_string(),
        0xC6 => {
            if data.len() < 2 { return None; }
            mnemonic = format!("ADD A, 0x{:02X}", data[1]);
            len = 2;
        }
        0xC7 => {
            mnemonic = "RST 0x00".to_string();
            flow = FlowControl::Call(0x0000);
        }
        0xC8 => mnemonic = "RET Z".to_string(),
        0xC9 => {
            mnemonic = "RET".to_string();
            flow = FlowControl::Return;
        }
        0xCA => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("JP Z, 0x{:04X}", val);
            flow = FlowControl::Branch(val);
            len = 3;
        }
        0xCB => {
            // CB prefix byte
            if data.len() < 2 { return None; }
            let cb_op = data[1];
            len = 2;
            mnemonic = decode_cb_instruction(cb_op);
        }
        0xCC => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("CALL Z, 0x{:04X}", val);
            flow = FlowControl::Call(val);
            len = 3;
        }
        0xCD => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("CALL 0x{:04X}", val);
            flow = FlowControl::Call(val);
            len = 3;
        }
        0xCE => {
            if data.len() < 2 { return None; }
            mnemonic = format!("ADC A, 0x{:02X}", data[1]);
            len = 2;
        }
        0xCF => {
            mnemonic = "RST 0x08".to_string();
            flow = FlowControl::Call(0x0008);
        }

        0xD0 => mnemonic = "RET NC".to_string(),
        0xD1 => mnemonic = "POP DE".to_string(),
        0xD2 => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("JP NC, 0x{:04X}", val);
            flow = FlowControl::Branch(val);
            len = 3;
        }
        0xD3 => return None, // Invalid opcode
        0xD4 => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("CALL NC, 0x{:04X}", val);
            flow = FlowControl::Call(val);
            len = 3;
        }
        0xD5 => mnemonic = "PUSH DE".to_string(),
        0xD6 => {
            if data.len() < 2 { return None; }
            mnemonic = format!("SUB 0x{:02X}", data[1]);
            len = 2;
        }
        0xD7 => {
            mnemonic = "RST 0x10".to_string();
            flow = FlowControl::Call(0x0010);
        }
        0xD8 => mnemonic = "RET C".to_string(),
        0xD9 => {
            mnemonic = "RETI".to_string();
            flow = FlowControl::Return;
        }
        0xDA => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("JP C, 0x{:04X}", val);
            flow = FlowControl::Branch(val);
            len = 3;
        }
        0xDB => return None, // Invalid opcode
        0xDC => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("CALL C, 0x{:04X}", val);
            flow = FlowControl::Call(val);
            len = 3;
        }
        0xDD => return None, // Invalid opcode
        0xDE => {
            if data.len() < 2 { return None; }
            mnemonic = format!("SBC A, 0x{:02X}", data[1]);
            len = 2;
        }
        0xDF => {
            mnemonic = "RST 0x18".to_string();
            flow = FlowControl::Call(0x0018);
        }

        0xE0 => {
            if data.len() < 2 { return None; }
            mnemonic = format!("LDH (0xFF00 + 0x{:02X}), A", data[1]);
            len = 2;
        }
        0xE1 => mnemonic = "POP HL".to_string(),
        0xE2 => mnemonic = "LD (0xFF00 + C), A".to_string(),
        0xE3 => return None, // Invalid opcode
        0xE4 => return None, // Invalid opcode
        0xE5 => mnemonic = "PUSH HL".to_string(),
        0xE6 => {
            if data.len() < 2 { return None; }
            mnemonic = format!("AND 0x{:02X}", data[1]);
            len = 2;
        }
        0xE7 => {
            mnemonic = "RST 0x20".to_string();
            flow = FlowControl::Call(0x0020);
        }
        0xE8 => {
            if data.len() < 2 { return None; }
            mnemonic = format!("ADD SP, {}", data[1] as i8);
            len = 2;
        }
        0xE9 => {
            mnemonic = "JP HL".to_string();
            flow = FlowControl::JumpIndirect;
        }
        0xEA => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("LD (0x{:04X}), A", val);
            len = 3;
        }
        0xEB => return None, // Invalid opcode
        0xEC => return None, // Invalid opcode
        0xED => return None, // Invalid opcode
        0xEE => {
            if data.len() < 2 { return None; }
            mnemonic = format!("XOR 0x{:02X}", data[1]);
            len = 2;
        }
        0xEF => {
            mnemonic = "RST 0x28".to_string();
            flow = FlowControl::Call(0x0028);
        }

        0xF0 => {
            if data.len() < 2 { return None; }
            mnemonic = format!("LDH A, (0xFF00 + 0x{:02X})", data[1]);
            len = 2;
        }
        0xF1 => mnemonic = "POP AF".to_string(),
        0xF2 => mnemonic = "LD A, (0xFF00 + C)".to_string(),
        0xF3 => mnemonic = "DI".to_string(),
        0xF4 => return None, // Invalid opcode
        0xF5 => mnemonic = "PUSH AF".to_string(),
        0xF6 => {
            if data.len() < 2 { return None; }
            mnemonic = format!("OR 0x{:02X}", data[1]);
            len = 2;
        }
        0xF7 => {
            mnemonic = "RST 0x30".to_string();
            flow = FlowControl::Call(0x0030);
        }
        0xF8 => {
            if data.len() < 2 { return None; }
            mnemonic = format!("LD HL, SP + {}", data[1] as i8);
            len = 2;
        }
        0xF9 => mnemonic = "LD SP, HL".to_string(),
        0xFA => {
            if data.len() < 3 { return None; }
            let val = u16::from_le_bytes([data[1], data[2]]);
            mnemonic = format!("LD A, (0x{:04X})", val);
            len = 3;
        }
        0xFB => mnemonic = "EI".to_string(),
        0xFC => return None, // Invalid opcode
        0xFD => return None, // Invalid opcode
        0xFE => {
            if data.len() < 2 { return None; }
            mnemonic = format!("CP 0x{:02X}", data[1]);
            len = 2;
        }
        0xFF => {
            mnemonic = "RST 0x38".to_string();
            flow = FlowControl::Call(0x0038);
        }
    }

    let bytes = data[..len].to_vec();
    Some(DecodedInstruction {
        pc,
        bank,
        bytes,
        mnemonic,
        flow,
    })
}

fn decode_cb_instruction(op: u8) -> String {
    let reg_names = ["B", "C", "D", "E", "H", "L", "(HL)", "A"];
    let reg = reg_names[(op & 0x07) as usize];
    let bit = (op >> 3) & 0x07;

    match op {
        0x00..=0x07 => format!("RLC {}", reg),
        0x08..=0x0F => format!("RRC {}", reg),
        0x10..=0x17 => format!("RL {}", reg),
        0x18..=0x1F => format!("RR {}", reg),
        0x20..=0x27 => format!("SLA {}", reg),
        0x28..=0x2F => format!("SRA {}", reg),
        0x30..=0x37 => format!("SWAP {}", reg),
        0x38..=0x3F => format!("SRL {}", reg),
        0x40..=0x7F => format!("BIT {}, {}", bit, reg),
        0x80..=0xBF => format!("RES {}, {}", bit, reg),
        0xC0..=0xFF => format!("SET {}, {}", bit, reg),
    }
}
