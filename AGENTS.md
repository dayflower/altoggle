# AGENTS.md

Guidance for agents working in this repository. It covers the things that are
costly to rediscover, not the things the code already says.

This file holds what is true whatever you are touching. The reference for a
given area — the dialog, the tray, the probes, the full measurement log — lives
in [notes/DEVELOP.md](notes/DEVELOP.md); the index at the end says which section
to open before which change.

## What this is

A Windows resident app that switches the IME from a *solo* press of a modifier
key. Right Alt alone turns the IME on, left Alt alone turns it off, and Alt with
anything else stays ordinary Alt. Personal tool, unelevated, one user.

Either trigger can be reassigned to any of Alt, Ctrl, Shift, or Win (left or
right), or the context-menu key, and whichever key you pick gives up its usual
solo-press behaviour in exchange: Alt the menu bar, Win the start menu, Menu the
context menu. Either can also be switched off — `None` in the file, `(none)` in
the dialog, `VK_NONE` (zero) in `Config`, which is inert for free because no
event can carry it. Both off at once is allowed and leaves the app doing
nothing.

## Layout

```
crates/core     State machine only. no_std, no windows-sys, tests run on any OS.
crates/app      Win32 adapter, the app, and the verification tools.
crates/icongen  design/*.svg -> the icons crates/app embeds. Run by hand, and
                outside default-members so no ordinary build touches it.
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

The probes isolate one layer each, and share a command line that **rejects
malformed arguments rather than ignoring them**. Flags and rationale: DEVELOP.md.

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
  stuck Win key turns every later keystroke into a hotkey. **This is why
  `[profile.release]` does not set `panic = "abort"`** and must not: aborting
  skips the unwinding that releases them

**A running instance holds `target/*/altoggle.exe` and cargo cannot replace it.**
If a build fails with "Access is denied", look for a running `altoggle` before
assuming anything else. Do not kill one you did not start without asking: the
user may be typing Japanese with it right now.

## Commands

```bash
cargo test                    # 51 tests: 22 in core, 21 in settings, the rest in app
cargo clippy --all-targets    # expected to be clean
cargo build --release         # the only build that reflects the real deployment
cargo run -p altoggle-icongen # by hand, only after changing design/
cargo fmt                     # clean on this tree; keep it that way
```

`cargo` may be missing from an already-open shell's PATH even though the
persisted user PATH is correct: the shell predates the rustup install. Prepend
`$env:USERPROFILE\.cargo\bin` or open a new terminal.

Config file: `%APPDATA%\altoggle\config.toml`, **regenerated whole** by
`settings::render` every time the dialog saves. Hand edits still work but only
take effect at the next start; there is no reload command.

## Measured facts that constrain the code

These came from real hardware. Do not re-derive them, and do not "fix" code that
looks wrong against the documentation but right against them. The ones that
constrain code anywhere:

- **Alt up arrives as `WM_KEYUP`, not `WM_SYSKEYUP`**, and without the ALTDOWN
  flag. Detect triggers by `vkCode`, never by the message id
- **Win is `0x5B` / `0x5C` and *both* are EXTENDED**, unlike Alt where only the
  right one is. `key_input` derives the flag from `MAPVK_VK_TO_VSC_EX` rather
  than a hand-written table, because a Win up injected without
  `KEYEVENTF_EXTENDEDKEY` leaves the key stuck down
- **The 400ms default threshold has no room below it.** Solo Alt presses measured
  30–215ms, solo Win 82–257ms, and auto-repeat starts around 500ms
- **The threshold is also the escape hatch.** Past it the up event is not
  blocked, so Windows opens the menu bar (or the start menu) exactly as it always
  did. Intended behaviour, not a leak
- **`inject::Suppression` has exactly two shapes**, `DummyThenUp` and `Swallow`,
  split by mechanism and not by a per-key table
- **`VK_APPS` is a trigger without being a modifier**, the only such case. It
  must stay out of `inject::RELEASED_ON_FAILURE` — sending its up is precisely
  what opens the context menu — and `foreign_modifier_held` correctly does not
  list it
- **The IME is read from `GetGUIThreadInfo(...).hwndFocus`, never from the
  foreground window**, which reports "closed" with full confidence and is wrong
  for Store apps and Windows 11 Notepad alike
- **Under en-US the IME keys do nothing**, so there is deliberately no layout
  check. An earlier `GetKeyboardLayout` gate was removed
- IMM32 read-back returns nothing for some Electron apps. That is not a failure;
  judge IME switching by whether Japanese actually types

The full log — every measurement, with what was tried and rejected — is in
[notes/DEVELOP.md](notes/DEVELOP.md#facts-established-by-measurement).

## Invariants to keep

- **A changed value reaches the state machine only by `PostThreadMessage` →
  message loop → `Machine::set_config`.** The dialog cannot reach the hook thread
  at all: its window procedure leaves committed settings on an outbox that `main`
  drains into `HookThread::set_config`, in the same place it drains the tray
- **`app.rc` must never gain a manifest.** `crates/app/build.rs` already embeds
  one through `embed-manifest`, and two `RT_MANIFEST` resources produce an
  executable Windows refuses to start. Other resource types are fine — the
  `VERSIONINFO` block beside the icon is one
- **`[profile.release]` must not set `panic = "abort"`**, for the reason above:
  the unwinding is what releases the modifiers
- **Only `altoggle.exe` is ever distributed.** `cargo build --release` also
  produces `keylog`, `altprobe` and `imeprobe`; `scripts/release.ps1` builds and
  packages the one binary so that rule is executable rather than remembered

## Conventions

- **Everything generated is in English**: code, comments, doc comments, console
  output, commit messages
- Commit subjects are Conventional Commits **without a scope**: `feat: ...`, not
  `feat(tray): ...`. The body contains nothing but the `Co-Authored-By` trailer
- Comments explain why, not what. Measured numbers belong next to the code that
  depends on them
- Line endings are LF, enforced by `.gitattributes`

## Where to look before you touch it

Sections of [notes/DEVELOP.md](notes/DEVELOP.md):

| Before you touch | Read |
|------------------|------|
| `dialog.rs`, `settings.rs` | [The settings dialog](notes/DEVELOP.md#the-settings-dialog) |
| `tray.rs`, `icons.rs`, `ime.rs`, `design/` | [The tray icon](notes/DEVELOP.md#the-tray-icon) |
| `hook.rs`, `inject.rs`, `keys.rs` | [Facts established by measurement](notes/DEVELOP.md#facts-established-by-measurement) |
| `build.rs`, `app.rc` | [Build resources](notes/DEVELOP.md#build-resources) |
| `bin/*.rs`, `probe_args.rs` | [The probes](notes/DEVELOP.md#the-probes) |
| `settings::render`, `config.toml` | [The config file](notes/DEVELOP.md#the-config-file) |
| the version, `README.md`, `scripts/` | [Releasing](notes/DEVELOP.md#releasing) |
| what is still undecided | [Where things are heading](notes/DEVELOP.md#where-things-are-heading) |
