# Build APK GridCraft 0.16.2-beta.1 (fix file .xlsm raksasa) — di PC Windows, dari repo

Repo ini (branch `fix-huge-xlsx`) berisi source GridCraft yang sudah diperbaiki
supaya file Excel raksasa (ratusan ribu baris, jutaan sel) bisa dibuka, diedit,
dan disimpan tanpa "not responding". Yang diperbaiki: lewati hitung-ulang semua
formula saat buka file + COUNTIF yang tadinya kuadratik kini pakai cache
frekuensi. Hasil tes di mesin build: buka file stok 350 MB 17 detik (dari
~50 detik), edit 1 detik (dari 16,5 detik), save tetap pertahankan makro.

## 1. Ambil source

```bat
git clone https://github.com/aikomiai818/gridcraft-android.git
cd gridcraft-android
git checkout fix-huge-xlsx
```

## 2. Prasyarat (install sekali saja)

1. **JDK 21** — unduh Temurin 21 (LTS) dari adoptium.net, install, lalu set
   environment variable `JAVA_HOME` ke folder JDK-nya (mis.
   `C:\Program Files\Eclipse Adoptium\jdk-21.x.x`).
2. **Android SDK Command Line Tools** — unduh "commandlinetools-win" dari
   developer.android.com/studio#command-tools, ekstrak ke mis.
   `C:\Android\cmdline-tools\latest` (isi folder `latest` harus langsung
   berisi `bin`, `lib`, dst). Lalu di Command Prompt, jalankan
   (sesuaikan path):

   ```bat
   C:\Android\cmdline-tools\latest\bin\sdkmanager.bat "platform-tools" "platforms;android-36" "build-tools;36.0.0" "ndk;29.0.14206865"
   ```

   Set environment variable `ANDROID_HOME` ke `C:\Android`
   (atau `ANDROID_SDK_ROOT` — salah satu cukup).
3. **Rust** — install dari rustup.rs (jalankan `rustup-init.exe`, pilih
   install default). Lalu di Command Prompt:

   ```bat
   cargo install cargo-ndk --version 4.1.2 --locked
   ```

   Pastikan `cargo` ada di PATH (tutup lalu buka lagi Command Prompt setelah
   install Rust).

## 3. Build

Double-click `build.bat` di root repo ini. (Atau jalankan manual dari Command
Prompt di folder itu:
`apps\gridcraft-android\android\gradlew.bat assembleRelease -Pgridcraft.abis=arm64-v8a`.)

Build pertama agak lama (10–30 menit): unduh Gradle + dependensi, compile
Rust + Kotlin. Build berikutnya jauh lebih cepat (cache).

## 4. Hasil

APK ada di:

```text
apps\gridcraft-android\android\app\build\outputs\apk\release\app-release.apk
```

APK ini ditandatangani **debug key** (bukan key Play Store), jadi sebelum
install: **uninstall dulu GridCraft dari Play Store di HP**, baru install APK
ini. Versi di APK: 0.16.2-beta.1.

## 5. Kalau build gagal

- `JAVA_HOME is not set` → set JAVA_HOME ke folder JDK 21 (langkah 2.1).
- `SDK location not found` / `ANDROID_HOME is not set` → set ANDROID_HOME ke
  folder SDK (langkah 2.2).
- `cargo: command not found` / `cargo-ndk` tidak ketemu → install Rust +
  cargo-ndk (langkah 2.3), lalu buka Command Prompt baru.
- Error lain → kirimkan pesan error lengkapnya (copy dari jendela Command
  Prompt).

Source asli: https://github.com/artcraft-android/gridcraft-android
