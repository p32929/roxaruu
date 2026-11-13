@echo off
rem `rox` with nothing after it opens the folder you are in (as on macOS
rem and Linux); `rox <path>` opens that one.
if "%~1"=="" ("%~dp0roxaruu.exe" . & exit /b)
"%~dp0roxaruu.exe" %*
