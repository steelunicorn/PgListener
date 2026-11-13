@echo off
rem Запуск build.ps1 двойным щелчком или из cmd; параметры передаются как есть.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build.ps1" %*
set RC=%ERRORLEVEL%
if "%~1"=="" pause
exit /b %RC%
