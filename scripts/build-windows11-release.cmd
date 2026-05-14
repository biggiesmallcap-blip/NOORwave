@echo off
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-windows11-release.ps1" %*
set EXITCODE=%ERRORLEVEL%
echo.
if not "%EXITCODE%"=="0" (
  echo Build did not finish. Exit code: %EXITCODE%.
) else (
  echo Build finished.
)
echo.
pause
exit /b %EXITCODE%
