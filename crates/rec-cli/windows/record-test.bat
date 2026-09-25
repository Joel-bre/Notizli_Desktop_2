@echo off
cd /d "%~dp0"
echo.
echo  Notizli recording test
echo  ----------------------
echo  Start this DURING a call (Teams, Zoom, ...) and let the other person talk.
echo  It records for 3 minutes. Press Enter to stop earlier.
echo  Leave this window open until Notepad shows the result.
echo.
notizli-rec.exe record --minutes 3 --out "%~dp0."
if exist "%~dp0notizli-test-result.txt" notepad "%~dp0notizli-test-result.txt"
