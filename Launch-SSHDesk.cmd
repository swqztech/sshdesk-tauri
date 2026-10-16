@echo off
chcp 65001 >nul
set "EXE=%~dp0src-tauri\target\release\ssh_desk.exe"
if exist "%EXE%" (
  start "" "%EXE%"
  exit /b 0
)
echo 未找到正式版 ssh_desk.exe，请先执行: npm run build:client
pause
