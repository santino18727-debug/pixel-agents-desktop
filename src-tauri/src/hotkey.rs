//! Global keyboard shortcut — toggles main window show/hide.

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tracing::{info, warn};

/// Register the configured global shortcut (default Ctrl+Shift+P / Cmd+Shift+P).
/// If `shortcut_str` is None or empty, no shortcut is registered.
pub fn register(app: &AppHandle, shortcut_str: Option<&str>) {
    let raw = match shortcut_str.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s,
        None => return,
    };

    let shortcut = match parse_shortcut(raw) {
        Some(s) => s,
        None => {
            warn!("Unrecognised global hotkey '{raw}', falling back to Ctrl+Shift+P");
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyP)
        }
    };

    if let Err(e) = app
        .global_shortcut()
        .on_shortcut(shortcut, move |app, _sc, ev| {
            // Fire on key press only (not release) to avoid double-toggling.
            if ev.state() != ShortcutState::Pressed {
                return;
            }
            toggle_main_window(app);
        })
    {
        warn!("Failed to register global shortcut: {e}");
    } else {
        info!("Registered global shortcut: {raw}");
    }
}

fn toggle_main_window(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let visible = win.is_visible().unwrap_or(false);
    let focused = win.is_focused().unwrap_or(false);
    if visible && focused {
        let _ = win.hide();
    } else {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// Parse strings like "Ctrl+Shift+P" or "CmdOrCtrl+Shift+P" into a Shortcut.
fn parse_shortcut(raw: &str) -> Option<Shortcut> {
    let mut mods = Modifiers::empty();
    let mut code: Option<Code> = None;

    for part in raw.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmdorctrl" | "commandorcontrol" => {
                mods |= Modifiers::CONTROL;
            }
            "cmd" | "command" | "super" | "meta" | "win" => {
                mods |= Modifiers::SUPER;
            }
            "shift" => mods |= Modifiers::SHIFT,
            "alt" | "option" => mods |= Modifiers::ALT,
            key => {
                code = parse_key(key);
            }
        }
    }

    code.map(|c| Shortcut::new(if mods.is_empty() { None } else { Some(mods) }, c))
}

fn parse_key(s: &str) -> Option<Code> {
    // Letters
    if s.len() == 1 {
        // SAFETY: len == 1 guarantees at least one char is present.
        let ch = s
            .chars()
            .next()
            .expect("single-byte string must contain one char")
            .to_ascii_uppercase();
        return match ch {
            'A' => Some(Code::KeyA),
            'B' => Some(Code::KeyB),
            'C' => Some(Code::KeyC),
            'D' => Some(Code::KeyD),
            'E' => Some(Code::KeyE),
            'F' => Some(Code::KeyF),
            'G' => Some(Code::KeyG),
            'H' => Some(Code::KeyH),
            'I' => Some(Code::KeyI),
            'J' => Some(Code::KeyJ),
            'K' => Some(Code::KeyK),
            'L' => Some(Code::KeyL),
            'M' => Some(Code::KeyM),
            'N' => Some(Code::KeyN),
            'O' => Some(Code::KeyO),
            'P' => Some(Code::KeyP),
            'Q' => Some(Code::KeyQ),
            'R' => Some(Code::KeyR),
            'S' => Some(Code::KeyS),
            'T' => Some(Code::KeyT),
            'U' => Some(Code::KeyU),
            'V' => Some(Code::KeyV),
            'W' => Some(Code::KeyW),
            'X' => Some(Code::KeyX),
            'Y' => Some(Code::KeyY),
            'Z' => Some(Code::KeyZ),
            '0' => Some(Code::Digit0),
            '1' => Some(Code::Digit1),
            '2' => Some(Code::Digit2),
            '3' => Some(Code::Digit3),
            '4' => Some(Code::Digit4),
            '5' => Some(Code::Digit5),
            '6' => Some(Code::Digit6),
            '7' => Some(Code::Digit7),
            '8' => Some(Code::Digit8),
            '9' => Some(Code::Digit9),
            _ => None,
        };
    }
    match s {
        "space" => Some(Code::Space),
        "tab" => Some(Code::Tab),
        "escape" | "esc" => Some(Code::Escape),
        "enter" | "return" => Some(Code::Enter),
        _ => None,
    }
}
