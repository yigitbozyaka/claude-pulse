@echo off
cd /d "%~dp0"
echo Building ClaudeUsageMonitor...
pyinstaller ClaudeUsageMonitor.spec --clean
if %errorlevel% neq 0 (
    echo.
    echo Build failed. Make sure the app is not running.
    pause
    exit /b 1
)
echo.
echo Build complete: dist\ClaudeUsageMonitor.exe
pause
