# AGENTS.md

Guidance for agents working in this repository. It covers the things that are
costly to rediscover, not the things the code already says.

## What this is

A Windows resident app that switches the IME from a *solo* press of a modifier
key. Right Alt alone turns the IME on, left Alt alone turns it off, and Alt with
anything else stays ordinary Alt. Personal tool, unelevated, one user.

## Layout

```
crates/core   State machine only. no_std, no windows-sys, tests run on any OS.
crates/app    Win32 adapter, the app, and the verification tools.
```

**Keeping `core` OS-independent is the point, not an aesthetic.** Every bug worth
fearing here (auto-repeat, both Alts at once, Alt+drag, a modifier already held,
threshold edges) is reachable from synthetic events, and that is where they get
pinned down. Do not let a Win32 type leak into it.

Binaries in `crates/app`:

| Binary     | Purpose |
|------------|---------|
| `altoggle` | The app. `src/main.rs` |
| `keylog`   | Dumps raw low-level hook events. Never blocks input |
| `altprobe` | Proves the dummy-key suppression works, without touching the IME |
| `imeprobe` | Suppression plus IME switching, with the IME state read back |

The probes are kept because they isolate one layer each. When something breaks,
they tell you *which* layer.

## Rules for the hook callback

The callback must return within `LowLevelHooksTimeout` (300ms by default) or
Windows drops the hook silently, with no notification and no way to query it.

Inside `keyboard_proc` / `mouse_proc`:

- **No `SendMessage`, no COM, no file or console I/O.** `crate::log` only pushes
  to a channel; the log thread does the writing
- **No `Machine::set_config`.** Config changes arrive as `PostThreadMessage` and
  are applied by the message loop
- **No IMM32.** `ime::read_open_status` uses `SendMessageTimeout`, which waits on
  another process's message loop
- `GetAsyncKeyState` is fine (no cross-process message)

Events we injected ourselves are filtered out by `inject::INJECT_TAG` in
`dwExtraInfo` before they reach the state machine. Removing that check loops
forever: our injected Alt up fires the machine, which injects another.

The state machine lives in a `thread_local` in `hook.rs` because low-level hook
callbacks and out-of-context WinEvent callbacks all run on the thread that
installed the hook.

## Running this on the machine you are developing on

The app **blocks real Alt up events**. A bug that wedges the message loop takes
the keyboard with it.

- `--exit-after=<seconds>` makes any build quit on its own. Use it for any run
  you start from an agent session
- Ctrl+C works in debug builds only. Release builds have no console, so the only
  ways out are the tray's Quit and `--exit-after`
- Ctrl+Alt+Del cannot be intercepted. Task Manager is always the last resort;
  killing the process makes Windows drop the hooks
- Normal exit and panic both inject an up for every modifier. Preserve that. A
  stuck Win key turns every later keystroke into a hotkey

**A running instance holds `target/*/altoggle.exe` and cargo cannot replace it.**
If a build fails with "Access is denied", look for a running `altoggle` before
assuming anything else. Do not kill one you did not start without asking: the
user may be typing Japanese with it right now.

## Commands

```bash
cargo test                    # 20 tests in core, 5 in settings
cargo clippy --all-targets    # expected to be clean
cargo build --release         # the only build that reflects the real deployment
```

`cargo` may be missing from an already-open shell's PATH even though the
persisted user PATH is correct: the shell predates the rustup install. Prepend
`$env:USERPROFILE\.cargo\bin` or open a new terminal.

Config file: `%APPDATA%\altoggle\config.toml`, written with comments on first
run. Trigger keys are named (`"LeftAlt"`), not numeric, because the eventual
settings dialog wants exactly that list for a dropdown.

## Facts established by measurement

These came from real hardware. Do not re-derive them, and do not "fix" code that
looks wrong against the documentation but right against this list.

- **Alt up arrives as `WM_KEYUP`, not `WM_SYSKEYUP`**, and without the ALTDOWN
  flag. Detect triggers by `vkCode`, never by the message id
- Left and right Alt arrive already split as `0xA4` / `0xA5`. Both have scan code
  `0x38`; only the right one has EXTENDED
- Solo presses measured 30–215ms. Auto-repeat starts around 500ms and then
  repeats every ~31ms. Hence the 400ms default threshold: 300ms would drop real
  presses
- **The threshold is also the escape hatch.** Past it, the up event is not
  blocked, so Windows opens the menu bar exactly as it always did. That is
  intended behaviour, not a leak
- Real keyboards use `dwExtraInfo == 0`, which is what makes the injection tag
  work
- Dummy key `0x07` (undefined VK) was harmless everywhere tested. `VK_F13`–`F24`
  also work
- `VK_IME_ON` / `VK_IME_OFF` work in both MS-IME and Google Japanese Input,
  across Notepad, Explorer, Chrome, VS Code, and Store apps
- **Under en-US the IME keys do nothing**, so there is no layout check. An
  earlier `GetKeyboardLayout` gate was removed: it is unreliable across processes
  (a Notepad started earlier reported en-US while set to Japanese) and it sat in
  the callback
- AltGr's phantom left Ctrl does **not** appear on a plain US layout. It is a
  US-International problem only
- IMM32 read-back returns nothing for some UWP and Electron apps. That is not a
  failure; judge IME switching by whether Japanese actually types

## Conventions

- **Everything generated is in English**: code, comments, doc comments, console
  output, commit messages
- Commit subjects are Conventional Commits **without a scope**: `feat: ...`, not
  `feat(tray): ...`. The body contains nothing but the `Co-Authored-By` trailer
- Comments explain why, not what. The measured numbers above belong next to the
  code that depends on them
- Line endings are LF, enforced by `.gitattributes`

## Where things are heading

The one open question with a structural consequence: **the settings file is a
placeholder for a settings dialog.** The path from a changed value to the running
state machine is already `PostThreadMessage` → message loop →
`Machine::set_config`, so a dialog replaces the UI and nothing else. Keep it that
way.

Also undecided, none of them load-bearing yet: showing IME state on the tray
icon, an exclusion list for games and remote-desktop clients, adding the Win key
as a trigger (which needs the suppression step turned into a strategy, since
dummy-key injection only addresses the menu bar), and the menu-bar underline
VS Code draws on Alt down, which option A cannot suppress because it passes Alt
down through untouched.
