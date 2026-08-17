//! The settings window.
//!
//! Hand-built rather than loaded from a dialog template, so that the layout,
//! the fonts and the DPI handling are all in one file and none of it needs a
//! resource compiler. The cost is that `DefWindowProcW` is not `DefDlgProc`:
//! the background colour, the default button and the Esc key all have to be
//! arranged by hand. Each of those is commented where it happens.
//!
//! Modeless, and pumped by the process's main loop through `pre_translate`.
//! Modal would have meant a nested message loop, which would starve
//! `session::run`'s `after_message` and so freeze the tray menu for as long as
//! the window was open. Being modeless also means the hooks stay live while the
//! window is up, which is the point: you change the threshold, press Apply, and
//! press the key to feel whether it is right.
//!
//! Nothing here touches the state machine. Committed settings go on an outbox
//! that `main` drains into `HookThread::set_config`, which stays the only way
//! in.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_3DFACE, CreateFontIndirectW, DeleteObject, GetMonitorInfoW, GetSysColorBrush, HDC, HFONT,
    HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, SetBkMode, SetTextColor,
    TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    EM_SETLIMITTEXT, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, WC_BUTTONW,
    WC_COMBOBOXW, WC_EDITW, WC_STATICW,
};
use windows_sys::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI,
    SystemParametersInfoForDpi,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, BS_GROUPBOX, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBN_SELCHANGE,
    CBS_DROPDOWNLIST, CBS_HASSTRINGS, CreateWindowExW, DC_HASDEFID, DM_GETDEFID, DefWindowProcW,
    DestroyWindow, EN_CHANGE, ES_AUTOHSCROLL, ES_NUMBER, ES_UPPERCASE, GetCursorPos, GetDlgItem,
    GetDlgItemInt, GetDlgItemTextW, IDC_ARROW, IDCANCEL, IDOK, IsChild, IsDialogMessageW, IsIconic,
    LoadCursorW, MB_ICONERROR, MB_OK, MSG, MessageBoxW, MoveWindow, NONCLIENTMETRICSW,
    RegisterClassExW, SPI_GETNONCLIENTMETRICS, SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOZORDER, SendDlgItemMessageW, SendMessageW, SetDlgItemInt, SetDlgItemTextW,
    SetForegroundWindow, SetWindowPos, ShowWindow, SystemParametersInfoW, USER_DEFAULT_SCREEN_DPI,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CTLCOLORSTATIC, WM_DESTROY,
    WM_DPICHANGED, WM_NCDESTROY, WM_SETFONT, WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE,
    WS_EX_CONTROLPARENT, WS_EX_DLGMODALFRAME, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
    WS_VSCROLL,
};

use crate::settings::{Settings, TriggerKey};
use crate::{log, settings, wide};

// Control ids. Every control has its own, including the labels, because the
// layout pass finds them again with `GetDlgItem`.
const IDC_LEFT: i32 = 100;
const IDC_RIGHT: i32 = 101;
const IDC_THRESHOLD: i32 = 102;
const IDC_DUMMY: i32 = 103;
const IDC_HINT: i32 = 105;
const IDC_APPLY: i32 = 106;
const IDC_GROUP: i32 = 199;
const IDC_LABEL_LEFT: i32 = 200;
const IDC_LABEL_RIGHT: i32 = 201;
const IDC_LABEL_THRESHOLD: i32 = 202;
const IDC_LABEL_DUMMY: i32 = 203;
const IDC_DUMMY_PREFIX: i32 = 204;
const IDC_THRESHOLD_UNIT: i32 = 205;

/// The first row of both dropdowns: leave this direction alone.
const NONE_LABEL: &str = "(none)";

/// `WS_EX_CONTROLPARENT` is what `GetNextDlgTabItem` looks for when deciding
/// whether to walk into a child, and is the flag the dialog manager sets on a
/// real dialog. `WS_EX_DLGMODALFRAME` is only the border.
const EX_STYLE: WINDOW_EX_STYLE = WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT;

/// Overlapped rather than `WS_POPUP`: the window has no owner, so it should get
/// a taskbar button and an Alt+Tab entry. Clicking away from a tray app's
/// settings and then needing to find them again is otherwise a dead end.
/// Fixed size, so no `WS_THICKFRAME` and no min/max boxes.
const STYLE: WINDOW_STYLE = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;

thread_local! {
    /// The one open window, or null. Main-thread only: the window is created,
    /// pumped and destroyed on the thread that runs `session::run`, and `HWND`
    /// is not `Send`. `hook.rs` uses atomics only because other threads post
    /// to it; nothing here crosses a thread.
    static WINDOW: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
    /// Whether the hint is showing a problem, and so should be red.
    ///
    /// Deliberately not inside `STATE`: it is read from `WM_CTLCOLORSTATIC`,
    /// which arrives during a paint that can be provoked while `STATE` is
    /// borrowed.
    static HINT_IS_BLOCKING: Cell<bool> = const { Cell::new(false) };
    static STATE: RefCell<Option<DialogState>> = const { RefCell::new(None) };
    /// Settings the user committed, waiting for `main` to pick them up.
    static OUTBOX: RefCell<Vec<Settings>> = const { RefCell::new(Vec::new()) };
}

struct DialogState {
    /// What the hook thread was last told, for the dirty check behind Apply.
    applied: Settings,
    /// What the two dropdowns showed before the change being handled. Needed to
    /// swap them: the key one dropdown gave up is the key the other takes.
    triggers: (Option<TriggerKey>, Option<TriggerKey>),
    /// The message font, owned by this window.
    font: HFONT,
}

impl Drop for DialogState {
    fn drop(&mut self) {
        if !self.font.is_null() {
            unsafe { DeleteObject(self.font as HGDIOBJ) };
        }
    }
}

/// Run `f` with the dialog state.
///
/// Does nothing when there is no dialog, and — deliberately — when the state is
/// already borrowed. A window procedure can be re-entered by any `SendMessage`
/// Windows makes on our behalf, and a `RefCell` panic unwinding out of an
/// `extern "system"` function would take the process down with the hooks
/// installed. Dropping one update is much the cheaper failure.
fn with_state(f: impl FnOnce(&mut DialogState)) {
    STATE.with(|cell| match cell.try_borrow_mut() {
        Ok(mut slot) => {
            if let Some(state) = slot.as_mut() {
                f(state);
            }
        }
        Err(_) => log::line("settings dialog re-entered while updating; update skipped"),
    });
}

// ---------------------------------------------------------------------------
// Layout
//
// Sizes are in dialog-independent pixels at 96 dpi and scaled by `dip`.
// ---------------------------------------------------------------------------

const MARGIN: i32 = 11;
const LABEL_W: i32 = 104;
const LABEL_H: i32 = 16;
/// Labels sit a little lower than the control beside them, or the text rides
/// high against the field's border.
const LABEL_DROP: i32 = 4;
const FIELD_X: i32 = 119;
const COMBO_W: i32 = 150;
const EDIT_W: i32 = 60;
/// Two hex digits and no more.
const DUMMY_W: i32 = 42;
const ROW_H: i32 = 23;
const ROW_STEP: i32 = 29;
const ROW0: i32 = 11;
/// The height passed for a `CBS_DROPDOWNLIST` is the height **with the list
/// dropped down**; the closed height comes from the font. Sized for the nine
/// triggers plus "(none)", so the list never needs scrolling.
const COMBO_H: i32 = ROW_H + 10 * 18;
const CLIENT_W: i32 = 280;
const CONTENT_RIGHT: i32 = CLIENT_W - MARGIN;

/// The dummy key lives in its own group, below the three settings anybody
/// actually changes. It is the one value here that needs a measurement session
/// to choose well, and putting it in line with the trigger dropdowns invited
/// the reading that it is an ordinary choice.
///
/// The gap above it is larger than a row step on purpose: the group has to read
/// as a separate part of the window rather than as a fourth row that happens to
/// have a box drawn round it.
const GROUP_Y: i32 = row(2) + ROW_H + 16;
const GROUP_H: i32 = 53;
/// Inset from the group box's own edge to its contents.
const GROUP_PAD: i32 = 9;
/// First row inside the group, clear of its caption.
const GROUP_ROW: i32 = GROUP_Y + 20;

/// Two lines, which `settings::MESSAGE_BUDGET` is sized to fit.
///
/// The space is reserved whether or not there is anything to say, so that a
/// message appearing never shoves the buttons down under the pointer.
const HINT_Y: i32 = GROUP_Y + GROUP_H + 10;
const HINT_H: i32 = 32;
const BTN_Y: i32 = HINT_Y + HINT_H + 10;
const BTN_W: i32 = 80;
const BTN_H: i32 = 25;
const BTN_GAP: i32 = 7;
const CLIENT_H: i32 = BTN_Y + BTN_H + MARGIN;

const fn row(n: i32) -> i32 {
    ROW0 + n * ROW_STEP
}

/// Buttons are right-aligned, so they are placed from the right edge inwards.
const fn button_x(from_right: i32) -> i32 {
    CONTENT_RIGHT - (from_right + 1) * BTN_W - from_right * BTN_GAP
}

fn dip(v: i32, dpi: u32) -> i32 {
    v * dpi as i32 / USER_DEFAULT_SCREEN_DPI as i32
}

#[derive(Clone, Copy)]
enum Class {
    Static,
    Combo,
    Edit,
    Button,
}

impl Class {
    fn name(self) -> *const u16 {
        match self {
            Class::Static => WC_STATICW,
            Class::Combo => WC_COMBOBOXW,
            Class::Edit => WC_EDITW,
            Class::Button => WC_BUTTONW,
        }
    }
}

struct Spec {
    id: i32,
    class: Class,
    text: &'static str,
    /// On top of `WS_CHILD | WS_VISIBLE`.
    style: WINDOW_STYLE,
    ex_style: WINDOW_EX_STYLE,
    /// x, y, width, height in DIPs.
    rect: (i32, i32, i32, i32),
}

/// Every control, **in creation order**.
///
/// Creation order is z-order, and z-order is the order `IsDialogMessageW` walks
/// for Tab, so this table is the tab order. `WS_TABSTOP` goes on the seven
/// controls that take focus and on nothing else; a label with a tab stop is a
/// dead stop the user has to press Tab through twice.
const CONTROLS: &[Spec] = &[
    Spec {
        id: IDC_LABEL_LEFT,
        class: Class::Static,
        text: "Turn IME &off:",
        style: 0,
        ex_style: 0,
        rect: (MARGIN, row(0) + LABEL_DROP, LABEL_W, LABEL_H),
    },
    Spec {
        id: IDC_LEFT,
        class: Class::Combo,
        text: "",
        style: WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32 | CBS_HASSTRINGS as u32,
        ex_style: 0,
        rect: (FIELD_X, row(0), COMBO_W, COMBO_H),
    },
    Spec {
        id: IDC_LABEL_RIGHT,
        class: Class::Static,
        text: "Turn IME o&n:",
        style: 0,
        ex_style: 0,
        rect: (MARGIN, row(1) + LABEL_DROP, LABEL_W, LABEL_H),
    },
    Spec {
        id: IDC_RIGHT,
        class: Class::Combo,
        text: "",
        style: WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32 | CBS_HASSTRINGS as u32,
        ex_style: 0,
        rect: (FIELD_X, row(1), COMBO_W, COMBO_H),
    },
    Spec {
        id: IDC_LABEL_THRESHOLD,
        class: Class::Static,
        text: "&Threshold:",
        style: 0,
        ex_style: 0,
        rect: (MARGIN, row(2) + LABEL_DROP, LABEL_W, LABEL_H),
    },
    Spec {
        id: IDC_THRESHOLD,
        class: Class::Edit,
        text: "",
        // ES_NUMBER refuses everything but digits, including a pasted minus
        // sign, which is most of this field's validation done before the fact.
        style: WS_TABSTOP | ES_NUMBER as u32 | ES_AUTOHSCROLL as u32,
        ex_style: WS_EX_CLIENTEDGE,
        rect: (FIELD_X, row(2), EDIT_W, ROW_H),
    },
    Spec {
        // The unit rides beside the field rather than inside the label, so it
        // reads with the number the way the dummy key's "0x" does.
        id: IDC_THRESHOLD_UNIT,
        class: Class::Static,
        text: "ms",
        style: 0,
        ex_style: 0,
        rect: (FIELD_X + EDIT_W + 6, row(2) + LABEL_DROP, 30, LABEL_H),
    },
    Spec {
        // A BUTTON with BS_GROUPBOX is Win32's group box; there is no separate
        // class. It draws a frame and a caption and takes no input, so it needs
        // no tab stop and never appears in a WM_COMMAND.
        id: IDC_GROUP,
        class: Class::Button,
        text: "Advanced",
        style: BS_GROUPBOX as u32,
        ex_style: 0,
        rect: (MARGIN, GROUP_Y, CONTENT_RIGHT - MARGIN, GROUP_H),
    },
    Spec {
        id: IDC_LABEL_DUMMY,
        class: Class::Static,
        text: "&Dummy key:",
        style: 0,
        ex_style: 0,
        rect: (
            MARGIN + GROUP_PAD,
            GROUP_ROW + LABEL_DROP,
            LABEL_W - GROUP_PAD,
            LABEL_H,
        ),
    },
    Spec {
        // The "0x" is a label rather than part of the field, so the field can
        // hold exactly the two digits it accepts and the base is never in doubt.
        id: IDC_DUMMY_PREFIX,
        class: Class::Static,
        text: "0x",
        style: 0,
        ex_style: 0,
        rect: (FIELD_X, GROUP_ROW + LABEL_DROP, 16, LABEL_H),
    },
    Spec {
        // Hex, not ES_NUMBER: the config file, the log and the probes' --dummy
        // flag all speak hex, and a field that took decimal here would be the
        // only place that did. Two characters is also exactly the virtual-key
        // range, so the limit does the range check.
        id: IDC_DUMMY,
        class: Class::Edit,
        text: "",
        style: WS_TABSTOP | ES_UPPERCASE as u32 | ES_AUTOHSCROLL as u32,
        ex_style: WS_EX_CLIENTEDGE,
        rect: (FIELD_X + 18, GROUP_ROW, DUMMY_W, ROW_H),
    },
    Spec {
        id: IDC_HINT,
        class: Class::Static,
        text: "",
        style: 0,
        ex_style: 0,
        rect: (MARGIN, HINT_Y, CONTENT_RIGHT - MARGIN, HINT_H),
    },
    Spec {
        // BS_DEFPUSHBUTTON draws the default ring. What makes Enter reach it is
        // the DM_GETDEFID handler in `wnd_proc`.
        id: IDOK,
        class: Class::Button,
        text: "OK",
        style: WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        ex_style: 0,
        rect: (button_x(2), BTN_Y, BTN_W, BTN_H),
    },
    Spec {
        // Id `IDCANCEL` is the whole of the Esc handling: `IsDialogMessageW`
        // turns Esc into a WM_COMMAND for it.
        id: IDCANCEL,
        class: Class::Button,
        text: "Cancel",
        style: WS_TABSTOP,
        ex_style: 0,
        rect: (button_x(1), BTN_Y, BTN_W, BTN_H),
    },
    Spec {
        id: IDC_APPLY,
        class: Class::Button,
        text: "&Apply",
        style: WS_TABSTOP,
        ex_style: 0,
        rect: (button_x(0), BTN_Y, BTN_W, BTN_H),
    },
];

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Show the settings window, or bring the existing one forward.
pub fn open(current: Settings) {
    let existing = WINDOW.get();
    if !existing.is_null() {
        unsafe {
            if IsIconic(existing) != 0 {
                ShowWindow(existing, SW_RESTORE);
            }
            // The tray click just gave this process foreground rights, so this
            // succeeds without the usual AttachThreadInput dance.
            SetForegroundWindow(existing);
        }
        return;
    }

    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let title = wide("altoggle settings");
    // Created hidden: the controls, the font and the position all follow, and
    // a visible window would show an empty frame while they do.
    let hwnd = unsafe {
        CreateWindowExW(
            EX_STYLE,
            class_name(),
            title.as_ptr(),
            STYLE,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        log::line("could not create the settings window");
        return;
    }

    create_controls(hwnd, hinstance);
    STATE.set(Some(DialogState {
        applied: current,
        triggers: (current.left_trigger, current.right_trigger),
        font: std::ptr::null_mut(),
    }));

    // The DPI has to come from the monitor we are about to move to, not from
    // `GetDpiForWindow`: the window was created at the origin, so that would
    // report whichever display happens to be there and lay the controls out at
    // the wrong scale.
    let (work, dpi) = target_monitor(hwnd);
    with_state(|state| layout(hwnd, state, dpi));
    place(hwnd, work, dpi);
    // Filling the edits emits EN_CHANGE, which runs `revalidate` and so fills
    // the hint in as a side effect. The state has to exist by now for that.
    fill(hwnd, current);

    WINDOW.set(hwnd);
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetFocus(GetDlgItem(hwnd, IDC_LEFT));
    }
}

/// Destroy the window if it is open.
///
/// Called after the main loop has quit. `request_quit` posts `WM_QUIT`, so
/// `GetMessageW` returns with the window still alive; `DestroyWindow` sends
/// `WM_DESTROY` and `WM_NCDESTROY` synchronously and so does not need a pump.
pub fn close() {
    let hwnd = WINDOW.get();
    if !hwnd.is_null() {
        unsafe { DestroyWindow(hwnd) };
    }
}

/// Settings the user committed since the last call, in order.
///
/// The window procedure cannot reach the `HookThread` — it is `extern "system"`
/// and `main` owns and finally consumes the handle — so committed values are
/// left here and `main` drains them in the same place it drains the tray.
pub fn poll() -> Vec<Settings> {
    OUTBOX.with_borrow_mut(std::mem::take)
}

/// Give `IsDialogMessageW` first refusal on a message. `true` means handled.
///
/// This is what makes Tab, Shift+Tab, Esc, Enter and Alt+mnemonics work on a
/// window that is not of the dialog class.
///
/// Alt+mnemonics are worth a word: Alt+O is not a solo Alt press. The state
/// machine sees Alt down then O down, marks the press contaminated, and lets
/// the Alt up through untouched — so the mnemonic works and the IME does not
/// move. A *solo* Alt press with this window open still switches the IME, which
/// is exactly why the window is modeless.
pub fn pre_translate(msg: &MSG) -> bool {
    let hwnd = WINDOW.get();
    if hwnd.is_null() {
        return false;
    }
    // The tray's own windows share this loop, and IsDialogMessage must not be
    // handed a message from outside the dialog's tree.
    if msg.hwnd != hwnd && unsafe { IsChild(hwnd, msg.hwnd) } == 0 {
        return false;
    }
    unsafe { IsDialogMessageW(hwnd, msg) != 0 }
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

fn class_name() -> *const u16 {
    static CLASS: OnceLock<Vec<u16>> = OnceLock::new();
    CLASS
        .get_or_init(|| {
            // Under the manifest this loads version 6 of the common controls in
            // the right activation context, which is what themes them.
            let icc = INITCOMMONCONTROLSEX {
                dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_STANDARD_CLASSES,
            };
            unsafe { InitCommonControlsEx(&icc) };

            let name = wide("altoggle-settings");
            let mut class: WNDCLASSEXW = unsafe { std::mem::zeroed() };
            class.cbSize = size_of::<WNDCLASSEXW>() as u32;
            class.lpfnWndProc = Some(wnd_proc);
            class.hInstance = unsafe { GetModuleHandleW(std::ptr::null()) };
            class.hCursor = unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) };
            // Without this the window erases to COLOR_WINDOW (white) and does
            // not read as a dialog. The brush is system-owned: never delete it.
            class.hbrBackground = unsafe { GetSysColorBrush(COLOR_3DFACE) };
            class.lpszClassName = name.as_ptr();
            unsafe { RegisterClassExW(&class) };
            name
        })
        .as_ptr()
}

fn create_controls(hwnd: HWND, hinstance: *mut std::ffi::c_void) {
    for spec in CONTROLS {
        let text = wide(spec.text);
        // Position and size are set by `layout`, which also runs on a DPI
        // change; there is no point computing them twice.
        let child = unsafe {
            CreateWindowExW(
                spec.ex_style,
                spec.class.name(),
                text.as_ptr(),
                WS_CHILD | WS_VISIBLE | spec.style,
                0,
                0,
                0,
                0,
                hwnd,
                spec.id as isize as *mut std::ffi::c_void,
                hinstance,
                std::ptr::null(),
            )
        };
        if child.is_null() {
            log::line(format!("settings dialog: control {} failed", spec.id));
        }
    }
    for id in [IDC_LEFT, IDC_RIGHT] {
        // "(none)" first, so leaving a direction alone is as reachable as any
        // key. The parentheses mark it as not-a-key; the config file spells it
        // `None`, and nothing couples the two because the list is read by index.
        for label in std::iter::once(NONE_LABEL).chain(TriggerKey::ALL.iter().map(|k| k.name())) {
            let text = wide(label);
            unsafe {
                SendDlgItemMessageW(hwnd, id, CB_ADDSTRING, 0, text.as_ptr() as LPARAM);
            }
        }
    }
    // Five digits covers any threshold worth typing. Two hex digits is exactly
    // the virtual-key range, so the limit is also the range check.
    unsafe {
        SendDlgItemMessageW(hwnd, IDC_THRESHOLD, EM_SETLIMITTEXT, 5, 0);
        SendDlgItemMessageW(hwnd, IDC_DUMMY, EM_SETLIMITTEXT, 2, 0);
    }
}

/// Put `settings` into the controls.
fn fill(hwnd: HWND, settings: Settings) {
    set_combo(hwnd, IDC_LEFT, settings.left_trigger);
    set_combo(hwnd, IDC_RIGHT, settings.right_trigger);
    unsafe { SetDlgItemInt(hwnd, IDC_THRESHOLD, settings.threshold_ms as u32, 0) };
    set_text(hwnd, IDC_DUMMY, &format!("{:02X}", settings.dummy_vk));
}

/// Build the font for `dpi`, move every control, and retire the old font.
fn layout(hwnd: HWND, state: &mut DialogState, dpi: u32) {
    let font = message_font(dpi);
    for spec in CONTROLS {
        let ctl = unsafe { GetDlgItem(hwnd, spec.id) };
        if ctl.is_null() {
            continue;
        }
        let (x, y, w, h) = spec.rect;
        unsafe {
            SendMessageW(ctl, WM_SETFONT, font as WPARAM, 1);
            MoveWindow(ctl, dip(x, dpi), dip(y, dpi), dip(w, dpi), dip(h, dpi), 1);
        }
    }
    // Only now that no control still has it selected. Deleting a font in use is
    // the classic way to lose text on the next repaint.
    let old = std::mem::replace(&mut state.font, font);
    if !old.is_null() {
        unsafe { DeleteObject(old as HGDIOBJ) };
    }
}

/// The shell's message font, already scaled for `dpi`.
fn message_font(dpi: u32) -> HFONT {
    let mut ncm: NONCLIENTMETRICSW = unsafe { std::mem::zeroed() };
    ncm.cbSize = size_of::<NONCLIENTMETRICSW>() as u32;
    let scaled = unsafe {
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS,
            ncm.cbSize,
            (&raw mut ncm).cast(),
            0,
            dpi,
        )
    } != 0;
    if !scaled {
        // Before Windows 10 1607 there is no per-dpi query, so scale by hand.
        unsafe {
            SystemParametersInfoW(
                SPI_GETNONCLIENTMETRICS,
                ncm.cbSize,
                (&raw mut ncm).cast(),
                0,
            )
        };
        ncm.lfMessageFont.lfHeight = dip(ncm.lfMessageFont.lfHeight, dpi);
    }
    unsafe { CreateFontIndirectW(&ncm.lfMessageFont) }
}

/// The work area and DPI of the monitor the window should open on.
///
/// The monitor under the cursor is the one whose tray was just clicked, and so
/// the one the user is looking at. Its DPI has to be asked of the monitor
/// rather than of the window, because the window has not been moved there yet.
fn target_monitor(hwnd: HWND) -> (Option<RECT>, u32) {
    let mut pt = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut pt) } != 0 {
        let monitor = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<MONITORINFO>() as u32;
        let mut dpi_x = 0u32;
        let mut dpi_y = 0u32;
        let placed = unsafe { GetMonitorInfoW(monitor, &mut info) } != 0;
        let scaled =
            unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } == 0;
        if placed && scaled {
            return (Some(info.rcWork), dpi_x);
        }
    }
    // Whatever monitor the freshly created window landed on is the fallback.
    (None, unsafe { GetDpiForWindow(hwnd) })
}

/// Size the window to its content and centre it on `work`.
fn place(hwnd: HWND, work: Option<RECT>, dpi: u32) {
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: dip(CLIENT_W, dpi),
        bottom: dip(CLIENT_H, dpi),
    };
    unsafe { AdjustWindowRectExForDpi(&mut frame, STYLE, 0, EX_STYLE, dpi) };
    let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
    let (x, y, flags) = match work {
        Some(r) => (
            r.left + (r.right - r.left - w) / 2,
            r.top + (r.bottom - r.top - h) / 2,
            SWP_NOZORDER,
        ),
        None => (0, 0, SWP_NOZORDER | SWP_NOMOVE),
    };
    unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), x, y, w, h, flags) };
}

// ---------------------------------------------------------------------------
// Reading, validating and committing
// ---------------------------------------------------------------------------

/// What a dropdown is showing.
///
/// Two layers of `Option` and both matter: the inner one is the user's choice
/// of "no key at all", the outer one is a combo with nothing selected, which
/// should not happen because `fill` always selects something.
fn combo_choice(hwnd: HWND, id: i32) -> Option<Option<TriggerKey>> {
    let index = unsafe { SendDlgItemMessageW(hwnd, id, CB_GETCURSEL, 0, 0) };
    // CB_ERR is -1 and fails the conversion.
    let index = usize::try_from(index).ok()?;
    match index.checked_sub(1) {
        // Row 0 is "(none)"; the rest are `TriggerKey::ALL` in order, so the
        // rest of the index is the index into `ALL`.
        None => Some(None),
        Some(key) => TriggerKey::ALL.get(key).copied().map(Some),
    }
}

fn set_combo(hwnd: HWND, id: i32, slot: Option<TriggerKey>) {
    let index = match slot {
        None => 0,
        Some(key) => TriggerKey::ALL
            .iter()
            .position(|k| *k == key)
            .map_or(0, |i| i + 1),
    };
    unsafe { SendDlgItemMessageW(hwnd, id, CB_SETCURSEL, index, 0) };
}

/// The number in an edit, or `None` when it is empty.
fn number(hwnd: HWND, id: i32) -> Option<u32> {
    let mut translated = 0;
    let value = unsafe { GetDlgItemInt(hwnd, id, &mut translated, 0) };
    (translated != 0).then_some(value)
}

fn text_of(hwnd: HWND, id: i32) -> String {
    let mut buffer = [0u16; 64];
    let len = unsafe { GetDlgItemTextW(hwnd, id, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..len as usize])
}

fn set_text(hwnd: HWND, id: i32, text: &str) {
    let text = wide(text);
    unsafe { SetDlgItemTextW(hwnd, id, text.as_ptr()) };
}

fn enable(hwnd: HWND, id: i32, on: bool) {
    unsafe { EnableWindow(GetDlgItem(hwnd, id), i32::from(on)) };
}

/// What the controls currently say, or why they cannot be read.
///
/// The error is what the hint shows, so it is phrased for the user rather than
/// for a log.
fn read(hwnd: HWND) -> Result<Settings, &'static str> {
    let threshold = number(hwnd, IDC_THRESHOLD).ok_or("A threshold is required.")?;
    // Hex, and never prefixed: the "0x" beside the field is a label, so a typed
    // one is a mistake rather than a second way of saying the same thing.
    let typed = text_of(hwnd, IDC_DUMMY);
    if typed.is_empty() {
        return Err("A dummy key is required.");
    }
    let dummy = u16::from_str_radix(&typed, 16)
        .map_err(|_| "The dummy key is hex: two digits, 00 to FF, no 0x.")?;
    Ok(Settings {
        left_trigger: combo_choice(hwnd, IDC_LEFT).ok_or("Nothing is selected.")?,
        right_trigger: combo_choice(hwnd, IDC_RIGHT).ok_or("Nothing is selected.")?,
        threshold_ms: u64::from(threshold),
        dummy_vk: dummy,
    })
}

/// Keep the two dropdowns from ever holding the same key, by swapping instead.
///
/// Two real keys have to differ — set the same, `Machine::side_of` matches left
/// first and "IME on" becomes unreachable. Rather than let the user build that
/// and then refuse it, the clash is resolved the way they almost certainly
/// meant: setting one side to the other's key swaps the two.
///
/// Two empty slots are not a clash and must not swap: "(none)" on both sides is
/// the inert state, and it is allowed.
fn keep_triggers_distinct(hwnd: HWND, state: &mut DialogState, changed: i32) {
    let (Some(left), Some(right)) = (combo_choice(hwnd, IDC_LEFT), combo_choice(hwnd, IDC_RIGHT))
    else {
        return;
    };
    if left.is_some() && left == right {
        let (was_left, was_right) = state.triggers;
        if changed == IDC_LEFT {
            set_combo(hwnd, IDC_RIGHT, was_left);
        } else {
            set_combo(hwnd, IDC_LEFT, was_right);
        }
    }
    state.triggers = (
        combo_choice(hwnd, IDC_LEFT).unwrap_or(left),
        combo_choice(hwnd, IDC_RIGHT).unwrap_or(right),
    );
}

/// Refresh the hint and the enabled state of OK and Apply.
fn revalidate(hwnd: HWND, state: &DialogState) {
    let current = read(hwnd);
    // The hint only ever reports something wrong. Restating the settings back
    // at the user in words tells them nothing the two dropdowns above do not.
    let (hint, blocked) = match current {
        Err(why) => (why.to_string(), true),
        Ok(settings) => match settings.problems().first() {
            Some(problem) => (problem.message(), true),
            None => (String::new(), false),
        },
    };
    // Before the text, so the repaint it triggers picks up the colour.
    HINT_IS_BLOCKING.set(blocked);
    set_text(hwnd, IDC_HINT, &hint);

    enable(hwnd, IDOK, !blocked);
    enable(hwnd, IDC_APPLY, !blocked && current != Ok(state.applied));
}

/// Save and apply what the controls say. `false` means nothing was committed.
fn commit(hwnd: HWND, state: &mut DialogState) -> bool {
    let Ok(settings) = read(hwnd) else {
        return false;
    };
    if !settings.problems().is_empty() {
        return false;
    }
    // The file first. If it cannot be written, the running config must not move
    // either, or the two would disagree until the next restart — and this
    // process stays resident for weeks.
    if let Err(e) = settings::save(&settings) {
        let message = wide(&e);
        let caption = wide("altoggle");
        unsafe {
            MessageBoxW(
                hwnd,
                message.as_ptr(),
                caption.as_ptr(),
                MB_OK | MB_ICONERROR,
            )
        };
        return false;
    }
    state.applied = settings;
    OUTBOX.with_borrow_mut(|out| out.push(settings));
    // Apply greys itself out again now that nothing is dirty.
    revalidate(hwnd, state);
    true
}

// ---------------------------------------------------------------------------
// Window procedure
// ---------------------------------------------------------------------------

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as i32;
            let notify = ((wparam >> 16) & 0xFFFF) as u32;
            match (id, notify) {
                (IDC_LEFT | IDC_RIGHT, CBN_SELCHANGE) => with_state(|state| {
                    keep_triggers_distinct(hwnd, state, id);
                    revalidate(hwnd, state);
                }),
                (IDC_THRESHOLD | IDC_DUMMY, EN_CHANGE) => {
                    with_state(|state| revalidate(hwnd, state))
                }
                (IDOK, _) => {
                    let mut done = false;
                    with_state(|state| done = commit(hwnd, state));
                    if done {
                        unsafe { DestroyWindow(hwnd) };
                    }
                }
                (IDC_APPLY, _) => with_state(|state| {
                    commit(hwnd, state);
                }),
                // Also the Esc key: IsDialogMessageW turns it into this.
                (IDCANCEL, _) => unsafe {
                    DestroyWindow(hwnd);
                },
                _ => {}
            }
            0
        }

        // DefWindowProc answers this with COLOR_WINDOW, which puts a white box
        // behind every label. Only DefDlgProc knows to answer with the dialog
        // face, so on a hand-built window we have to.
        WM_CTLCOLORSTATIC => {
            let hdc = wparam as HDC;
            unsafe {
                SetBkMode(hdc, TRANSPARENT as i32);
                if lparam as HWND == GetDlgItem(hwnd, IDC_HINT) && HINT_IS_BLOCKING.get() {
                    // COLORREF is 0x00BBGGRR, so this is red rather than blue.
                    SetTextColor(hdc, 0x0000_00C0 as COLORREF);
                }
                GetSysColorBrush(COLOR_3DFACE) as LRESULT
            }
        }

        // IsDialogMessageW asks for the default id before it acts on Enter.
        // DefWindowProc does not answer, and the fallback it then takes sends
        // IDOK **without checking whether that button is enabled** — which
        // would let Enter commit settings the greyed-out OK refuses. Answering
        // makes it look the button up, find it disabled, and beep instead.
        DM_GETDEFID => ((DC_HASDEFID as isize) << 16) | IDOK as isize,

        // Opting into per-monitor v2 means Windows stops bitmap-scaling this
        // window when it crosses onto a display at another scale factor — it
        // just leaves it the wrong size. The rect it suggests is in lParam and
        // the new dpi is in both halves of wParam.
        WM_DPICHANGED => {
            // `WINDOW` is only set once `open` has finished assembling the
            // window. Until then the move being reported is the one `open` just
            // made, deliberately, at a size it already computed for the target
            // monitor — and the rect Windows suggests here would scale that
            // size a second time.
            if WINDOW.get().is_null() {
                return 0;
            }
            let suggested = unsafe { &*(lparam as *const RECT) };
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
            with_state(|state| layout(hwnd, state, (wparam & 0xFFFF) as u32));
            0
        }

        WM_CLOSE => {
            unsafe { DestroyWindow(hwnd) };
            0
        }

        // Emphatically not PostQuitMessage: this window shares the process's
        // main message loop, so quitting here would take the whole app down.
        WM_DESTROY => 0,

        // The last message this window sees, and it arrives after every child's
        // own WM_NCDESTROY — so this is where the font is safe to release.
        WM_NCDESTROY => {
            STATE.set(None);
            HINT_IS_BLOCKING.set(false);
            WINDOW.set(std::ptr::null_mut());
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
