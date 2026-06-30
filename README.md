# pinentry-win

[![Release](https://img.shields.io/github/v/release/flejz/pinentry-win)](https://github.com/flejz/pinentry-win/releases/latest)

Fast, modern Windows-native pinentry for GnuPG — written in Rust.

A drop-in replacement for GNU pinentry on Windows, using the native Win32 API via [windows-rs](https://github.com/microsoft/windows-rs).
No cygwin, no Qt, no GTK — just a clean Win32 dialog that starts instantly.

## Download

Grab the latest `pinentry.exe` from [Releases](https://github.com/flejz/pinentry-win/releases) — no build required.

## Features

- Full Assuan IPC protocol compatibility (GETPIN, CONFIRM, MESSAGE, all SET* commands)
- Native Win32 dialog — starts in milliseconds
- Secure: zeroes passphrase from memory after use
- High DPI aware (PerMonitorV2)
- Visual styles (modern button rendering)
- Works with GnuPG 2.x / gpg4win

## Installation

### Download (recommended)
Download `pinentry.exe` from the [latest release](https://github.com/flejz/pinentry-win/releases/latest).

### Build from source
```
cargo build --release --target x86_64-pc-windows-msvc
copy target\x86_64-pc-windows-msvc\release\pinentry.exe "C:\Program Files (x86)\GnuPG\bin\pinentry.exe"
```

### Configure GnuPG

In `%APPDATA%\gnupg\gpg-agent.conf`:

```
pinentry-program C:/Program Files (x86)/GnuPG/bin/pinentry.exe
```

Then restart gpg-agent: `gpgconf --kill gpg-agent`

## Protocol support

Commands: SETDESC, SETPROMPT, SETERROR, SETTITLE, SETOK, SETCANCEL, SETNOTOK, GETPIN, CONFIRM, MESSAGE, GETINFO, BYE, RESET, NOP, OPTION, SETKEYINFO, CLEARPASSPHRASE, SETREPEAT, SETQUALITYBAR

## License

MIT
