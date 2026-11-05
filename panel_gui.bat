@echo off
setlocal
cd /d "%~dp0"

where pyw >nul 2>nul
if %errorlevel%==0 (
  start "" pyw -3 "%~dp0panel_gui.py" %*
  exit /b 0
)

where pythonw >nul 2>nul
if %errorlevel%==0 (
  start "" pythonw "%~dp0panel_gui.py" %*
  exit /b 0
)

python "%~dp0panel_gui.py" %*
exit /b %errorlevel%
