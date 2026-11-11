@echo off
rem Double-click to install or update pr2md for the current user (no admin needed).
rem Arguments are passed on, e.g.  install.cmd -Uninstall
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
if errorlevel 1 pause
