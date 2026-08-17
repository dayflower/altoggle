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

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// What the user picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    OpenSettings,
    ReinstallHooks,
    ToggleAutostart,
    Quit,
}

pub struct Tray {
    /// Dropping this removes the icon from the tray, so it has to be kept.
    _icon: TrayIcon,
    settings: MenuId,
    reinstall_hooks: MenuId,
    /// Kept whole, not just its id: its checked state has to be readable and
    /// correctable when the registry write fails.
    autostart: CheckMenuItem,
    quit: MenuId,
}

impl Tray {
    pub fn new(tooltip: &str, autostart_on: bool) -> Result<Self, String> {
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
            .with_icon(make_icon()?)
            .build()
            .map_err(|e| format!("could not create the tray icon: {e}"))?;

        Ok(Self {
            _icon: icon,
            settings: settings.id().clone(),
            reinstall_hooks: reinstall_hooks.id().clone(),
            autostart,
            quit: quit.id().clone(),
        })
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

/// Draw the icon rather than shipping an .ico file.
///
/// A toggle switch: a pill with the knob to one side. Drawn from signed
/// distances so the edges are antialiased, which matters at 32x32 where a hard
/// edge reads as a jagged blob.
fn make_icon() -> Result<Icon, String> {
    const SIZE: u32 = 32;
    /// Pill body, mid blue: visible against both light and dark taskbars.
    const BODY: [u8; 3] = [0x3D, 0x7E, 0xC8];
    /// Knob, near white.
    const KNOB: [u8; 3] = [0xF5, 0xF7, 0xFA];

    // Pill: the segment from (10,16) to (22,16) grown by radius 7.
    let (pill_x0, pill_x1, pill_cy, pill_r) = (10.0f32, 22.0f32, 16.0f32, 7.0f32);
    // Knob: a disc at the right-hand cap.
    let (knob_cx, knob_cy, knob_r) = (22.0f32, 16.0f32, 4.0f32);

    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            // Sample at pixel centres.
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            let nearest_x = px.clamp(pill_x0, pill_x1);
            let pill_d = ((px - nearest_x).powi(2) + (py - pill_cy).powi(2)).sqrt() - pill_r;
            let knob_d = ((px - knob_cx).powi(2) + (py - knob_cy).powi(2)).sqrt() - knob_r;

            // Coverage across one pixel of the edge.
            let pill_a = (0.5 - pill_d).clamp(0.0, 1.0);
            let knob_a = (0.5 - knob_d).clamp(0.0, 1.0);

            // Knob over body, both over transparency.
            let alpha = pill_a.max(knob_a);
            let color = if alpha <= 0.0 {
                [0, 0, 0]
            } else {
                let mut c = [0u8; 3];
                for i in 0..3 {
                    let body = BODY[i] as f32;
                    let knob = KNOB[i] as f32;
                    c[i] = (body * (1.0 - knob_a) + knob * knob_a).round() as u8;
                }
                c
            };

            rgba.extend_from_slice(&[color[0], color[1], color[2], (alpha * 255.0).round() as u8]);
        }
    }

    Icon::from_rgba(rgba, SIZE, SIZE).map_err(|e| format!("could not build the tray icon: {e}"))
}
