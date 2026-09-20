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
use tauri::{Emitter, Manager, http};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let heif = Arc::new(oxy_media::HeifDecodeService::default());
    let protocol_heif = heif.clone();
    let media_resources = oxy_media::shared_resource_registry();
    let measure_media_protocol = std::env::var_os("OXY_PERF_SCENARIO").is_some();
    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol(
            "oxy-media",
            move |context, request, responder| {
                let dispatched = measure_media_protocol.then(std::time::Instant::now);
                let registry = context
                    .app_handle()
                    .state::<AppState>()
                    .media_resources
                    .clone();
                let protocol_heif = Arc::clone(&protocol_heif);
                tauri::async_runtime::spawn_blocking(move || {
                    let started = dispatched.map(|_| std::time::Instant::now());
                    // File-backed originals may live on a slow volume. Hold the read
                    // lease through materialization without blocking the WebView callback.
                    let response = (|| {
                        let parts = request
                            .uri()
                            .path()
                            .trim_start_matches('/')
                            .split('/')
                            .collect::<Vec<_>>();
                        if parts.len() == 2 && parts[0] == "resource" {
                            let resource = registry.resolve(parts[1]);
                            return match resource {
                                Some(resource) => {
                                    let body = registry.materialize(&resource);
                                    match body {
                                        Ok(body) => {
                                            let mut response = http::Response::builder()
                                                .status(http::StatusCode::OK)
                                                .header(http::header::CONTENT_TYPE, resource.media_type)
                                                .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
                                            if let (Some(dispatched), Some(started)) = (dispatched, started) {
                                                response = response
                                                    .header("Timing-Allow-Origin", "*")
                                                    .header("Server-Timing", format!(
                                                        "dispatch;dur={:.3},materialize;dur={:.3}",
                                                        started.duration_since(dispatched).as_secs_f64() * 1000.0,
                                                        started.elapsed().as_secs_f64() * 1000.0,
                                                    ));
                                            }
                                            response.body(body).expect("valid resource protocol response")
                                        },
                                        Err(error) => {
                                            eprintln!(
                                                "media protocol materialization failed: {error}"
                                            );
                                            http::Response::builder()
                                                .status(http::StatusCode::NOT_FOUND)
                                                .header(
                                                    http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
                                                    "*",
                                                )
                                                .body(Vec::new())
                                                .expect("valid missing resource response")
                                        }
                                    }
                                }
                                None => http::Response::builder()
                                    .status(http::StatusCode::NOT_FOUND)
                                    .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                                    .body(Vec::new())
                                    .expect("valid missing resource response"),
                            };
                        }
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
                                let (content_type, stride, body) = match tile.payload {
                                    oxy_media::HeifTileData::Jpeg(jpeg) => {
                                        ("image/jpeg", 0, jpeg.to_vec())
                                    }
                                    oxy_media::HeifTileData::Rgba { stride, bytes } => {
                                        ("application/octet-stream", stride, bytes.to_vec())
                                    }
                                };
                                http::Response::builder()
                                    .status(http::StatusCode::OK)
                                    .header(http::header::CONTENT_TYPE, content_type)
                                    .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                                    .header("x-oxy-width", tile.width)
                                    .header("x-oxy-height", tile.height)
                                    .header("x-oxy-stride", stride)
                                    .body(body)
                                    .expect("valid tile protocol response")
                            }
                            None => http::Response::builder()
                                .status(http::StatusCode::NOT_FOUND)
                                .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                                .body(Vec::new())
                                .expect("valid missing tile response"),
                        }
                    })();
                    responder.respond(response);
                });
            },
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .on_page_load(|webview, _payload| {
            let window = webview.window();
            // Dev reloads can occasionally leave the webview underneath the native title bar.
            // Reapplying decorations forces the native client area to be recalculated.
            let _ = window.set_decorations(true);

            #[cfg(target_os = "macos")]
            {
                let _ = window.set_title_bar_style(tauri::TitleBarStyle::Visible);
            }
        })
        .on_window_event(|window, event| {
            if window.label() == "debug-queues" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                    let _ = window.emit("debug-queue-visibility", false);
                }
                return;
            }
            if window.label() == "main"
                && matches!(
                    event,
                    tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                )
            {
                window.app_handle().exit(0);
            }
        })
        .setup(move |app| {
            let perf_harness = std::env::var_os("OXY_PERF_SCENARIO").is_some();
            let data_dir = if perf_harness {
                std::env::var_os("OXY_PERF_DATA_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or(app.path().app_data_dir()?)
            } else {
                app.path().app_data_dir()?
            };
            let preview_dir = if perf_harness {
                std::env::var_os("OXY_PERF_CACHE_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or(app.path().app_cache_dir()?.join("previews"))
            } else {
                app.path().app_cache_dir()?.join("previews")
            };
            let cache = Arc::new(CacheManager::load(
                preview_dir,
                data_dir.join("cache-settings.json"),
            )?);
            cache.schedule_prune(None);
            let library = Arc::new(Library::open(&data_dir.join("oxyviewer.sqlite"))?);
            let external_apps = Arc::new(state::external_apps::ExternalAppManager::load(data_dir.join("external-apps.json")));
            let metadata_provider = Arc::new(ProviderManager::load(data_dir));
            let files = Arc::new(FsCatalog::default());
            let directory_tree_queue =
                DirectoryTreeQueue::new(app.handle().clone(), files.clone(), library.clone());
            let metadata = metadata_provider.facade();
            let metadata_queue = jobs::metadata::MetadataQueue::new(
                app.handle().clone(),
                files.clone(),
                metadata.clone(),
                library.clone(),
            );
            let preview_queue = jobs::preview::PreviewQueue::new(
                app.handle().clone(),
                library.clone(),
                cache.clone(),
            );
            let library_index_queue =
                jobs::LibraryIndexQueue::new(library.clone(), files.clone());
            app.manage(AppState {
                external_apps,
                files,
                jobs: JobRegistry::default(),
                library,
                cache,
                heif,
                media_resources,
                metadata,
                metadata_queue,
                preview_queue,
                debug_snapshots: Default::default(),
                directory_tree_queue,
                library_index_queue,
                metadata_provider,
                burst_scan: Default::default(),
            });

            create_debug_queue_window(app.handle())?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_external_app_settings,
            update_external_app_settings,
            open_asset_with_application,
            open_asset_with_system_dialog,
            get_raw_decoder_status,
            retry_raw_full,
            open_raw_decoder_install_page,
            open_folder,
            list_assets,
            get_directory_tree,
            set_directory_expanded,
            collapse_directory_tree,
            set_active_directory,
            search_directories,
            refresh_directory,
            get_asset_details,
            request_metadata,
            scan_burst_groups,
            get_preview,
            cancel_preview_request,
            renew_media_resource,
            release_media_resource,
            reconcile_preview_schedule,
            upsert_preview_schedule,
            release_preview_schedule,
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
            list_custom_tags,
            get_asset_tag_assignments,
            get_asset_tag_assignments_by_path,
            create_custom_tag,
            update_custom_tag,
            get_custom_tag_delete_impact,
            delete_custom_tag,
            set_asset_custom_tag,
            get_tag_sync_status,
            retry_tag_xmp_sync,
            cancel_job,
            get_heif_capabilities,
            start_heif_full,
            cancel_heif_decode,
            get_perf_scenario,
            get_app_info,
            check_for_updates,
            open_about_link,
            get_media_resource_stats,
            get_debug_queue_snapshot,
            open_debug_queue_window,
            close_debug_queue_window,
            write_perf_report
        ])
        .run(tauri::generate_context!())
        .expect("error while running OxyViewer");
}
