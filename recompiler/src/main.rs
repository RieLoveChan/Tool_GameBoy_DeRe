use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

mod codegen;
mod disasm;

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmulationMode {
    Dmg,
    Cgb,
    Auto,
}

#[derive(Parser, Debug)]
#[command(name = "gb-recompiler", author, version, about = "GameBoy to Native Code Transpiler")]
struct Args {
    /// Path to the GameBoy ROM file (.gb or .gbc)
    rom_path: PathBuf,

    /// Output directory for the generated C files
    #[arg(short, long, default_value = "./generated")]
    output_dir: PathBuf,

    /// Target emulation mode: forced DMG (Classic), GBC (Color), or Auto-detect
    #[arg(short, long, value_enum, default_value_t = EmulationMode::Auto)]
    mode: EmulationMode,
}

pub struct RomInfo {
    pub title: String,
    pub is_gbc: bool,
    pub cartridge_type: u8,
    pub rom_size: usize,
    pub ram_size: usize,
    pub rom_data: Vec<u8>,
}

fn parse_rom_header(rom_data: &[u8], requested_mode: EmulationMode) -> Result<RomInfo> {
    if rom_data.len() < 0x0150 {
        anyhow::bail!("ROM file is too small to contain a valid GameBoy header.");
    }

    // Extract Title (0x0134 - 0x0143)
    let mut title_bytes = Vec::new();
    for i in 0x0134..0x0143 {
        let b = rom_data[i];
        if b == 0 {
            break;
        }
        if b.is_ascii_graphic() || b == b' ' {
            title_bytes.push(b);
        }
    }
    let title = String::from_utf8_lossy(&title_bytes).trim().to_string();

    // CGB Flag (0x0143): 0x80 = DMG/CGB compatible, 0xC0 = GBC only, other = DMG
    let cgb_flag = rom_data[0x0143];
    let is_gbc = match requested_mode {
        EmulationMode::Dmg => false,
        EmulationMode::Cgb => true,
        EmulationMode::Auto => cgb_flag == 0x80 || cgb_flag == 0xC0,
    };

    let cartridge_type = rom_data[0x0147];
    
    // ROM size mapping
    let rom_size_code = rom_data[0x0148];
    let rom_size = match rom_size_code {
        0x00 => 32 * 1024,      // 32KB
        0x01 => 64 * 1024,      // 64KB
        0x02 => 128 * 1024,     // 128KB
        0x03 => 256 * 1024,     // 256KB
        0x04 => 512 * 1024,     // 512KB
        0x05 => 1024 * 1024,    // 1MB
        0x06 => 2048 * 1024,    // 2MB
        0x07 => 4096 * 1024,    // 4MB
        0x08 => 8192 * 1024,    // 8MB
        _ => anyhow::bail!("Unknown ROM size code: 0x{:02X}", rom_size_code),
    };

    // RAM size mapping
    let ram_size_code = rom_data[0x0149];
    let ram_size = match ram_size_code {
        0x00 => 0,
        0x01 => 2 * 1024,       // 2KB
        0x02 => 8 * 1024,       // 8KB
        0x03 => 32 * 1024,      // 32KB (4 banks of 8KB)
        0x04 => 128 * 1024,     // 128KB (16 banks of 8KB)
        0x05 => 64 * 1024,      // 64KB (8 banks of 8KB)
        _ => anyhow::bail!("Unknown RAM size code: 0x{:02X}", ram_size_code),
    };

    Ok(RomInfo {
        title,
        is_gbc,
        cartridge_type,
        rom_size,
        ram_size,
        rom_data: rom_data.to_vec(),
    })
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("Loading ROM from: {:?}", args.rom_path);
    let mut file = File::open(&args.rom_path)
        .with_context(|| format!("Failed to open ROM file: {:?}", args.rom_path))?;
    let mut rom_data = Vec::new();
    file.read_to_end(&mut rom_data)?;

    let rom_info = parse_rom_header(&rom_data, args.mode)?;

    println!("ROM Information:");
    println!("  Title:          {}", rom_info.title);
    println!("  Emulation Mode: {}", if rom_info.is_gbc { "GameBoy Color (CGB)" } else { "GameBoy Classic (DMG)" });
    println!("  Cartridge Type: 0x{:02X}", rom_info.cartridge_type);
    println!("  ROM Size:       {} KB (Header Code: 0x{:02X})", rom_info.rom_size / 1024, rom_data[0x0148]);
    println!("  RAM Size:       {} KB (Header Code: 0x{:02X})", rom_info.ram_size / 1024, rom_data[0x0149]);

    // Ensure output directory exists
    std::fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("Failed to create output directory: {:?}", args.output_dir))?;

    println!("Analyzing ROM and generating control flow...");
    let disasm_result = disasm::analyze_rom(&rom_info)?;

    println!("Generating C code into: {:?}", args.output_dir);
    codegen::generate_c_project(&rom_info, &disasm_result, &args.output_dir)?;

    println!("Transpilation complete!");
    Ok(())
}
