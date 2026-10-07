@echo off
rem edscene 安装入口：双击这个文件即可，它会自己申请管理员权限。
rem 命令行等价物：powershell -NoProfile -ExecutionPolicy Bypass -File install.ps1
cd /d "%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"
echo.
pause
