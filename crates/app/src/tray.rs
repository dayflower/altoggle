//! The tray icon and its menu.
//!
//! Must live on the thread that pumps the main message loop: the tray icon owns
//! a hidden window, and menu clicks arrive as window messages before muda turns
//! them into channel events.
//!
//! Until there is a settings dialog, "Open config file" is the settings UI.
//! Swapping in a dialog replaces this menu item and nothing else, because the
//! values reach the state machine through `HookThread::set_config` either way.

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// What the user picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    OpenConfig,
    ReloadConfig,
    ReinstallHooks,
    Quit,
}

pub struct Tray {
    /// Dropping this removes the icon from the tray, so it has to be kept.
    _icon: TrayIcon,
    open_config: MenuId,
    reload_config: MenuId,
    reinstall_hooks: MenuId,
    quit: MenuId,
}

impl Tray {
    pub fn new(tooltip: &str) -> Result<Self, String> {
        let open_config = MenuItem::new("Open config file", true, None);
        let reload_config = MenuItem::new("Reload config", true, None);
        let reinstall_hooks = MenuItem::new("Reinstall hooks", true, None);
        let quit = MenuItem::new("Quit", true, None);

        let menu = Menu::new();
        menu.append_items(&[
            &open_config,
            &reload_config,
            &PredefinedMenuItem::separator(),
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
            open_config: open_config.id().clone(),
            reload_config: reload_config.id().clone(),
            reinstall_hooks: reinstall_hooks.id().clone(),
            quit: quit.id().clone(),
        })
    }

    /// Drain pending menu clicks. Call after dispatching messages.
    pub fn poll(&self) -> Vec<Command> {
        let mut out = Vec::new();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id();
            let command = if *id == self.open_config {
                Command::OpenConfig
            } else if *id == self.reload_config {
                Command::ReloadConfig
            } else if *id == self.reinstall_hooks {
                Command::ReinstallHooks
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
