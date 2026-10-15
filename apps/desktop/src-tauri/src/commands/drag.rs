use crate::state::AppState;
use std::{path::PathBuf, sync::mpsc};
use tauri::State;

/// Native window adaptation only; file validation belongs to core::files.
#[tauri::command]
pub(crate) async fn start_asset_drag(
    window: tauri::Window,
    paths: Vec<PathBuf>,
    preview_path: Option<PathBuf>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let resources = state.resource_service.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let paths =
            oxy_core::files::drag::prepare_drag_paths(paths).map_err(|error| error.to_string())?;
        let preview_path = preview_path.unwrap_or_else(|| paths[0].clone());
        if !paths.contains(&preview_path) {
            return Err("drag preview must belong to the dragged files".into());
        }
        let image = resources.drag_image(preview_path, paths.len())?;
        let (sender, receiver) = mpsc::channel();
        let native_window = window.clone();
        window
            .run_on_main_thread(move || {
                // Validation may wait on an offline/slow volume. AppKit's drag
                // implementation synthesizes an event, so reject a gesture
                // that ended before this main-thread dispatch reached us.
                #[cfg(target_os = "macos")]
                if objc2_app_kit::NSEvent::pressedMouseButtons() & 1 == 0 {
                    let _ = sender.send(Ok(()));
                    return;
                }

                #[cfg(target_os = "linux")]
                let handle = native_window.gtk_window();
                #[cfg(not(target_os = "linux"))]
                let handle = Ok::<_, tauri::Error>(native_window);

                let result = handle
                    .map_err(|error| error.to_string())
                    .and_then(|handle| {
                        let completed = sender.clone();
                        drag::start_drag(
                            &handle,
                            drag::DragItem::Files(paths),
                            drag::Image::Raw(image),
                            move |_, _| {
                                let _ = completed.send(Ok(()));
                            },
                            drag::Options {
                                mode: drag::DragMode::Copy,
                                // AppKit keeps the native drag alive during its
                                // cancellation return animation, swallowing
                                // clicks after the user has released the mouse.
                                skip_animatation_on_cancel_or_failure: true,
                            },
                        )
                        .map_err(|error| error.to_string())
                    });
                // On AppKit/GTK, successful start returns before the gesture
                // ends. Keep the frontend's outbound fence until the native
                // completion callback, including both cancellation and drop.
                if let Err(error) = result {
                    let _ = sender.send(Err(error));
                }
            })
            .map_err(|error| error.to_string())?;
        receiver.recv().map_err(|error| error.to_string())?
    })
    .await
    .map_err(|error| error.to_string())?
}
