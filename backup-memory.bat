@echo off
setlocal enabledelayedexpansion
title FrameForge Memory Backup
cd /d "%~dp0"

set MEMORY_PATH=%USERPROFILE%\.claude\projects\c--Users-Jochem-Desktop-warframe-companion\memory

if not exist "!MEMORY_PATH!" (
    echo ERROR: Memory folder not found at !MEMORY_PATH!
    pause & exit /b 1
)

cd /d "!MEMORY_PATH!"

git add -A
git diff --cached --quiet
if %errorlevel% equ 0 (
    echo Memory is already up to date - nothing to backup.
    timeout /t 2 >nul
    exit /b 0
)

for /f "delims=" %%d in ('powershell -NoProfile -Command "Get-Date -Format 'yyyy-MM-dd HH:mm'"') do set TIMESTAMP=%%d
git commit -m "Memory backup !TIMESTAMP!"
git push origin main

if !errorlevel! equ 0 (
    echo.
    echo Memory backed up successfully.
) else (
    echo.
    echo ERROR: Push failed. Check your internet connection or GitHub credentials.
)
timeout /t 3 >nul
