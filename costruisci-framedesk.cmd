@echo off
rem Scarica (o aggiorna) e compila FrameDesk. Metti questo file in AmbienteRust.
title Costruisci FrameDesk
set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"
rem I pacchetti vanno estratti su un disco NTFS: exFAT/FAT32 (chiavette, dischi USB) non accettano le date
rem 1970 dei pacchetti e cargo fallisce con "failed to set mtime". Toolchain e programmi restano nella cartella.
set "CARGO_HOME=%LOCALAPPDATA%\AmbienteRust-cargo"
if not exist "%CARGO_HOME%" mkdir "%CARGO_HOME%"
set "RUSTUP_HOME=%ROOT%\rust\rustup"
rem Usa direttamente cargo e rustc della toolchain: non serve rustup (che su un PC nuovo prova a riscaricarla).
set "TC="
for /d %%t in ("%ROOT%\rust\rustup\toolchains\*") do if exist "%%t\bin\cargo.exe" set "TC=%%t\bin;"
set "PATH=%TC%%ROOT%\winlibs\mingw64\bin;%ROOT%\rust\cargo\bin;%ROOT%\git\cmd;%ROOT%\git\bin;%PATH%"
set "PROG=%ROOT%\Progetti"
if not exist "%PROG%" mkdir "%PROG%"
cd /d "%PROG%"
taskkill /f /im framedesk.exe >nul 2>&1

echo === Scarico o aggiorno FrameDesk ===
if exist "%PROG%\FrameDesk\.git" (
  git -c "safe.directory=*" -C "%PROG%\FrameDesk" fetch origin
  if errorlevel 1 goto fail
  git -c "safe.directory=*" -C "%PROG%\FrameDesk" reset --hard origin/main
) else (
  git clone https://github.com/lorenzodapolito-oss/FrameDesk "%PROG%\FrameDesk"
)
if errorlevel 1 goto fail

echo === Compilo ===
cd /d "%PROG%\FrameDesk"
cargo build --release
if errorlevel 1 goto fail

set "OUT=%PROG%\FrameDesk\dist\FrameDesk-portable"
if exist "%OUT%" rmdir /s /q "%OUT%"
mkdir "%OUT%"
copy /y "target\release\framedesk.exe" "%OUT%\FrameDesk.exe" >nul
if errorlevel 1 goto fail

if exist "%ROOT%\ffmpeg\bin\ffmpeg.exe" (
  copy /y "%ROOT%\ffmpeg\bin\ffmpeg.exe" "%OUT%\" >nul
  copy /y "%ROOT%\ffmpeg\bin\ffprobe.exe" "%OUT%\" >nul
) else echo NOTA: metti ffmpeg.exe e ffprobe.exe accanto a FrameDesk.exe (o nel PATH).
echo.
echo FATTO: %OUT%\FrameDesk.exe
pause
exit /b 0

:fail
echo.
echo ERRORE: operazione non riuscita. Controlla i messaggi sopra.
pause
exit /b 1
