//! System tray with live agent count and quick actions.
//!
//! Builds the menu once at startup, then updates it whenever the active
//! agent set changes (via `update_active_count`). Updates are debounced
//! to event-driven calls from `file_watcher` — no polling.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tracing::warn;

use crate::error::MutexExt;
use crate::pet_window;

/// Shared state — set of agent IDs currently in `active` status.
pub type ActiveAgents = Arc<Mutex<HashSet<usize>>>;

/// Handles needed to mutate the tray menu after creation.
pub struct TrayState {
    pub active: ActiveAgents,
    pub count_item: Arc<MenuItem<tauri::Wry>>,
}

pub type SharedTrayState = Arc<Mutex<Option<TrayState>>>;

pub fn new_active_agents() -> ActiveAgents {
    Arc::new(Mutex::new(HashSet::new()))
}

pub fn new_shared_tray_state() -> SharedTrayState {
    Arc::new(Mutex::new(None))
}

/// Build the system tray icon + menu. Should be called from `setup()`.
pub fn build_tray(
    app: &AppHandle,
    active: ActiveAgents,
    shared: SharedTrayState,
) -> tauri::Result<()> {
    let count_item = MenuItem::with_id(app, "count", "0 agents actifs", false, None::<&str>)?;
    let sep1 = tauri::menu::PredefinedMenuItem::separator(app)?;
    let pet_item = MenuItem::with_id(app, "pet", "Open Pet Mode", true, None::<&str>)?;
    let refresh_item = MenuItem::with_id(app, "refresh", "Refresh", true, None::<&str>)?;
    let sep2 = tauri::menu::PredefinedMenuItem::separator(app)?;
    let show_item = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let tray_menu = Menu::with_items(
        app,
        &[
            &count_item,
            &sep1,
            &pet_item,
            &refresh_item,
            &sep2,
            &show_item,
            &quit_item,
        ],
    )?;

    let count_arc = Arc::new(count_item);
    *shared.lock_or_recover() = Some(TrayState {
        active,
        count_item: Arc::clone(&count_arc),
    });

    TrayIconBuilder::new()
        .icon(
            app.default_window_icon()
                .expect("default window icon must be embedded at build time")
                .clone(),
        )
        .menu(&tray_menu)
        .tooltip("Pixel Agents Desktop")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
            "show" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.unminimize();
                    let _ = win.set_focus();
                }
            }
            "pet" => {
                // Mirror the toggle_pet_mode command.
                let new_state = if pet_window::is_pet_window_open(app) {
                    let _ = pet_window::close_pet_window(app);
                    false
                } else {
                    pet_window::create_pet_window(app).ok();
                    true
                };
                use tauri::Emitter;
                let _ = app.emit("pet-mode-changed", new_state);
            }
            "refresh" => {
                use tauri::Emitter;
                // Frontend's __pixelAgentsRefresh listener triggers a full bootstrap.
                let _ = app.emit("tray-refresh", ());
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(win) = tray.app_handle().get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
        })
        .build(app)?;

    Ok(())
}

/// Marshal `set_text` onto the main thread. Windows requires native menu
/// mutations to happen on the GUI thread that owns the menu handle —
/// calling `set_text` from a background thread (notify watcher, hooks
/// server) can crash or no-op silently.
fn update_count_text(app: &AppHandle, item: Arc<MenuItem<tauri::Wry>>, count: usize) {
    let text = format_count(count);
    if let Err(e) = app.run_on_main_thread(move || {
        if let Err(e) = item.set_text(&text) {
            warn!("Failed to update tray count: {e}");
        }
    }) {
        warn!("Failed to dispatch tray update to main thread: {e}");
    }
}

/// Mark an agent active and refresh the tray count item.
pub fn mark_active(app: &AppHandle, shared: &SharedTrayState, agent_id: usize) {
    let (item, count) = {
        let guard = shared.lock_or_recover();
        let Some(state) = guard.as_ref() else { return };
        let mut set = state.active.lock_or_recover();
        if !set.insert(agent_id) {
            return;
        }
        (Arc::clone(&state.count_item), set.len())
    };
    update_count_text(app, item, count);
}

/// Mark an agent inactive (closed) and refresh the tray count item.
pub fn mark_inactive(app: &AppHandle, shared: &SharedTrayState, agent_id: usize) {
    let (item, count) = {
        let guard = shared.lock_or_recover();
        let Some(state) = guard.as_ref() else { return };
        let mut set = state.active.lock_or_recover();
        if !set.remove(&agent_id) {
            return;
        }
        (Arc::clone(&state.count_item), set.len())
    };
    update_count_text(app, item, count);
}

fn format_count(n: usize) -> String {
    match n {
        0 => "0 agents actifs".to_owned(),
        1 => "1 agent actif".to_owned(),
        n => format!("{n} agents actifs"),
    }
}
