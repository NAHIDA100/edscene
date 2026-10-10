@echo off
rem edscene installer entry point. Double-click this file.
rem Kept ASCII-only on purpose: cmd.exe reads .bat in the OEM codepage, so
rem non-ASCII text here would come out as mojibake. install.ps1 does the
rem Chinese output, and it is saved as UTF-8 with BOM for that reason.
cd /d "%~dp0"
net session >nul 2>&1
if %errorlevel% neq 0 (
    echo Need administrator rights to register the autostart service.
    echo A UAC prompt is opening...
    powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
    exit /b
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
echo.
pause
