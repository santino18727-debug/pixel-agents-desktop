// Pet Mode: a small frameless, transparent, always-on-top window that displays
// a single "pet" character (the most recently active agent). Built as a second
// webview window sharing the same React bundle, differentiated via the
// `?petMode=true` query parameter.

use tauri::{AppHandle, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder};

pub const PET_WINDOW_LABEL: &str = "pet";
const PET_SIZE: f64 = 200.0;

/// Create (or focus, if already present) the pet-mode window.
pub fn create_pet_window(app: &AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window(PET_WINDOW_LABEL) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }

    WebviewWindowBuilder::new(
        app,
        PET_WINDOW_LABEL,
        WebviewUrl::App("index.html?petMode=true".into()),
    )
    .title("Pet")
    .inner_size(PET_SIZE, PET_SIZE)
    .min_inner_size(PET_SIZE, PET_SIZE)
    .max_inner_size(PET_SIZE, PET_SIZE)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .build()
    .map_err(|e| format!("Failed to create pet window: {e}"))?;

    // Ensure the logical size is exact (some platforms ignore the builder hint
    // when combined with `decorations(false)`).
    if let Some(win) = app.get_webview_window(PET_WINDOW_LABEL) {
        let _ = win.set_size(LogicalSize::new(PET_SIZE, PET_SIZE));
    }

    Ok(())
}

/// Close the pet-mode window if it exists. No-op otherwise.
pub fn close_pet_window(app: &AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(PET_WINDOW_LABEL) {
        win.close()
            .map_err(|e| format!("Failed to close pet window: {e}"))?;
    }
    Ok(())
}

/// Returns true if the pet window currently exists.
pub fn is_pet_window_open(app: &AppHandle) -> bool {
    app.get_webview_window(PET_WINDOW_LABEL).is_some()
}
