//! System clipboard for the copy-path context actions.

/// Put `text` on the system clipboard.
#[tauri::command]
pub async fn set_clipboard(text: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        arboard::Clipboard::new()
            .and_then(|mut c| c.set_text(text))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("clipboard task failed: {e}"))?
}
