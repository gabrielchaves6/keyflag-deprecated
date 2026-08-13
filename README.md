<p align="center">
  <img src="rs/assets/logo.png" width="120" alt="KeyFlag logo">
</p>

<h1 align="center">KeyFlag</h1>

<p align="center">Keep Windows on a <b>single keyboard layout</b>, enforced globally, from a tray icon.</p>

---

## ⚠️ Deprecated — KeyFlag is part of DeskFlag now

**This repository is no longer maintained and no further releases will be cut here.** As of
**DeskFlag v0.2.46** (13 August 2026) the layout enforcer is a DeskFlag feature, living in
`rs/src/keyboard.rs` in [gabrielchaves6/desk_flag](https://github.com/gabrielchaves6/desk_flag) on a
thread of its own. The US/BR badge is an element on DeskFlag's taskbar card instead of a tray icon
of its own; everything else — the About window, the dialogs, the updater — DeskFlag already had.

**What to do:** install DeskFlag, then uninstall KeyFlag (Add or remove programs, or `unins000.exe`
in `%LOCALAPPDATA%\Programs\KeyFlag`). Running both is worse than running neither: two enforcers
take turns correcting each other several times a second and neither wins. DeskFlag asks first if it
finds this one running.

**One thing got better on the way over.** This app collapsed `HKCU\Keyboard Layout\Preload` to a
single entry and never put it back — switching modes was the only thing its menu could undo.
DeskFlag writes that list down *before* it changes it and restores it when you switch the hold off,
reloading the layouts into the running session so you get them back without signing out. If KeyFlag
has already collapsed your list, though, the list you had before is gone: it was never saved
anywhere. Re-add the layouts you want in Windows' language settings **before** switching DeskFlag's
hold on, and that is what it will remember.

The code below is left up for reference.

---

Written in Rust (`windows-rs`). A sibling of [DeskFlag](https://github.com/gabrielchaves6/desk_flag).

Windows' keyboard handling mixes layouts per-window, silently re-adds them, and scatters
a dozen switch shortcuts (Win+Space, Alt+Shift, Ctrl+Shift). KeyFlag picks **one** layout
and makes the whole session stay on it. While it's set to US International, you get US
International everywhere — no mixing. Same for ABNT2.

## What it does

- Two modes, chosen from the tray: **United States-International** (`00020409`) and
  **Portuguese (Brazilian ABNT2)** (`00010416`).
- **Snap-back enforcement.** A foreground-change hook plus a ~350 ms backup poll watch the
  active window; if anything (an app, the OS, or a stray Win+Space) drifts to a different
  layout, KeyFlag pushes it straight back to the chosen one. In practice the system switch
  shortcuts become no-ops.
- On a mode switch it flips every already-open window immediately (not just on next focus)
  and sets the default input language for new processes.
- **Tray-only.** No window, no taskbar entry. The tray icon *is* the indicator: a blue
  **US** badge or a green **BR** badge tells you which mode is active at a glance.
- Styled **About** window and an in-app **Check for updates** (pulls the latest installer
  from this repo's releases), matching DeskFlag.
- Remembers the last mode across restarts (`HKCU\Software\KeyFlag`).
- Single instance (a named mutex) — two enforcers would fight over the layout.

## Install

KeyFlag is a single, self-contained `KeyFlag.exe` (the MSVC runtime is statically linked,
so **no Visual C++ redistributable is needed**). It needs Windows 10/11.

### Option A — the installer (recommended)

1. Download `KeyFlag-Setup.exe` from a
   [Release](https://github.com/gabrielchaves6/keyflag/releases).
2. Run it. It installs per-user (no admin), into `%LOCALAPPDATA%\Programs\KeyFlag`, adds a
   Start-menu entry, and offers a *Start KeyFlag when I sign in to Windows* checkbox.

> **Heads-up — unsigned for now.** The installer isn't code-signed yet, so Windows
> SmartScreen may show *"Windows protected your PC"* (click **More info → Run anyway**), and
> Microsoft Defender may flag the setup with a generic false positive common for unsigned
> Inno Setup installers. The `KeyFlag.exe` itself is not flagged.

The installer is produced automatically by GitHub Actions on every version tag
(`installer/KeyFlag.iss`, built by `.github/workflows/release.yml`).

### Option B — copy the prebuilt executable

Build it (see below) or grab `KeyFlag.exe` from a Release, copy it anywhere, and run it.

### Start automatically with Windows

Drop a shortcut to `KeyFlag.exe` in the Startup folder (`Win+R` → `shell:startup`).

## Usage

Run KeyFlag. A small **US**/**BR** badge appears in the tray (possibly under the `^`
overflow). Right-click it:

- **US International** / **Português ABNT2** — pick the layout to lock to (the active one is
  dotted).
- **About KeyFlag** / **Check for updates…** / **Exit**

## Build from source

Requires the [Rust toolchain](https://rustup.rs). The C runtime is statically linked
(`rs/.cargo/config.toml`), so the resulting `KeyFlag.exe` is self-contained.

```powershell
cd rs
cargo build --release
```

The result is `rs\target\release\KeyFlag.exe`.

**Without Visual Studio**, use the GNU toolchain (it ships its own linker):

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu
cargo +stable-x86_64-pc-windows-gnu build --release
```

CI builds release binaries with the stock MSVC toolchain, which also embeds the app icon
(`rs/app.rc` → `rs/assets/keyflag.ico`); local GNU builds fall back to a runtime-drawn icon.
The logo is generated by `tools/make_logo.ps1`.

## How the enforcement works

Both layouts are loaded once at startup via `LoadKeyboardLayout`, giving two `HKL` handles.
The chosen one is the target. Enforcement is a single cheap function: read the foreground
window's thread layout with `GetKeyboardLayout`, and if it differs from the target, post
`WM_INPUTLANGCHANGEREQUEST` with the target `HKL` to that window — the same mechanism the
language bar uses, which works cross-process. It runs on every `EVENT_SYSTEM_FOREGROUND`
(via `SetWinEventHook`) and on a 350 ms timer as a backstop, and is a no-op when the layout
is already correct.
