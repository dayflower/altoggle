# DEVELOP.md

The per-area reference [AGENTS.md](../AGENTS.md) defers to. Read the section for
the area you are about to touch; nothing here needs to be in context otherwise.

AGENTS.md holds what applies wherever you are working — the layout, the hook
callback rules, how to run this safely, the commands, the conventions, and a
digest of the measured facts. This file holds the rest, in full.

## The probes

`altprobe` and `imeprobe` share a command line (`crates/app/src/probe_args.rs`):
`--left` / `--right` / `--dummy` / `--threshold` / `--secs` / `--dry-run`.
**`--split` is `imeprobe` only** — it picks how the injection is batched, which
`altprobe` has no code for, so `altprobe --split` is refused rather than accepted
and ignored. Which probe accepts what is spelled out in the `Probe` descriptor
(`ALTPROBE` / `IMEPROBE`), which also carries the name each prints and its default
`--secs`; the usage line is generated from it, so it cannot advertise a flag the
parser would reject.
**Malformed arguments are rejected, never ignored.** An earlier positional parser
silently discarded every flag it did not understand, which cost a full afternoon
of measurement: the probe was watching Alt while the log was being read as
evidence about Win. For the same reason the header prints `built from:` (the
source tree the binary was compiled in) and the scan code and extendedness of
every key involved. `--dry-run` prints that header and installs no hook — use it
to confirm which build you are about to arm.

The probes are kept because they isolate one layer each. When something breaks,
they tell you *which* layer.

What they do **not** isolate is our own wiring. Both hooks, the callback behind
them, and the state machine dispatch come from `lowlevel.rs` and `dispatch.rs`,
shared with the app; each probe supplies only its `Callbacks` — for `altprobe`
the suppression and nothing else, which is why nothing in its `on_fire` may
touch `ime`. Sharing a copy of the wiring would have proved less, not more: what
a probe answers is a question about Windows, and every layer it shares with the
app is a layer it is genuinely testing. Each keeps its own message loop, because
`hook.rs` pumps messages (config, reinstall, the foreground WinEvent) the probes
have no equivalent of. Their two escape hatches — Ctrl+C and `--secs` — are
`probe_exit::arm`.

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

The IME display is a checkbox rather than a fourth label-and-field row: it is a
yes/no about what the tray shows, not a value to pick. It sits above the
"Advanced" group because it is an ordinary preference, and applying it takes
effect at once — `main` retunes `session::set_tick` and the tray from the same
outbox it already drains.

Validation lives in `settings::Problem`, shared with `probe_args` so the dialog
and the command line cannot disagree. **It reports only what cannot work**, and
an unusual value is not that: a threshold outside 250-500ms and an untried dummy
both run. The measured bands are in the config file's comments, and the probes
exist to measure odd values — a front end that also nagged would be arguing with
the one number a user came to tune. A problem greys out OK and shows one line,
held under `settings::MESSAGE_BUDGET` because the label clips rather than wraps.
Picking a key already used by the other dropdown swaps the two rather than
producing a state that then gets refused — but two `(none)`s are not a clash and
must not swap.

## The config file

`%APPDATA%\altoggle\config.toml` is **regenerated whole** by `settings::render`
every time the dialog saves, so the comments survive (they live in the code) and
anything the user added to the file does not. Hand edits still work but only take
effect at the next start; there is no reload command. Trigger keys are named
(`"LeftAlt"`), not numeric, because the dialog wants exactly that list for a
dropdown.

Trigger names live in the `triggers!` table at the top of `settings.rs` and
nowhere else. That one table generates the `TriggerKey` enum, `ALL`, `vk` and
`name`, so adding a trigger is a single row and there is no list left to forget.
The config file reads the names through the hand-written `settings::trigger_slot`
serde module rather than a derive on the enum, so the file format cannot grow a
second copy of every name and drift from what the dialog and the probes show.

The dummy key is hex everywhere — dialog, log, `--dummy`, and `config.toml`
(`dummy_vk = 0x07`; TOML reads hex integers, and an older file's decimal still
parses). It sits in its own "Advanced" group because it is the one value here
that needs a measurement session to choose well.

## Build resources

An application manifest is embedded by `crates/app/build.rs` (`embed-manifest`),
for common controls v6 and per-monitor v2 DPI. **It applies to all four binaries
and changes DPI awareness process-wide**, the tray icon included — a manifest is
per crate, not per target.

The same `build.rs` also compiles `crates/app/app.rc` (`embed-resource`), which
gives the executable its icon and its version resource. Two consequences and one
rule:

- the resource is per crate too, so the probes carry altoggle's icon and claim to
  be altoggle in their file properties
- **the build now needs `rc.exe` from the Windows SDK.** A toolchain with
  `link.exe` but no SDK resource compiler fails here and nowhere else. The
  failure is deliberately not swallowed
- **`app.rc` must never gain a manifest.** `embed-manifest` already supplies
  one, and two `RT_MANIFEST` resources produce an executable Windows refuses to
  start. That is the whole of the rule: other resource types are fine, and the
  `VERSIONINFO` block is one. Note that it and the icon are both id 1, which does
  not collide because resource ids are scoped per type

Without `VERSIONINFO` the executable has no product name, no publisher and no
version at all, which is most of the way to "unknown publisher" on a machine that
has never seen it. Its version is a second copy of the one in `Cargo.toml`,
because `rc.exe` cannot read a manifest; a test in `crates/app/src/lib.rs`
`include_str!`s the `.rc` and asserts the two agree, so a forgotten bump fails
`cargo test` instead of shipping a binary that reports the previous version.

## The tray icon

**The IME display is off by default**, and then the tray simply carries the
application icon, loaded from the executable's own resource. `show_ime_state` in
the config file turns it on, and the settings dialog has a checkbox for it.
Reading the IME means a `SendMessageTimeout` into the foreground application
several times a second for as long as altoggle runs, which is cheap but not free
and not a thing to start doing to somebody who never asked for it. The field
carries `#[serde(default)]` so an older config file means off as well, and a
test pins that: an upgrade must not switch polling on behind the user.

When it is on, six pictures: black or white (to contrast with the taskbar) times
a Latin **a** (IME off), a hiragana **あ** (IME on) and an asterisk (cannot
tell).

`design/*.svg` and `design/appicon.png` are the masters. `crates/icongen` turns
them into `crates/app/assets/*.png` and `appicon.ico`, which are committed and
`include_bytes!`d. Run it by hand after changing the artwork (the command is in
AGENTS.md). It is a workspace member but **not** in `default-members`, so
`cargo build`, `cargo test` and `cargo clippy --all-targets` never touch it.
That is the point: the app embeds finished pixels, and neither it nor the machine
building it should need an SVG rasteriser.

Three things that are only true for a reason:

- **The IME read cannot live in a hook callback.** `ime::read_open_status` is a
  `SendMessageTimeout`, and the only foreground-change signal the app has,
  `hook::foreground_proc`, runs on the hook thread against the 300ms budget. So
  `main`'s `after_message` polls instead, woken by a timer on `session`'s window.
  `session::set_tick` starts and stops that timer, so with the display off the
  loop is not woken at all rather than woken to do nothing. It is deliberately
  order-independent — `main` decides from its settings before `run` has created
  the window, and an earlier version that needed the window first silently did
  nothing
- **`WM_SETTINGCHANGE` never arrives**, so the light/dark theme is polled on the
  same tick. Windows broadcasts it with `"ImmersiveColorSet"`, but `session`'s
  window is parented to `HWND_MESSAGE` and gets no broadcasts. Reading
  `SystemUsesLightTheme` costs a cached-hive lookup and needs no second window
- **An unreadable IME gets its own picture, not the Latin one.** `None` from
  `read_open_status` means the application did not answer at all, which is not
  the same as "off"; the asterisk artwork also drops the direction arrow so the
  three states stay distinguishable at 32 pixels

`Tray::set_state` returns early when nothing changed. That is load-bearing: the
poll runs several times a second and `set_icon` is a `Shell_NotifyIcon`.

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
- IMM32 read-back returns nothing for some Electron apps. That is not a failure;
  judge IME switching by whether Japanese actually types
- **The foreground window is the wrong thing to ask, and it does not decline.**
  It reports "closed" with full confidence, which is worse than silence: there is
  no way to tell it from a genuine off, so the symptom is a stuck "a" rather than
  the asterisk. Two modern app shapes were measured doing this, and
  `ime::input_window` exists to handle both:
  - a **Store app**: `GetForegroundWindow` returns an `ApplicationFrameWindow`
    owned by ApplicationFrameHost.exe, a shell process hosting only the frame.
    The app itself is a `Windows.UI.Core.CoreWindow` child, in its own process.
    Over a minute of toggling the frame said closed on every sample while the
    `CoreWindow` tracked every switch
  - **Windows 11 Notepad** (WinUI 3): one process, but the top-level `Notepad`
    window and the `RichEditD2DPT` control holding the focus do not answer alike.
    Same measurement: the top-level said closed throughout, the focus window
    followed every switch
- **`GetGUIThreadInfo(...).hwndFocus` is the general answer**, and the CoreWindow
  hop is only what gets you into the right process first. Ordinary windows are
  their own focus window's answer, so the same path serves all three cases and
  none of them is a special case in the code

**One loose end, deliberately not chased:** PowerShell *may* have shown its
context menu on a swallowed Menu press. The observer was unsure and it was not
worth the time; every other app tested was clean. Recorded only so that someone
seeing it later knows it is not new. A console host reading raw input rather than
going through `DefWindowProc` would be the thing to suspect.

## Releasing

Portable exe, GitHub Releases, no installer and no code signing. The eventual
destination is a winget or scoop manifest, which is why the zip is named for its
version and target triple and why the SHA256 is published with it: those are the
two things such a manifest asks for.

1. `.\scripts\bump.ps1 <major|minor|patch|X.Y.Z>` — resolves the next version
   from the current one, writes it to the root `Cargo.toml` and to
   `crates/app/app.rc`, runs `cargo test`, commits on `release/v<version>`,
   pushes, and opens the pull request. `-DryRun` runs every check and prints
   every change without touching a file or a branch
2. Merge that pull request
3. `.github/workflows/release.yml` notices the version change on `main`, runs
   the tests, calls `scripts/release.ps1`, and publishes `v<version>` with the
   zip attached and its SHA256 in the notes

**The version number is typed once.** It used to have three homes — the root
`Cargo.toml`, `crates/app/app.rc`, and the tag — and only the first two were
tied together, by the drift test in `crates/app/src/lib.rs` that fails if you
bump one and not the other. The tag was a third copy nobody checked. Now
`bump.ps1` writes the two files and CI derives the tag from `Cargo.toml` at the
commit it is releasing, so a tag disagreeing with the binary is unreachable
rather than caught.

`scripts/release.ps1` stays worth running by hand when you want to see what
would ship. It is the same script CI calls, so a clean local run is the same
packaging — that is the point of CI calling it rather than assembling the zip
itself.

**Only `altoggle.exe` ships.** A `keylog.exe` sitting next to a tray app in a
downloaded zip is an antivirus incident and a trust problem in one. The script
exists to make that rule executable; do not hand-assemble a zip instead.

`crates/app` depends on `altoggle-core` **by path with no `version`**. Nothing is
published, so a version requirement there would be a third copy of the number —
and a stale one refuses to resolve at the exact moment you bump the other two.

`README.md` is the user-facing document and the only one; it deliberately does
not mention the probes. It carries the paragraph explaining why SmartScreen warns
(unsigned, installs a keyboard hook) and what the app actually does about it (no
network, two writes, unelevated). Those claims are load-bearing: if any of them
stops being true, that paragraph changes with it.

Not built, and listed here so each stays a decision rather than an oversight: a
signing certificate, an installer, and arm64. Pre-release versions are refused
by `bump.ps1` rather than unimplemented — `app.rc`'s `FILEVERSION` takes four
numbers and a hyphen is not one of them. The largest gap the current state
leaves is that every diagnostic goes to `OutputDebugStringW` only, so a release
build that refuses to start does so in complete silence — worth fixing the first
time a user reports "nothing happens".

## Continuous integration

Two workflows. `ci.yml` checks the tree on every pull request; `release.yml`
publishes when the version on `main` changes. They share a runner image and
nothing else.

### ci.yml

It runs on every push to `main` and every pull request, and it runs exactly the
four commands in the AGENTS.md command block — fmt, clippy, test, release build
— as four steps of one job, with `-D warnings` added to clippy because otherwise
it reports and still exits zero. One job because compiling the dependency tree
is most of the wall clock on a Windows runner, and this way it happens once;
that order because it fails cheapest-first.

`windows-latest`, and no matrix. `crates/app` pulls `windows-sys`
unconditionally and its `build.rs` needs `rc.exe` from the Windows SDK, so an
ubuntu runner could only ever build `crates/core` — which `cargo test` reaches
anyway. If the job goes red for what looks like an environment reason rather
than a code one, the SDK's resource compiler is the thing to suspect, because
`build.rs` is the only place that wants it.

The toolchain is `stable` and deliberately unpinned: this checks the code
against the compiler people actually have. `rust-version = "1.85"` is a separate
claim, and still an untested one.

**A green run means the code compiles, lints and passes its unit tests, and says
nothing about behaviour on a keyboard.** Nothing in a hosted runner can install
a low-level hook against real input or read a real IME, so everything in [Facts
established by measurement](#facts-established-by-measurement) stays outside
CI's reach — as does the whole reason the probes exist. The app itself is never
launched there, and should not be: it blocks real Alt up events.

Considered and left out, each a decision rather than an oversight: an MSRV job,
a `cargo check -p altoggle-icongen` that would drag an SVG toolchain into CI for
a crate no build touches, and a second job running `crates/core` on ubuntu to
prove it is OS independent.

### release.yml

Triggered by a push to `main` that touches `Cargo.toml`, and then guarded by two
conditions that both have to hold: the version differs from the one at `HEAD^`
(so an ordinary dependency bump is a no-op, and so adding the workflow does not
itself release whatever version is current), and no release exists for it yet
(so a re-run publishes nothing twice). Everything after the guard is skipped by
`if:` rather than by a second job.

It then runs `cargo test` — not a repeat of the pull request's run, because what
is built here is the merge commit, and because the drift test is the claim the
whole scheme rests on — calls `scripts/release.ps1` for the packaging, and hands
the zip to `gh release create --target <sha>`.

**The tag is created by that command, not pushed by a step.** It costs a
lightweight tag rather than an annotated one, and buys back the failure mode
where a pushed tag survives a failed publish. It also sidesteps the rule that a
tag pushed with `GITHUB_TOKEN` starts no further workflow: there is no further
workflow, because this one runs to the published release.

What it guarantees is that the tag, the version resource in the exe, and
`Cargo.toml` agree — all three come from one number that `scripts/bump.ps1`
wrote once. What it does not do is sign anything, build an installer, or make
the exe any less alarming to SmartScreen.

## Where things are heading

The one open question with a structural consequence is now closed: the settings
dialog exists, and it changed the UI and nothing else. **A changed value still
reaches the state machine only by `PostThreadMessage` → message loop →
`Machine::set_config`.** The dialog cannot reach the hook thread at all — its
window procedure leaves committed settings on an outbox that `main` drains into
`HookThread::set_config`, in the same place it drains the tray. Keep it that way.

Also undecided, none of them load-bearing yet: an exclusion list for games and
remote-desktop clients, a dark-mode title bar and controls for the dialog, and
the menu-bar underline VS Code draws on Alt down, which option A cannot suppress
because it passes Alt down through untouched.
