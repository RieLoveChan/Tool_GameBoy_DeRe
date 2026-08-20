# GameBoy / GameBoy Color Native Static Recompiler

This project compiles GameBoy (DMG) and GameBoy Color (CGB) ROMs into portable, high-performance native C code. The generated C code is built alongside a custom hardware simulation runtime and linked with SDL2 to generate standalone native executables for Windows, macOS, and Linux.

---

## Project Structure

*   **`recompiler/`**: The offline transpiler written in **Rust**. It parses the ROM header, decodes LR35902 assembly instructions, constructs basic blocks, and outputs decompiled portable C code.
*   **`runtime/`**: The C hardware runtime. It emulates:
    *   **CPU Interpreter**: A fallback CPU emulator to run dynamically loaded RAM routines (e.g. OAM DMA loops in HRAM).
    *   **Memory Controller**: Cartridge banking (MBC1, MBC3, MBC5) and GBC WRAM/VRAM bank-switching.
    *   **PPU (Pixel Processing Unit)**: DMG monochrome/CGB color scanline rendering, sprite-to-sprite priority, and tile/background layer priority.
    *   **APU (Audio Processing Unit)**: Emulates Square 1, Square 2, Wave Table, and Noise sound channels, synchronized using an SDL2 audio queue.
*   **`generated/`**: Holds the output C source files (`game.c`, `game.h`, `dispatcher.c`, and `rom_data.c`) produced by the transpiler.
*   **`CMakeLists.txt`**: Root build configuration. Fetches and links SDL2 statically.

---

## Prerequisites & Dependencies

To compile and run this project, make sure you have installed:
1.  **Rust Toolchain**: For compiling the offline transpiler.
2.  **CMake** (version 3.15 or higher).
3.  **C Compiler & Build System**: MSVC (Windows), GCC/Clang (Linux/macOS) with a generator like **Ninja** or Make.
4.  **Git**: Required by CMake to fetch SDL2.

---

## Quick Start (Windows)

For Windows users, you can compile and play any GameBoy ROM using the drag-and-drop helper:

1. Drag and drop your `.gb` or `.gbc` ROM file directly onto the [drop_rom_here.bat](file:///e:/Repos/Tool_GameBoy_DeRe/drop_rom_here.bat) script in the root directory.
2. The script will automatically build the Rust transpiler (if missing), transpile the ROM, compile the native executable, and launch the game.

## Step-by-Step Usage

### 1. Build the Rust Transpiler
Navigate to the `recompiler/` directory and compile the Rust transpiler:
```bash
cd recompiler
cargo build --release
cd ..
```

### 2. Transpile a ROM
Run the Rust recompiler on your target ROM:
```bash
# Force DMG mode (for original GameBoy Classic games)
recompiler/target/release/recompiler.exe "path/to/game.gb" --output-dir generated --mode dmg

# Force GBC mode (for GameBoy Color games)
recompiler/target/release/recompiler.exe "path/to/game.gbc" --output-dir generated --mode cgb

# Auto-detect mode based on the ROM header
recompiler/target/release/recompiler.exe "path/to/game.gb" --output-dir generated --mode auto
```
This generates the C sources inside the `generated/` directory.

### 3. Compile the Native Binary
Generate the build system and compile the binary using CMake:
```bash
# Configure the build directory
cmake -G "Ninja" -S . -B build

# Build the release executable
cmake --build build --config Release
```
This compiles the generated game C code, links the hardware runtime and SDL2, and produces `build/gb_game.exe` (or `build/gb_game` on Linux/macOS).

### 4. Run the Game
Run the executable directly on your host machine:
```bash
build\gb_game.exe
```
To enable execution logging, run with the `--log` flag:
```bash
build\gb_game.exe --log game_execution.log
```

---

**Dev Setup / Contributing**

- **Prereqs:** Install Rust (`cargo`), CMake (>=3.15), a C compiler (MSVC/GCC/Clang), `ninja` (or another generator), and Git.
- **ROMs:** Provide a legally-owned `.gb`/`.gbc` ROM (ROM files are intentionally not committed to the repo).

- **Build the recompiler:**
```bash
cd recompiler
cargo build --release
cd ..
```

- **Transpile a ROM (generates `generated/`):**
```bash
recompiler/target/release/recompiler.exe "path/to/game.gb" --output-dir generated --mode auto
```

- **Configure & build the native binary:**
```bash
cmake -G "Ninja" -S . -B build
cmake --build build --config Release
```

- **Run the game (Windows example):**
```powershell
build\gb_game.exe
```

- **Helper:** Drop a ROM onto [drop_rom_here.bat](drop_rom_here.bat) to run the full pipeline.

- **Reproducibility note:** `generated/` and ROM files are listed in `.gitignore`; other users must supply the ROM and rebuild `recompiler` to reproduce generated sources. If you want generated sources versioned, remove them from `.gitignore` and commit the outputs.

## Controls

*   **Arrow Keys**: D-Pad (Up/Down/Left/Right)
*   **Z Key**: A Button
*   **X Key**: B Button
*   **Spacebar**: Select
*   **Enter/Return**: Start

---

## Technical Highlights

*   **Perfect Audio Synchronization**: The execution speed of the main hardware loop is directly throttled to the SDL2 audio hardware queue size. This guarantees glitch-free, synchronized audio playback with under 30ms of latency, eliminating pitch-drift and crackling.
*   **Hybrid AOT/Interpreter Execution**: GameBoy ROM code is compiled ahead-of-time (AOT) to native functions. When the code jumps to RAM (like standard OAM DMA routines copied to HRAM), the runtime automatically drops back to the CPU fallback interpreter to run those blocks safely, resuming native execution once code returns to ROM boundaries.

AI use disclaimer: This is a personal experiment I did with Gemini 3.5 High and other models.
