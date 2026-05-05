@echo off
setlocal EnableDelayedExpansion
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"

if not exist "%VSWHERE%" (
  echo [NVDARemoteAudioServer] vswhere.exe not found.
  exit /b 1
)

set "VSINSTALL="
for /f "usebackq delims=" %%I in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do (
  set "VSINSTALL=%%I"
)

if not defined VSINSTALL (
  echo [NVDARemoteAudioServer] Visual Studio C++ Build Tools not found.
  exit /b 1
)

call "%VSINSTALL%\Common7\Tools\VsDevCmd.bat" -arch=x64 -host_arch=x64
if errorlevel 1 (
  echo [NVDARemoteAudioServer] Failed to activate Visual Studio developer environment.
  exit /b 1
)

set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
set "RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc"

if defined VCToolsInstallDir set "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=!VCToolsInstallDir!bin\Hostx64\x64\link.exe"

if "%~1"=="" (
echo [NVDARemoteAudioServer] Development environment is ready.
echo [NVDARemoteAudioServer] Rust toolchain: !RUSTUP_TOOLCHAIN!
if defined CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER echo [NVDARemoteAudioServer] Linker: !CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER!
  cmd /k
  exit /b %errorlevel%
)

%*
