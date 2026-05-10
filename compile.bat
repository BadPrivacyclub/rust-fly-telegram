@echo off
setlocal enabledelayedexpansion

echo ========================================
echo   fly-telegram ^| Windows build script
echo ========================================
echo.

:: --- Check Rust / Cargo ---
where cargo >nul 2>&1
if errorlevel 1 (
    echo [ERROR] Rust is not installed or not in PATH.
    echo.
    echo Install it from https://rustup.rs/
    echo   1. Download and run rustup-init.exe
    echo   2. Follow the on-screen instructions
    echo   3. Restart this terminal and run compile.bat again
    echo.
    pause
    exit /b 1
)

for /f "tokens=*" %%v in ('cargo --version 2^>^&1') do set CARGO_VER=%%v
echo [OK] Found %CARGO_VER%

:: --- Check C compiler (MSVC via cl.exe) ---
where cl >nul 2>&1
if errorlevel 1 (
    echo.
    echo [WARNING] cl.exe - MSVC C compiler - not found in PATH.
    echo Lua 5.4 requires a C compiler to build.
    echo.
    echo To fix this, open this terminal from:
    echo   "Developer Command Prompt for VS" or "x64 Native Tools Command Prompt"
    echo.
    echo Alternatively, install Visual Studio Build Tools:
    echo   https://aka.ms/vs/17/release/vs_BuildTools.exe
    echo   Select "Desktop development with C++"
    echo.
    set /p CONTINUE="Continue anyway? y/N: "
    if /i "!CONTINUE!" neq "y" (
        exit /b 1
    )
)

where cl >nul 2>&1
if not errorlevel 1 (
    for /f "tokens=*" %%v in ('cl 2^>^&1 ^| findstr /i "version"') do echo [OK] C compiler: %%v
)

:: --- Check Git (optional, for version info) ---
where git >nul 2>&1
if errorlevel 1 (
    echo [INFO] Git not found - skipping version check
) else (
    for /f "tokens=*" %%v in ('git --version 2^>^&1') do echo [OK] %%v
)

echo.
echo Building in release mode...
echo.

cargo build --release
if errorlevel 1 (
    echo.
    echo [ERROR] Build failed. See output above for details.
    pause
    exit /b 1
)

echo.
echo ========================================
echo   Build successful!
echo ========================================
echo.
echo Binary: target\release\fly-telegram.exe
echo.
echo To run:
echo   PowerShell: $env:TELOXIDE_TOKEN = "your_bot_token_here"
echo   cmd.exe:    set TELOXIDE_TOKEN=your_bot_token_here
echo   .\target\release\fly-telegram.exe
echo.
pause
