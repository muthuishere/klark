@echo off
rem klark installer for Windows. One line, in Command Prompt:
rem
rem   curl -fsSL https://muthuishere.github.io/klark/install.cmd -o "%TEMP%\klark-install.cmd" && "%TEMP%\klark-install.cmd"
rem
rem Downloads klark.exe from the GitHub release, checks its sha256, installs it for your
rem user (no admin needed) and adds it to your PATH. Uses only what Windows 10 and later
rem ship: curl.exe, certutil and reg.
rem
rem   set KLARK_VERSION=v0.9.5.5   install that release (default: the latest)
rem   set INSTALL_DIR=C:\tools     where to put it (default: %LOCALAPPDATA%\klark\bin)
rem
rem The build is x64; it also runs on Windows on ARM. Windows support is new and has had
rem little real use: please report problems.
setlocal EnableExtensions

set "REPO=muthuishere/klark"
set "FILE=klark-windows-x86_64.exe"
set "SUMFILE=klark-windows-x86_64.sha256"
set "BASE=https://github.com/%REPO%/releases/latest/download"
if defined KLARK_VERSION set "BASE=https://github.com/%REPO%/releases/download/%KLARK_VERSION%"
set "DEST=%LOCALAPPDATA%\klark\bin"
if defined INSTALL_DIR set "DEST=%INSTALL_DIR%"

where curl >nul 2>&1
if errorlevel 1 goto :nocurl

set "TMPD=%TEMP%\klark-install-%RANDOM%%RANDOM%"
mkdir "%TMPD%" >nul 2>&1

rem Preferred: the zip with klark.exe plus Microsoft's modern ConPTY (conpty\), which keeps the
rem answers agents send back to the terminal. Falls back to the bare exe (Windows' own ConPTY).
set "ZIP=klark-windows-x86_64.zip"
where tar >nul 2>&1
if errorlevel 1 goto :barefile
echo Downloading %ZIP% ...
curl -fsSL "%BASE%/%ZIP%" -o "%TMPD%\%ZIP%" 2>nul
if errorlevel 1 goto :barefile
curl -fsSL "%BASE%/%ZIP%.sha256" -o "%TMPD%\%ZIP%.sha256" 2>nul
if errorlevel 1 goto :barefile
set "WANT="
for /f "usebackq tokens=1" %%H in ("%TMPD%\%ZIP%.sha256") do if not defined WANT set "WANT=%%H"
set "GOT="
for /f "skip=1 tokens=*" %%H in ('certutil -hashfile "%TMPD%\%ZIP%" SHA256') do if not defined GOT set "GOT=%%H"
set "GOT=%GOT: =%"
if not defined WANT goto :sumfail
if /i not "%WANT%"=="%GOT%" goto :sumfail
if not exist "%DEST%" mkdir "%DEST%"
if exist "%DEST%\conpty" rmdir /s /q "%DEST%\conpty"
tar -xf "%TMPD%\%ZIP%" -C "%DEST%"
if errorlevel 1 goto :copyfail
goto :installed

:barefile
set "WANT="
set "GOT="
echo Downloading %FILE% ...
curl -fsSL "%BASE%/%FILE%" -o "%TMPD%\%FILE%"
if errorlevel 1 goto :downloadfail
curl -fsSL "%BASE%/%SUMFILE%" -o "%TMPD%\%SUMFILE%"
if errorlevel 1 goto :downloadfail

rem The expected hash is the first word of the .sha256 file; certutil prints the real one on line 2.
set "WANT="
for /f "usebackq tokens=1" %%H in ("%TMPD%\%SUMFILE%") do if not defined WANT set "WANT=%%H"
set "GOT="
for /f "skip=1 tokens=*" %%H in ('certutil -hashfile "%TMPD%\%FILE%" SHA256') do if not defined GOT set "GOT=%%H"
set "GOT=%GOT: =%"
if not defined WANT goto :sumfail
if /i not "%WANT%"=="%GOT%" goto :sumfail

if not exist "%DEST%" mkdir "%DEST%"
copy /y "%TMPD%\%FILE%" "%DEST%\klark.exe" >nul
if errorlevel 1 goto :copyfail

:installed
echo Installed: %DEST%\klark.exe
"%DEST%\klark.exe" --version
rem The agent skill for Claude Code (%USERPROFILE%\.claude) and %USERPROFILE%\.agents, where those exist.
"%DEST%\klark.exe" --install-skill >nul 2>&1 && echo Installed the klark agent skill ^(skills\klark^)

rem Add the folder to the user PATH (not the machine PATH) unless it is there already.
set "UPATH="
for /f "tokens=2,*" %%A in ('reg query "HKCU\Environment" /v Path 2^>nul') do set "UPATH=%%B"
echo ;%UPATH%; | find /i ";%DEST%;" >nul
if not errorlevel 1 goto :done
if defined UPATH (set "NEWPATH=%UPATH%;%DEST%") else (set "NEWPATH=%DEST%")
reg add "HKCU\Environment" /v Path /t REG_EXPAND_SZ /d "%NEWPATH%" /f >nul
rem setx tells Windows the environment changed, so new terminals pick the PATH up now.
setx KLARK_INSTALL_DIR "%DEST%" >nul
echo Added %DEST% to your PATH. Open a new terminal to use "klark" there.

:done
rmdir /s /q "%TMPD%" >nul 2>&1
echo.
echo Run: klark
exit /b 0

:nocurl
echo klark install: curl.exe was not found. It ships with Windows 10 and later; on older Windows download klark-windows-x86_64.exe from https://github.com/%REPO%/releases/latest by hand.
exit /b 1

:downloadfail
echo klark install: download failed: %BASE%
rmdir /s /q "%TMPD%" >nul 2>&1
exit /b 1

:sumfail
echo klark install: checksum mismatch (expected %WANT%, got %GOT%); nothing installed.
rmdir /s /q "%TMPD%" >nul 2>&1
exit /b 1

:copyfail
echo klark install: cannot write %DEST%\klark.exe. If klark is running, close it (klark server stop) and run this again.
rmdir /s /q "%TMPD%" >nul 2>&1
exit /b 1
