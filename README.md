# KeyboardFlag

Keep Windows on a **single keyboard layout**, enforced globally, from a tray icon.
Written in Rust (`windows-rs`). A sibling of [DeskFlag](https://github.com/gabrielchaves6/desk_flag).

Windows' keyboard handling mixes layouts per-window, silently re-adds them, and scatters
a dozen switch shortcuts (Win+Space, Alt+Shift, Ctrl+Shift). KeyboardFlag picks **one**
layout and makes the whole session stay on it. While it's set to US International, you get
US International everywhere — no mixing. Same for ABNT2.

## What it does

- Two modes, chosen from the tray: **United States-International** (`00020409`) and
  **Portuguese (Brazilian ABNT2)** (`00010416`).
- **Snap-back enforcement.** A foreground-change hook plus a ~350 ms backup poll watch the
  active window; if anything (an app, the OS, or a stray Win+Space) drifts to a different
  layout, KeyboardFlag pushes it straight back to the chosen one. In practice the system
  switch shortcuts become no-ops.
- On a mode switch it flips every already-open window immediately (not just on next focus)
  and sets the default input language for new processes.
- **Tray-only.** No window, no taskbar entry. The tray icon *is* the indicator: a blue
  **US** badge or a green **BR** badge tells you which mode is active at a glance.
- Remembers the last mode across restarts (`HKCU\Software\KeyboardFlag`).
- Single instance (a named mutex) — two enforcers would fight over the layout.

## Usage

Run `KeyboardFlag.exe`. A small **US**/**BR** badge appears in the tray (possibly under the
`^` overflow). Right-click it:

- **US International** / **Português ABNT2** — pick the layout to lock to (the active one is
  dotted).
- **About KeyboardFlag**
- **Exit**

To start automatically with Windows, drop a shortcut to `KeyboardFlag.exe` in the Startup
folder (`Win+R` → `shell:startup`).

## Build from source

Requires the [Rust toolchain](https://rustup.rs). The C runtime is statically linked
(`rs/.cargo/config.toml`), so the resulting `KeyboardFlag.exe` is self-contained — no Visual
C++ redistributable needed.

```powershell
cd rs
cargo build --release
```

The result is `rs\target\release\KeyboardFlag.exe`.

**Without Visual Studio**, use the GNU toolchain (it ships its own linker):

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu
cargo +stable-x86_64-pc-windows-gnu build --release
```

## How the enforcement works

Both layouts are loaded once at startup via `LoadKeyboardLayout`, giving two `HKL` handles.
The chosen one is the target. Enforcement is a single cheap function: read the foreground
window's thread layout with `GetKeyboardLayout`, and if it differs from the target, post
`WM_INPUTLANGCHANGEREQUEST` with the target `HKL` to that window — the same mechanism the
language bar uses, which works cross-process. It runs on every `EVENT_SYSTEM_FOREGROUND`
(via `SetWinEventHook`) and on a 350 ms timer as a backstop, and is a no-op when the layout
is already correct.

## Status

First cut: the working tray app and the enforcement engine. Installer, CI release, and
auto-update (mirroring DeskFlag's setup) are not wired up yet.
