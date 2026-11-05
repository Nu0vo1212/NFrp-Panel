@echo off
chcp 65001 >nul
setlocal EnableExtensions
cd /d "%~dp0"
title FrpPanel Dev Launcher

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0dev.ps1" %*
set "RC=%ERRORLEVEL%"

if not "%RC%"=="0" goto failed
goto done

:failed
echo.
echo [FrpPanel] Failed to start (exit code %RC%).
echo See the error above, or run: dev.bat help
pause
goto done

:done
endlocal & exit /b %RC%
