# AGENTS.md

Guidance for agents working in this repository. It covers the things that are
costly to rediscover, not the things the code already says.

## What this is

A Windows resident app that switches the IME from a *solo* press of a modifier
key. Right Alt alone turns the IME on, left Alt alone turns it off, and Alt with
anything else stays ordinary Alt. Personal tool, unelevated, one user.

Either trigger can be reassigned to any of Alt, Ctrl, Shift, or Win (left or
right), or the context-menu key. Whichever key you pick gives up its usual
solo-press behaviour in exchange: Alt the menu bar, Win the start menu, Menu the
context menu.

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

`altprobe` and `imeprobe` share a command line (`crates/app/src/probe_args.rs`):
`--left` / `--right` / `--dummy` / `--threshold` / `--secs` / `--dry-run`.
**Malformed arguments are rejected, never ignored.** An earlier positional parser
silently discarded every flag it did not understand, which cost a full afternoon
of measurement: the probe was watching Alt while the log was being read as
evidence about Win. For the same reason the header prints `built from:` (the
source tree the binary was compiled in) and the scan code and extendedness of
every key involved. `--dry-run` prints that header and installs no hook — use it
to confirm which build you are about to arm.

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
cargo test                    # 43 tests: 20 in core, 18 in settings, the rest in app
cargo clippy --all-targets    # expected to be clean
cargo build --release         # the only build that reflects the real deployment
```

`cargo fmt` is **not** clean on this tree and running it would rewrite files
nobody touched. Format new files individually with `rustfmt` instead.

`cargo` may be missing from an already-open shell's PATH even though the
persisted user PATH is correct: the shell predates the rustup install. Prepend
`$env:USERPROFILE\.cargo\bin` or open a new terminal.

Config file: `%APPDATA%\altoggle\config.toml`. **Regenerated whole** by
`settings::render` every time the dialog saves, so the comments survive (they
live in the code) and anything the user added to the file does not. Hand edits
still work but only take effect at the next start; there is no reload command.
Trigger keys are named (`"LeftAlt"`), not numeric, because the dialog wants
exactly that list for a dropdown.

## The settings dialog

`crates/app/src/dialog.rs`, raw Win32, no dialog template and no GUI crate.
Modeless and pumped by the main loop, so the hooks stay live while it is open —
that is the point: change the threshold, press Apply, press the key, feel
whether it is right. Modal would have needed a nested loop, which would starve
`session::run`'s `after_message` and freeze the tray.

Things that are only true because `DefWindowProcW` is not `DefDlgProc`, and
that break silently if removed:

- `session::run` calls `dialog::pre_translate` **before** `TranslateMessage`.
  Without it there is no Tab, no Esc, no Enter, no mnemonics
- **`DM_GETDEFID` is handled by hand.** Left to the fallback, `IsDialogMessageW`
  sends `IDOK` on Enter *without checking whether that button is enabled*, so
  Enter would commit settings the greyed-out OK refuses
- `WM_CTLCOLORSTATIC` is handled, or every label sits on a white box
- `WM_DESTROY` must not `PostQuitMessage`: the window shares the process's main
  loop
- The DPI comes from `GetDpiForMonitor` on the *target* monitor, not from
  `GetDpiForWindow`. The window is created at the origin, so the latter reports
  whichever display is there. `WM_DPICHANGED` is ignored until `open` has
  finished, because the move it reports is the one `open` just made at a size it
  already computed

Validation lives in `settings::Problem`, shared with `probe_args` so the dialog
and the command line cannot disagree. Blocking problems grey out OK; warnings
(a threshold outside 250-500ms, an unmeasured dummy) only show a line, because
the probes exist to measure odd values. Picking a key already used by the other
dropdown swaps the two rather than producing a state that then gets refused.

An application manifest is embedded by `crates/app/build.rs` (`embed-manifest`),
for common controls v6 and per-monitor v2 DPI. **It applies to all four binaries
and changes DPI awareness process-wide**, the tray icon included — a manifest is
per crate, not per target.

## Facts established by measurement

These came from real hardware. Do not re-derive them, and do not "fix" code that
looks wrong against the documentation but right against this list.

- **Alt up arrives as `WM_KEYUP`, not `WM_SYSKEYUP`**, and without the ALTDOWN
  flag. Detect triggers by `vkCode`, never by the message id
- Left and right Alt arrive already split as `0xA4` / `0xA5`. Both have scan code
  `0x38`; only the right one has EXTENDED
- Solo Alt presses measured 30–215ms. **Solo Win presses measured 82–257ms**, so
  the Win key is held noticeably longer. Auto-repeat starts around 500ms (512ms
  for Alt, 514ms for Win) and then repeats every ~31ms. Hence the 400ms default
  threshold: 300ms would drop real presses, and with Win in play there is no room
  to lower it
- **Win arrives as `0x5B` / `0x5C`, scan code `0x5B` / `0x5C`, and *both* are
  EXTENDED** (unlike Alt, where only the right one is). Right Ctrl is extended
  too. `key_input` derives the flag from `MAPVK_VK_TO_VSC_EX` rather than from a
  hand-written table, because a Win up injected without `KEYEVENTF_EXTENDEDKEY`
  leaves the key stuck down
- **Option A suppresses the start menu exactly as it suppresses the menu bar.**
  Nothing about the Win key needed special handling: not a dummy with a real scan
  code, not holding the dummy down across the trigger up, not
  `KEYEVENTF_SCANCODE`. All three were built and measured; none was necessary
- **The context menu is raised by the Menu key's up, not its down.** Confirmed by
  holding the key with nothing installed: the menu waits for the release
- **The dummy does nothing for the context menu**, and the injected up is what
  opens it. Alt and Win are tracked by Windows as "pressed alone", and the dummy
  is what breaks that tracking. `VK_APPS` has no such state: `DefWindowProc`
  turns *any* `WM_KEYUP` for it into `WM_CONTEXTMENU`, so there is nothing to
  break and our own replacement up performs the side effect. Hence
  `inject::Suppression` has exactly two shapes, `DummyThenUp` and `Swallow`, split
  by that mechanism and not by a per-key table
- **`Swallow` suppresses the context menu**: blocking the up and injecting no
  replacement leaves nothing to raise it, and holding past the threshold still
  gets the menu, so the escape hatch behaves like every other trigger's
- Withholding the up is safe for `VK_APPS` **only because it is not a modifier**.
  Left logically down it changes nothing; a withheld Alt up would turn every
  later keystroke into a chord. For the same reason `VK_APPS` must stay out of
  `inject::RELEASED_ON_FAILURE`: sending its up is precisely what opens the
  context menu, prior down or not, so listing it would pop a menu on every exit
  and every panic. A test in `settings` ties the release list to
  `suppression_for` in both directions
- The default dummy `0x07` is injected with `wScan` zero, because an undefined
  virtual key has no scan code. That is fine, and is not what decides whether
  suppression works
- **`VK_APPS` (`0x5D`, the context-menu key) is a trigger without being a
  modifier**, the only such case, and it is extended. `foreign_modifier_held`
  correctly does not list it: a held Menu key is not what "another modifier was
  already down" means
- **The threshold is also the escape hatch.** Past it, the up event is not
  blocked, so Windows opens the menu bar (or the start menu) exactly as it always
  did. That is intended behaviour, not a leak
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

**One loose end, deliberately not chased:** PowerShell *may* have shown its
context menu on a swallowed Menu press. The observer was unsure and it was not
worth the time; every other app tested was clean. Recorded only so that someone
seeing it later knows it is not new. A console host reading raw input rather than
going through `DefWindowProc` would be the thing to suspect.

## Conventions

- **Everything generated is in English**: code, comments, doc comments, console
  output, commit messages
- Commit subjects are Conventional Commits **without a scope**: `feat: ...`, not
  `feat(tray): ...`. The body contains nothing but the `Co-Authored-By` trailer
- Comments explain why, not what. The measured numbers above belong next to the
  code that depends on them
- Line endings are LF, enforced by `.gitattributes`

## Where things are heading

The one open question with a structural consequence is now closed: the settings
dialog exists, and it changed the UI and nothing else. **A changed value still
reaches the state machine only by `PostThreadMessage` → message loop →
`Machine::set_config`.** The dialog cannot reach the hook thread at all — its
window procedure leaves committed settings on an outbox that `main` drains into
`HookThread::set_config`, in the same place it drains the tray. Keep it that way.

Also undecided, none of them load-bearing yet: showing IME state on the tray
icon, an exclusion list for games and remote-desktop clients, a dark-mode title
bar and controls for the dialog, and the menu-bar underline VS Code draws on Alt
down, which option A cannot suppress because it passes Alt down through
untouched.
