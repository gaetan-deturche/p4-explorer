//! A second app window, on another workspace.
//!
//! Nothing the feature stores hold is shared between webviews — their module
//! state is per-window — so a window on the main route already IS an
//! independent workspace as soon as it knows which one to open. That is all
//! this does: name the window after the workspace and put the answer in its URL.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use super::diffwin::enc;

/// Tauri labels take only alphanumerics, `-`, `/`, `:` and `_`, and workspace
/// names here carry dots (`gaetan.deturche_sloclap-41_Curiosity2`).
fn sanitize(client: &str) -> String {
    client
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// One window per workspace, labelled by it, so asking twice raises the window
/// already showing that workspace instead of opening a rival copy — two windows
/// on one workspace would each run their own offline scan over the same files.
///
/// An empty `client` is the "new window" case: it is asked for precisely so a
/// DIFFERENT workspace can be picked in it, so there is nothing yet to name it
/// after and it takes a counted label.
fn label_for(app: &AppHandle, client: &str) -> String {
    if !client.is_empty() {
        return format!("work-{}", sanitize(client));
    }
    for n in 2.. {
        let label = format!("work-new{n}");
        if app.get_webview_window(&label).is_none() {
            return label;
        }
    }
    unreachable!()
}

/// Open (or re-focus) a workspace window. `client` empty = an empty window for
/// the user to pick in.
#[tauri::command]
pub async fn open_workspace_window(
    app: AppHandle,
    port: String,
    client: String,
) -> Result<(), String> {
    let label = label_for(&app, &client);
    if let Some(win) = app.get_webview_window(&label) {
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    // `new=1` says "don't restore the remembered workspace" — an empty window
    // exists to be pointed somewhere else.
    let url = if client.is_empty() {
        format!("?new=1&port={}", enc(&port))
    } else {
        format!("?port={}&client={}", enc(&port), enc(&client))
    };
    let title = if client.is_empty() {
        "Auger".to_string()
    } else {
        format!("Auger — {client}")
    };
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .title(title)
        .inner_size(1280.0, 780.0)
        .min_inner_size(900.0, 500.0)
        // Same as the main window: Tauri's OS drag-drop handler otherwise
        // swallows the in-webview HTML5 drag that moves a file between
        // changelists.
        .disable_drag_drop_handler()
        .visible(false) // shown by wingeom::apply, already at its remembered spot
        .build()
        .map_err(|e| format!("failed to open the workspace window: {e}"))?;
    // One geometry for every workspace window, as for diff/resolve windows:
    // the labels differ per workspace, so per-label state would rarely match.
    crate::wingeom::apply(&win, "work");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn a_workspace_name_becomes_a_usable_label() {
        // Dots are the reason this exists: Tauri rejects a label containing one,
        // and every workspace name on this server starts with `user.name_`.
        assert_eq!(
            sanitize("gaetan.deturche_sloclap-41_Curiosity2"),
            "gaetan_deturche_sloclap-41_Curiosity2"
        );
        assert_eq!(sanitize("ws with spaces"), "ws_with_spaces");
    }
}
