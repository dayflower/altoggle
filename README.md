# altoggle

[![CI](https://github.com/dayflower/altoggle/actions/workflows/ci.yml/badge.svg)](https://github.com/dayflower/altoggle/actions/workflows/ci.yml)

<p align="center">
  <img src="design/appicon.png" alt="" width="160">
</p>

Switch the Windows IME with a **solo press of a modifier key**.

- Press and release **right Alt** on its own → IME **on**
- Press and release **left Alt** on its own → IME **off**
- Alt **with anything else** → ordinary Alt, untouched

No new key to learn and no chord to hit: the two keys already under your thumbs
become "start typing Japanese" and "stop typing Japanese", and they stay
ordinary Alt for everything else.

## The trade you are making

Whichever key you assign gives up its usual solo-press behaviour — Alt the menu
bar, Win the start menu, Menu the context menu.

You get it back by **holding the key**. A press longer than the threshold
(400ms by default) falls through to Windows untouched, so holding Alt still
opens the menu bar. That is the escape hatch, and it is deliberate.

Either direction can also be set to `(none)` if you only want one of the two.

## Requirements

- Windows 10 or 11, 64-bit (x86_64)
- A Japanese IME — Microsoft IME and Google Japanese Input are both known to
  work, across Notepad, Explorer, Chrome, VS Code and Store apps

altoggle sends the `VK_IME_ON` / `VK_IME_OFF` virtual keys. Under a US keyboard
layout those keys do nothing, so with no Japanese IME active the app is inert
rather than broken.

## Install

1. Download the zip from the [releases page][releases] and check the SHA256 if
   you like — it is published with each release.
2. **Unblock it.** Windows marks anything downloaded from the internet, and
   left marked the executable is treated as untrusted. Right-click the zip →
   *Properties* → tick *Unblock* → *OK*, **before** extracting.
3. Put `altoggle.exe` somewhere permanent — `%LOCALAPPDATA%\Programs\altoggle\`
   is a reasonable home. There is no installer; the exe is the whole app.
4. Run it. A tray icon appears.
5. To have it come back after a reboot, tick **Start with Windows** on the tray
   menu.

Autostart records the **path the exe has right now**. If you move or rename
`altoggle.exe` afterwards, Windows will keep launching the old path; untick
*Start with Windows* and tick it again to repoint it.

## Why Windows warns about it

altoggle is not code-signed, and it installs a low-level keyboard hook. Both
are things SmartScreen and some antivirus heuristics react to, so expect a
"Windows protected your PC" dialog (*More info* → *Run anyway*) on first launch.

That warning is about the absence of a certificate, not about anything the app
was observed doing. What it actually does:

- **No network access at all.** It opens no sockets and contacts nothing.
- **Writes exactly two things**: `%APPDATA%\altoggle\config.toml`, and one value
  under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` — and that one only
  while *Start with Windows* is ticked.
- **Runs unelevated.** It never asks for administrator rights.
- **The keyboard hook exists to see solo presses**, which is the one thing that
  cannot be done any other way: deciding whether Alt was pressed *alone* means
  watching what comes after it. Nothing is recorded, stored or transmitted. The
  hook is [`crates/app/src/hook.rs`](crates/app/src/hook.rs) and the whole
  source is here to read.

If you would rather not trust a binary from a stranger, build it yourself — see
below. It takes one command.

## Settings

**Settings…** on the tray menu opens a small dialog. It stays live while it is
open, so you can change the threshold, press *Apply*, press the key, and feel
whether the new value is right.

- **Trigger keys** — either direction can be Alt, Ctrl, Shift or Win (left or
  right), the Menu key, or `(none)`. Note that many Japanese laptops and JIS
  keyboards have no right Win key, and some have no Menu key; mixing sides
  (say left Win and right Alt) is fine.
- **Threshold** — how short a press still counts as a solo press. Deliberate
  solo presses were measured up to 215ms and auto-repeat starts around 500ms,
  so useful values sit between roughly 250 and 500ms.
- **Show the IME state on the tray icon** — off by default. When on, the tray
  icon shows a Latin *a* when the IME is off, a hiragana *あ* when it is on, and
  an asterisk when the foreground application will not say. It is off by default
  because reading the IME means asking the foreground application several times
  a second for as long as altoggle runs.
- **Dummy key** (under *Advanced*) — the key injected to make a solo press stop
  looking solo, which is what stops Windows opening the menu bar. Leave it alone
  unless something in your setup reacts badly to `0x07`.

The same values live in `%APPDATA%\altoggle\config.toml`, which is written with
explanatory comments on first run. Editing it by hand works, but only takes
effect at the next start — and the dialog rewrites the file whole when you save,
so anything you add to it will not survive.

**Reinstall hooks** on the tray menu is there for the rare case where Windows
drops the hook (it does so silently, and gives no way to ask). If solo presses
suddenly stop working, try that before restarting the app.

## Quitting

**Quit** on the tray menu. The release build has no console, so Ctrl+C does
nothing — the tray menu is the way out.

Quitting releases every modifier key on the way, which matters here: altoggle
suppresses the trigger key's release, and a modifier left logically held down
would turn every later keystroke into a chord.

## Uninstall

1. Untick **Start with Windows** on the tray menu.
2. **Quit**.
3. Delete `altoggle.exe` and the `%APPDATA%\altoggle\` folder.

That is everything. There is no installer state, no service, and nothing else in
the registry.

## Build from source

```
cargo build --release --bin altoggle
```

The result is `target\release\altoggle.exe`, a single self-contained executable
— the icons are compiled into it and there are no DLLs to ship beyond the MSVC
runtime.

One prerequisite is easy to miss: the build needs **`rc.exe` from the Windows
SDK**, to compile the icon and version resources. A Rust MSVC toolchain that has
`link.exe` but no SDK resource compiler fails here and nowhere else. Installing
the *Desktop development with C++* workload of Visual Studio Build Tools, with
the Windows SDK component, is enough.

`cargo build --release` without `--bin altoggle` also builds three developer
tools that are deliberately not distributed. Only `altoggle.exe` is the app.

## License

MIT — see [LICENSE](LICENSE).

[releases]: https://github.com/dayflower/altoggle/releases
