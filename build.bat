@echo off
REM Build APK GridCraft 0.16.2-beta.1 (fix file .xlsm raksasa).
REM Cara pakai: file ini ada di root repo (branch fix-huge-xlsx); double-click dari root repo.
REM Prasyarat: JDK 21 (JAVA_HOME), Android SDK (ANDROID_HOME), Rust + cargo-ndk.
REM Lihat pc-build/README.md untuk langkah install prasyarat.
setlocal
cd /d %~dp0
if not defined JAVA_HOME (
  echo JAVA_HOME belum di-set. Ikuti langkah 2.1 di pc-build/README.md dulu.
  pause
  exit /b 1
)
if not defined ANDROID_HOME (
  if not defined ANDROID_SDK_ROOT (
    echo ANDROID_HOME belum di-set. Ikuti langkah 2.2 di pc-build/README.md dulu.
    pause
    exit /b 1
  )
)
call apps\gridcraft-android\android\gradlew.bat assembleRelease -Pgridcraft.abis=arm64-v8a
if errorlevel 1 (
  echo.
  echo BUILD GAGAL. Lihat pesan error di atas (atau pc-build/README.md bagian 5).
  pause
  exit /b 1
)
echo.
echo BUILD BERHASIL.
echo APK ada di: apps\gridcraft-android\android\app\build\outputs\apk\release\app-release.apk
echo Ingat: uninstall dulu GridCraft Play Store di HP sebelum install APK ini.
pause
