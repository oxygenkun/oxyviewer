mod commands;
mod jobs;
mod providers;
mod state;

use commands::*;
use jobs::directory_tree::DirectoryTreeQueue;
use oxy_fs::FsCatalog;
use oxy_library::Library;
use oxy_runtime::JobRegistry;
use providers::exiftool::ProviderManager;
use state::{AppState, cache::CacheManager};
use std::sync::Arc;
use tauri::{Manager, http};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let heif = Arc::new(oxy_media::HeifDecodeService::default());
    let protocol_heif = heif.clone();
    tauri::Builder::default()
        .register_uri_scheme_protocol("oxy-media", move |_context, request| {
            let parts = request
                .uri()
                .path()
                .trim_start_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            let tile = if parts.len() == 5 && parts[0] == "tile" {
                let generation = parts[2].parse().ok();
                let x = parts[3].parse().ok();
                let y = parts[4].parse().ok();
                match (generation, x, y) {
                    (Some(generation), Some(x), Some(y)) => {
                        protocol_heif.tile(parts[1], generation, x, y)
                    }
                    _ => None,
                }
            } else {
                None
            };
            match tile {
                Some(tile) => {
                    let (content_type, body) = match tile.encoded_jpeg {
                        Some(jpeg) => ("image/jpeg", jpeg.to_vec()),
                        None => ("application/octet-stream", tile.rgba.to_vec()),
                    };
                    http::Response::builder()
                        .status(http::StatusCode::OK)
                        .header(http::header::CONTENT_TYPE, content_type)
                        .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                        .header("x-oxy-width", tile.width)
                        .header("x-oxy-height", tile.height)
                        .header("x-oxy-stride", tile.stride)
                        .body(body)
                        .expect("valid tile protocol response")
                }
                None => http::Response::builder()
                    .status(http::StatusCode::NOT_FOUND)
                    .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .body(Vec::new())
                    .expect("valid missing tile response"),
            }
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .on_window_event(|window, event| {
            if matches!(
                event,
                tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
            ) {
                window.app_handle().exit(0);
            }
        })
        .setup(move |app| {
            let data_dir = app.path().app_data_dir()?;
            let preview_dir = app.path().app_cache_dir()?.join("previews");
            let cache = Arc::new(CacheManager::load(
                preview_dir,
                data_dir.join("cache-settings.json"),
            )?);
            let library = Arc::new(Library::open(&data_dir.join("oxyviewer.sqlite"))?);
            let metadata_provider = Arc::new(ProviderManager::load(data_dir));
            let files = Arc::new(FsCatalog::default());
            let directory_tree_queue = DirectoryTreeQueue::new(app.handle().clone(), files.clone());
            let metadata = metadata_provider.facade();
            let metadata_queue = jobs::metadata::MetadataQueue::new(
                app.handle().clone(),
                files.clone(),
                metadata.clone(),
                library.clone(),
            );
            let preview_queue =
                jobs::preview::PreviewQueue::new(app.handle().clone(), library.clone());
            app.manage(AppState {
                files,
                jobs: JobRegistry::default(),
                library,
                cache,
                heif: heif.clone(),
                metadata,
                metadata_queue,
                preview_queue,
                directory_tree_queue,
                metadata_provider,
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_folder,
            list_assets,
            get_directory_tree,
            set_directory_expanded,
            set_active_directory,
            search_directories,
            refresh_directory,
            get_asset_details,
            request_metadata,
            get_preview,
            get_cache_settings,
            update_cache_settings,
            clear_preview_cache,
            patch_metadata,
            sync_metadata_to_embedded,
            get_exiftool_status,
            configure_exiftool,
            install_exiftool,
            execute_file_operation,
            open_in_file_manager,
            add_library_root,
            remove_library_root,
            list_library_roots,
            reorder_library_roots,
            cancel_job,
            get_heif_capabilities,
            get_heif_diagnostics,
            get_cached_heif_full,
            start_heif_decode,
            cancel_heif_decode,
            get_perf_scenario,
            write_perf_report
        ])
        .run(tauri::generate_context!())
        .expect("error while running OxyViewer");
}
