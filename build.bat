@echo off
setlocal

where cargo >nul 2>&1
if errorlevel 1 (
    echo Error: cargo not found. Add Cargo to PATH and try again.
    exit /b 1
)

pushd "%~dp0"
if errorlevel 1 (
    echo Error: could not enter the project directory.
    exit /b 1
)

echo Building wcap...
call cargo build --release --manifest-path "Cargo.toml"
set "wcapBuildExit=%errorlevel%"
popd

if not "%wcapBuildExit%"=="0" (
    echo Error: build failed.
    exit /b %wcapBuildExit%
)

echo Build succeeded: "%~dp0target\release\wcap.exe"
exit /b 0
