@echo off
setlocal

:: Check if a file was dropped
if "%~1"=="" (
    echo ========================================================
    echo  GameBoy Static Recompiler Drag-and-Drop Helper
    echo ========================================================
    echo.
    echo  Drag and drop any GameBoy .gb or GameBoy Color .gbc
    echo  ROM file onto this batch script to transpile and run it.
    echo.
    pause
    exit /b
)

echo [1/4] Checking Rust transpiler...
if not exist "recompiler\target\release\recompiler.exe" (
    echo Transpiler binary not found. Building it now...
    cd recompiler
    cargo build --release
    if errorlevel 1 (
        echo Error: Failed to build Rust recompiler.
        pause
        exit /b
    )
    cd ..
)

echo [2/4] Transpiling ROM: "%~nx1"...
recompiler\target\release\recompiler.exe "%~1" --output-dir generated --mode auto
if errorlevel 1 (
    echo Error: Transpilation failed.
    pause
    exit /b
)

echo [3/4] Building native executable...
if not exist "build" (
    cmake -G "Ninja" -S . -B build
    if errorlevel 1 (
        echo Error: CMake configuration failed.
        pause
        exit /b
    )
)

cmake --build build --config Release
if errorlevel 1 (
    echo Error: Compilation failed.
    pause
    exit /b
)

echo [4/4] Starting the game...
build\gb_game.exe

echo.
echo Game execution finished. Press any key to close this window.
pause
