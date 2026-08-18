//! The tray icon and its menu.
//!
//! Must live on the thread that pumps the main message loop: the tray icon owns
//! a hidden window, and menu clicks arrive as window messages before muda turns
//! them into channel events.
//!
//! "Settings…" is the whole settings UI. Editing the config file by hand still
//! works, but it is read at start-up only; the dialog is the supported way in,
//! and the values reach the state machine the same way either way, through
//! `HookThread::set_config`.
//!
//! The icon shows what the IME is doing. It is pushed here by `set_state`, from
//! the poll in `main`'s `after_message`; nothing in this module reads the IME,
//! because `ime::read_open_status` blocks and this is the thread the tray menu
//! and the settings window are pumped on.

use std::cell::Cell;

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

use crate::icons::{self, IconState};
use crate::log;

/// What the user picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    OpenSettings,
    ReinstallHooks,
    ToggleAutostart,
    Quit,
}

pub struct Tray {
    /// Dropping this removes the icon from the tray, so it has to be kept —
    /// and `set_state` needs it anyway.
    icon: TrayIcon,
    artwork: icons::Set,
    /// What `icon` is currently displaying, so an unchanged poll costs nothing.
    shown: Cell<IconState>,
    settings: MenuId,
    reinstall_hooks: MenuId,
    /// Kept whole, not just its id: its checked state has to be readable and
    /// correctable when the registry write fails.
    autostart: CheckMenuItem,
    quit: MenuId,
}

impl Tray {
    /// `state` is what to show straight away, so the first icon the user sees is
    /// already right rather than correcting itself a fraction of a second later.
    pub fn new(tooltip: &str, autostart_on: bool, state: IconState) -> Result<Self, String> {
        let artwork = icons::Set::load()?;

        // The ellipsis is the convention for an item that opens a window.
        let settings = MenuItem::new("Settings…", true, None);
        let reinstall_hooks = MenuItem::new("Reinstall hooks", true, None);
        let autostart = CheckMenuItem::new("Start with Windows", true, autostart_on, None);
        let quit = MenuItem::new("Quit", true, None);

        let menu = Menu::new();
        menu.append_items(&[
            &settings,
            &PredefinedMenuItem::separator(),
            &autostart,
            &reinstall_hooks,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|e| format!("could not build the tray menu: {e}"))?;

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(tooltip)
            .with_icon(artwork.get(state))
            .build()
            .map_err(|e| format!("could not create the tray icon: {e}"))?;

        Ok(Self {
            icon,
            artwork,
            shown: Cell::new(state),
            settings: settings.id().clone(),
            reinstall_hooks: reinstall_hooks.id().clone(),
            autostart,
            quit: quit.id().clone(),
        })
    }

    /// Show the artwork for this state.
    ///
    /// **Returns immediately when nothing changed, and that is load-bearing.**
    /// The caller polls several times a second, while `set_icon` is a
    /// `Shell_NotifyIcon(NIM_MODIFY)` plus a `SendMessageW` to the tray's own
    /// hidden window. Only a real change may reach the shell.
    pub fn set_state(&self, state: IconState) {
        if self.shown.get() == state {
            return;
        }
        match self.icon.set_icon(Some(self.artwork.get(state))) {
            // Record only on success, so a transient failure is retried by the
            // next poll rather than being remembered as displayed.
            Ok(()) => self.shown.set(state),
            Err(e) => log::line(format!("could not update the tray icon: {e}")),
        }
    }

    /// Whether the autostart item is currently ticked.
    ///
    /// muda flips the tick itself when the item is clicked, so this reports what
    /// the user just asked for.
    pub fn autostart_checked(&self) -> bool {
        self.autostart.is_checked()
    }

    /// Force the tick back, for when acting on the click failed.
    pub fn set_autostart_checked(&self, checked: bool) {
        self.autostart.set_checked(checked);
    }

    /// Drain pending menu clicks. Call after dispatching messages.
    pub fn poll(&self) -> Vec<Command> {
        let mut out = Vec::new();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id();
            let command = if *id == self.settings {
                Command::OpenSettings
            } else if *id == self.reinstall_hooks {
                Command::ReinstallHooks
            } else if *id == *self.autostart.id() {
                Command::ToggleAutostart
            } else if *id == self.quit {
                Command::Quit
            } else {
                continue;
            };
            out.push(command);
        }
        out
    }
}
