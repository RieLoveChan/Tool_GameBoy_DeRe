cmake_minimum_required(VERSION 3.15)
project(gb-recompiled C)

set(CMAKE_C_STANDARD 99)
set(CMAKE_C_STANDARD_REQUIRED ON)

# Configure FetchContent to download SDL2
include(FetchContent)
FetchContent_Declare(
    SDL2
    GIT_REPOSITORY https://github.com/libsdl-org/SDL.git
    GIT_TAG release-2.30.3
)

# On Windows, build SDL2 as static library or configure options
set(SDL_SHARED OFF CACHE BOOL "Build shared SDL library" FORCE)
set(SDL_STATIC ON CACHE BOOL "Build static SDL library" FORCE)
set(SDL_TEST OFF CACHE BOOL "Build SDL test programs" FORCE)

message(STATUS "Fetching SDL2 dependency...")
FetchContent_MakeAvailable(SDL2)

# Define include directories
include_directories(
    ${CMAKE_CURRENT_SOURCE_DIR}/runtime
    ${CMAKE_CURRENT_SOURCE_DIR}/generated
)

# Runtime Source Files
set(RUNTIME_SOURCES
    runtime/main.c
    runtime/memory.c
    runtime/ppu.c
    runtime/apu.c
    runtime/interpreter.c
)

# Check if generated files exist. If not, we will compile with a placeholder
# so the configuration succeeds before transpilation has been run.
if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/generated/game.c")
    message(STATUS "Found generated game source files. Compiling target...")
    set(GAME_SOURCES
        generated/game.c
        generated/dispatcher.c
        generated/rom_data.c
    )
else()
    message(WARNING "Generated source files not found at generated/. Running with mock game placeholder. Please run the transpiler first.")
    
    # Write a mock game.c / dispatcher.c / rom_data.c so configuration succeeds
    file(MAKE_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/generated")
    file(WRITE "${CMAKE_CURRENT_SOURCE_DIR}/generated/game.h" 
        "#ifndef GAME_H\n#define GAME_H\n#include \"runtime.h\"\nvoid fn_00_0100(CPUState* cpu);\n#endif\n"
    )
    file(WRITE "${CMAKE_CURRENT_SOURCE_DIR}/generated/game.c" 
        "#include \"game.h\"\nvoid fn_00_0100(CPUState* cpu) { cpu->halted = true; }\n"
    )
    file(WRITE "${CMAKE_CURRENT_SOURCE_DIR}/generated/dispatcher.c" 
        "#include \"game.h\"\nvoid dispatch(CPUState* cpu, uint16_t bank, uint16_t pc) { fn_00_0100(cpu); }\n"
    )
    file(WRITE "${CMAKE_CURRENT_SOURCE_DIR}/generated/rom_data.c" 
        "#include <stdint.h>\n#include <stddef.h>\nconst uint8_t g_rom_data[1] = {0};\nconst size_t g_rom_size = 1;\nconst char* g_rom_title = \"Mock Game\";\nconst uint8_t g_rom_is_gbc = 0;\nconst uint8_t g_rom_cartridge_type = 0;\nconst size_t g_rom_ram_size = 0;\n"
    )
    
    set(GAME_SOURCES
        generated/game.c
        generated/dispatcher.c
        generated/rom_data.c
    )
endif()

# Define Executable Target
add_executable(gb_game
    ${RUNTIME_SOURCES}
    ${GAME_SOURCES}
)

# Link SDL2 (SDL2-static or SDL2)
if(TARGET SDL2-static)
    target_link_libraries(gb_game PRIVATE SDL2-static)
else()
    target_link_libraries(gb_game PRIVATE SDL2)
endif()

# Copy SDL2 runtime libraries on Windows if needed (not needed for static linking)
