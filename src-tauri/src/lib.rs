use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::Manager;
use app_state::{load_corrections, AppState};
use catalogue::{CATALOGUE_CACHE, patch_catalogue_items};
use image_cache::serve_image_files;
use inventory_state::{load_inventory_state_cache, is_unique_path};
use pricing::{BulkPrices, BULK_PRICES_CACHE};
use relic_pick::park_overlay_offscreen;
use settings::{restore_window_state, save_window_state};
use wfm::Wfm;

mod cache;
mod console_login; // [console-login feature] remove this line to drop the feature
mod db;
// EE.log lives at a different path per platform (Proton prefix on Linux), so
// path construction lives here rather than being inlined at each watcher.
mod log_parser;
mod logging;
mod mem_regions;
mod memory_scanner;
#[cfg(target_os = "linux")]
mod memory_scanner_linux;
mod ocr;
// ── BEGIN ocrs fallback ─────────────────────────────────────────────────────
// Linux reads the screen with Tesseract, so this covers Windows alone: it is
// what runs when the machine has no Windows.Media.Ocr language pack.
#[cfg(target_os = "windows")]
mod ocr_fallback;
// ── END ocrs fallback ───────────────────────────────────────────────────────
// Overlay placement is X11 work with no Windows counterpart — the Windows
// overlay needs nothing beyond the window options Tauri already sets.
#[cfg(target_os = "linux")]
mod overlay_linux;
mod paths;
mod refresh;
mod resolver;
mod updater;
mod wfcd;
mod wfm;

mod app_state;
mod catalogue;
mod companion_api;
mod credentials;
mod diagnostics;
mod image_cache;
mod inventory_state;
mod log_watcher;
mod monitor;
mod platform;
mod pricing;
mod relic_pick;
mod rivens;
mod settings;
mod stats;
mod syndicates;
mod trade_log;
mod wfm_commands;
mod wfm_queue;
mod wfm_top;
mod worldstate;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // ==========================================================================
    // Linux: run the GTK/WebKit side under XWayland, not native Wayland
    // ==========================================================================
    //
    // The relic and riven overlays exist to sit exactly on top of the game
    // window, and they are hidden by being parked off-screen. Wayland gives a
    // client no say in where its own surfaces go, so under the native backend
    // `set_position` silently does nothing: the overlay would appear wherever the
    // compositor decided, and "hiding" it off-screen would leave it on screen.
    //
    // Forcing GDK to the X11 backend restores absolute positioning and
    // always-on-top, and it costs nothing in fidelity here — Warframe itself runs
    // under XWayland (it is a Proton/Wine X11 client), so the overlay ends up in
    // the same coordinate space as the window it tracks.
    //
    // Set before any GTK call, which for Tauri means before the builder runs.
    #[cfg(target_os = "linux")]
    if std::env::var_os("GDK_BACKEND").is_none() {
        std::env::set_var("GDK_BACKEND", "x11");
    }

    // A <select> opens a native GTK menu the GTK theme paints, not the page, so
    // our dark CSS never reaches it. The AppImage launcher pins GTK_THEME to the
    // desktop's Adwaita variant, so on a light desktop the popup renders light.
    // Force the dark variant before GTK starts; an explicit APPIMAGE_GTK_THEME
    // still overrides it.
    #[cfg(target_os = "linux")]
    if std::env::var_os("APPIMAGE_GTK_THEME").is_none() {
        std::env::set_var("GTK_THEME", "Adwaita:dark");
    }

    // Everything below used to sit in a single directory; carry the files that
    // cannot be refetched over to the split layout before anything opens them.
    paths::migrate_legacy();
    let cache_dir = paths::cache_dir();
    let data_dir = paths::data_dir();
    let state_dir = paths::state_dir();
    let config_dir = paths::config_dir();

    // ── BEGIN ocrs fallback ─────────────────────────────────────────────────
    #[cfg(target_os = "windows")]
    ocr_fallback::set_data_dir(cache_dir.clone());
    // ── END ocrs fallback ───────────────────────────────────────────────────

    let db_path = data_dir.join("data.db");
    let quantities_cache_path = cache_dir.join("quantities_cache.json");
    let inventory_state_cache_path = cache_dir.join("inventory_state_cache.json");
    let settings_path = config_dir.join("settings.json");
    log_parser::init_watched_log_path(&settings_path);
    let log_path = state_dir.join("scan_log.txt");
    let changes_log_path = state_dir.join("inventory_changes.txt");
    let debug_root = state_dir.join("Debugging");
    let blob_log_dir = debug_root.join("Inventory Snapshots");
    let api_log_dir = debug_root.join("Api Responses");
    let auto_capture_dir = debug_root.join("Auto-Capture");
    let manual_capture_dir = debug_root.join("Manual Capture");
    let memory_probe_dir = debug_root.join("Memory Probe");
    let raw_scan_dir = debug_root.join("Raw Memory Record");
    let unmatched_paths_dir = debug_root.join("Unmatched Paths");
    let raw_scan_path = raw_scan_dir.join("raw_scan.txt");
    let memory_probe_path = memory_probe_dir.join("memory_probe.txt");
    for dir in &[&blob_log_dir, &api_log_dir, &auto_capture_dir, &manual_capture_dir,
                 &memory_probe_dir, &raw_scan_dir, &unmatched_paths_dir] {
        let _ = std::fs::create_dir_all(dir);
    }
    let img_cache_dir = cache_dir.join("img_cache");
    let _ = std::fs::create_dir_all(&img_cache_dir);
    let auction_ids_path = data_dir.join("auction_ids.json");
    let initial_auction_ids: Vec<String> = std::fs::read_to_string(&auction_ids_path)
        .ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    // Serve whatever prices were last written, however old; the background
    // refresh below replaces them once the window is up.
    let initial_relics_run = cache::load::<BulkPrices>(BULK_PRICES_CACHE)
        .map(|c| c.data)
        .unwrap_or_default();
    let initial_relics_run_prices = initial_relics_run.by_name;
    let initial_wfm_prices: HashMap<String, Option<u32>> = initial_relics_run
        .by_slug
        .into_iter()
        .map(|(k, v)| (k, Some(v)))
        .collect();

    // If a factory-reset was requested on the previous run, finish it now:
    // the DB files can only be deleted before a new connection is opened.
    let reset_marker = std::env::temp_dir().join("frameforge_factory_reset");
    if reset_marker.exists() {
        let _ = std::fs::remove_file(&reset_marker);
        for suffix in ["data.db", "data.db-wal", "data.db-shm"] {
            let _ = std::fs::remove_file(data_dir.join(suffix));
        }
    }

    let conn = db::init_db(&db_path).expect("Failed to initialize database");

    // Serve the last catalogue written, however old — the frontend revalidates
    // it in the background once the window is up. `fallback_items` is the floor
    // for a first launch with no network.
    let cached_catalogue = cache::load::<wfcd::FetchResult>(CATALOGUE_CACHE).map(|c| c.data);
    let (
        initial_items,
        initial_recipes,
        initial_relic_drops,
        initial_relic_rewards,
        initial_blueprint_names,
        initial_wiki_reward_names,
        initial_syndicate_catalog,
    ) = match cached_catalogue {
        Some(c) => (
            patch_catalogue_items(c.items),
            c.recipes,
            c.relic_drops,
            c.relic_rewards,
            c.blueprint_names,
            c.wiki_reward_names,
            c.syndicate_catalog,
        ),
        None => (
            wfcd::fallback_items(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            std::collections::HashSet::new(),
            HashMap::new(),
        ),
    };
    let initial_weapon_dispositions: HashMap<String, f32> = initial_items.iter()
        .filter_map(|i| i.omega_attenuation.map(|d| (i.unique_name.clone(), d)))
        .collect();
    // Load unified inventory state cache. All data lives in items: unique_name → CachedItem.
    let initial_state = load_inventory_state_cache(&inventory_state_cache_path);
    // Stackable resources: non-mod, non-unique paths.
    // Also include items whose path would match is_unique_path but whose category is
    // Blueprints or Parts — e.g. ClanTech blueprints live under /Lotus/Weapons/ClanTech/
    // but are stackable resource-scanner items, not unique weapon instances.
    let initial_quantities: HashMap<String, i64> = initial_state.items.iter()
        .filter(|(k, v)| {
            // FlavourItems (skins/cosmetics) are binary-owned. Load them at qty=1 regardless
            // of mod_ranks (the mod scanner picks them up from RawUpgrades and writes mod_ranks
            // to the cache, which would otherwise exclude them from initial_quantities).
            if v.is_flavour { return true; }
            v.mod_ranks.is_none()
                && (!is_unique_path(k) || matches!(v.category.as_str(), "Blueprints" | "Parts"))
                && v.amount > 0
        })
        .map(|(k, v)| (k.clone(), if v.is_flavour { 1 } else { v.amount }))
        .collect();
    // Unique items: warframes, weapons, companions.
    // Exclude blueprint/parts items even when their path matches is_unique_path.
    let initial_unique: HashMap<String, i64> = initial_state.items.iter()
        .filter(|(k, v)| {
            v.mod_ranks.is_none() && is_unique_path(k) && v.amount > 0
                && !matches!(v.category.as_str(), "Blueprints" | "Parts")
        })
        .map(|(k, _)| (k.clone(), 1i64))
        .collect();
    // Mods and arcanes.
    let initial_mods: HashMap<String, memory_scanner::ModCount> = initial_state.items.iter()
        .filter(|(_, v)| v.mod_ranks.is_some())
        .map(|(k, v)| {
            let mc = memory_scanner::ModCount {
                total: v.amount,
                by_rank: v.mod_ranks.as_ref().unwrap()
                    .iter()
                    .filter_map(|(r, &c)| r.parse::<u8>().ok().map(|rank| (rank, c)))
                    .collect(),
            };
            (k.clone(), mc)
        })
        .collect();
    let corrections = load_corrections(&config_dir.join("corrections.json"));
    // Before any command can read the log: rows recorded for a path before it
    // was ignored would otherwise show until the seven-day prune.
    for (path, c) in &corrections {
        if c.category.as_deref() == Some("Ignored") {
            let _ = conn.execute("DELETE FROM quantity_changes WHERE unique_name = ?1", [path]);
        }
    }

    tauri::Builder::default()
        .register_uri_scheme_protocol("ffauth", |ctx, req| console_login::handle_ffauth(ctx.app_handle(), &req)) // [console-login feature]
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updater::LaunchCheck::default())
        .manage(AppState {
            db_path,
            quantities_cache_path,
            inventory_state_cache_path,
            settings_path,
            log_path,
            changes_log_path,
            conn: Mutex::new(conn),
            wfcd_items: Mutex::new(initial_items),
            recipes: Mutex::new(initial_recipes),
            relic_drops: Mutex::new(initial_relic_drops),
            relic_rewards: Mutex::new(initial_relic_rewards),
            blueprint_to_result: Mutex::new(initial_blueprint_names),
            wiki_reward_names: Mutex::new(initial_wiki_reward_names),
            weapon_dispositions: Mutex::new(initial_weapon_dispositions),
            current_quantities: Arc::new(Mutex::new(initial_quantities)),
            unique_quantities: Arc::new(Mutex::new(initial_unique)),
            current_mods: Arc::new(Mutex::new(initial_mods)),
            api_quantities_cache: Arc::new(Mutex::new(HashMap::new())),
            api_mod_copies_cache: Arc::new(Mutex::new(Vec::new())),
            last_ocr_frame: Arc::new(Mutex::new(None)),
            current_crafting: Arc::new(Mutex::new(vec![])),
            monitor_active: Arc::new(AtomicBool::new(false)),
            raw_scan_active: Arc::new(AtomicBool::new(false)),
            raw_scan_path,
            blob_sync_pending: Arc::new(AtomicBool::new(false)),
            blob_log_enabled: Arc::new(AtomicBool::new(false)),
            blob_log_dir,
            api_log_enabled: Arc::new(AtomicBool::new(false)),
            api_log_dir,
            wfm: {
                let w = Arc::new(Wfm::new());
                for (slug, price) in initial_wfm_prices {
                    w.cache_price(slug, price);
                }
                w
            },
            wfm_price_queue: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            wfm_priority_queue: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            wfm_queue_started: Arc::new(AtomicBool::new(false)),
            syndicate_catalog: Mutex::new(initial_syndicate_catalog),
            auction_ids: Mutex::new(initial_auction_ids),
            auction_ids_path,
            img_cache_dir,
            img_server_port: Mutex::new(0),
            local_player_name: Arc::new(Mutex::new(None)),
            pending_relic_rewards: Mutex::new(None),
            relics_run_prices: Mutex::new(initial_relics_run_prices),
            worldstate_cache: Mutex::new(None),
            debug_cat_enabled: Arc::new(AtomicBool::new(false)),
            auto_capture_dir,
            manual_capture_dir,
            memory_probe_path,
            unmatched_paths_dir,
            corrections,
            force_pid_check: Arc::new(AtomicBool::new(false)),
            relic_pick_overlay_enabled: Arc::new(AtomicBool::new(true)),
        })
        .setup(|app| {
            use tauri::Manager;

            logging::init();

            // Every Linux bundle carries its own Tesseract language model. Point
            // the OCR engine at it before anything can call it.
            #[cfg(target_os = "linux")]
            if let Ok(dir) = app.path().resource_dir() {
                ocr::use_bundled_tessdata(&dir);
            }

            // Spin up a tiny local HTTP server that serves cached item images from disk.
            // This is more reliable than convertFileSrc (which needs assetProtocol scope).
            // Bind the std listener here (sync) to get the port, then convert to tokio
            // inside the spawned async block where the tokio runtime is active.
            {
                let img_cache_dir = app.state::<AppState>().img_cache_dir.clone();
                let std_listener = std::net::TcpListener::bind("127.0.0.1:0")
                    .map_err(|e| e.to_string())?;
                let port = std_listener.local_addr().map_err(|e| e.to_string())?.port();
                *app.state::<AppState>().img_server_port.lock().unwrap() = port;
                tauri::async_runtime::spawn(async move {
                    std_listener.set_nonblocking(true).ok();
                    if let Ok(tokio_listener) = tokio::net::TcpListener::from_std(std_listener) {
                        serve_image_files(tokio_listener, img_cache_dir).await;
                    }
                });
            }

            if let Some(window) = app.get_webview_window("main") {
                let icon = tauri::image::Image::from_bytes(
                    include_bytes!("../icons/icon.png")
                ).map_err(|e| e.to_string())?;
                window.set_icon(icon).map_err(|e| e.to_string())?;

                // Restore saved window geometry, then show (window starts hidden so
                // it doesn't flash at the default position on the primary monitor first)
                let state = app.state::<AppState>();
                restore_window_state(app.handle(), &window, &state.settings_path, "window", 400, 300);
                let _ = window.show();
            }

            // Overlay windows start as visible:false in tauri.conf.json.
            // Overlay windows start as visible:false in tauri.conf.json. show() here
            // triggers webview initialisation so the first fissure doesn't pay for it,
            // and the window is put away again immediately so nothing flashes on
            // screen. How "away" is achieved differs per platform — see
            // park_overlay_offscreen.
            // Only relic-overlay needs pre-initialization at startup (to avoid the
            // WebView2 init delay on the first fissure).  overlay-test is on-demand only.
            if let Some(win) = app.get_webview_window("relic-overlay") {
                let _ = win.show();
                // Windows may reposition a newly-shown window that is outside all
                // monitors, and KWin clamps one to y=0 outright — either way the
                // window has to be put away again right after show().
                park_overlay_offscreen(app.handle(), "relic-overlay");
                #[cfg(target_os = "linux")]
                {
                    // The band reports on the rewards and is never clicked, so
                    // the pointer belongs to the game underneath it.
                    let _ = win.set_ignore_cursor_events(true);
                    overlay_linux::hint_before_map(&win, overlay_linux::AfterHinting::LeaveHidden);
                }
            }

            // The riven panel is created from the frontend on demand and is on
            // screen the moment it exists, so its hints are written from here
            // rather than at a call site that would have to know about X11.
            #[cfg(target_os = "linux")]
            {
                use tauri::Listener;
                let handle = app.handle().clone();
                app.listen("tauri://window-created", move |event| {
                    #[derive(serde::Deserialize)]
                    struct Created {
                        label: String,
                    }
                    let Ok(created) = serde_json::from_str::<Created>(event.payload()) else {
                        return;
                    };
                    if created.label != "riven-overlay" {
                        return;
                    }
                    if let Some(win) = handle.get_webview_window(&created.label) {
                        overlay_linux::hint_before_map(&win, overlay_linux::AfterHinting::ShowAgain);
                    }
                });
            }

            // Every cache is revalidated from here on: the first tick, five
            // seconds in, walks the whole table, and a cache still inside its
            // TTL costs a disk read.
            refresh::spawn(app.handle().clone());

            updater::spawn_launch_check(app.handle().clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            catalogue::get_all_items,
            catalogue::get_items_by_paths,
            catalogue::get_current_quantities,
            catalogue::get_item_list_status,
            catalogue::fetch_item_list,
            stats::get_change_log,
            stats::get_tracked_items,
            stats::add_tracked_item,
            stats::remove_tracked_item,
            stats::get_item_snapshots,
            trade_log::get_trades,
            trade_log::add_trade,
            trade_log::delete_trade,
            stats::export_stats,
            stats::import_stats,
            diagnostics::clear_cache,
            settings::load_settings,
            settings::save_settings,
            log_parser::get_ee_log_status,
            diagnostics::read_scan_log,
            diagnostics::log_api_changes,
            diagnostics::dump_memory_probe,
            diagnostics::toggle_raw_scan,
            diagnostics::set_blob_log,
            diagnostics::set_api_log,
            settings::get_app_version,
            settings::set_app_version,
            updater::check_for_update,
            updater::pending_update,
            updater::restart_app,
            settings::force_quit,
            catalogue::get_weapon_catalog,
            catalogue::get_craftable_items,
            diagnostics::toggle_debug_categorization,
            catalogue::get_recipe,
            catalogue::get_recipes_bulk,
            catalogue::get_relic_drops,
            catalogue::get_relic_rewards,
            wfcd::get_drop_data,
            wfm_commands::fetch_wfm_items,
            wfm_commands::fetch_wfm_price,
            wfm_queue::start_wfm_queue,
            wfm_queue::wfm_queue_prices,
            wfm_queue::wfm_queue_price_priority,
            wfm_queue::wfm_get_cached_prices,
            wfm_top::get_wfm_top_items,
            wfm_commands::get_item_price,
            pricing::refresh_bulk_prices,
            settings::factory_reset,
            pricing::refresh_all_caches,
            pricing::get_cache_statuses,
            wfm_commands::wfm_set_status,
            log_watcher::start_log_watcher,
            rivens::ocr_riven_log_error,
            rivens::start_riven_memory_watcher,
            rivens::riven_screen_visible,
            rivens::riven_screen_status,
            rivens::save_riven_roll,
            rivens::get_saved_riven_rolls,
            rivens::delete_saved_riven_roll,
            rivens::rename_saved_riven_roll,
            rivens::get_riven_weapons,
            rivens::reload_riven_database,
            rivens::analyze_riven,
            rivens::ocr_riven_screen,
            diagnostics::get_riven_session_log,
            wfm_commands::wfm_debug_dump,
            wfm_commands::wfm_get_riven_attributes,
            wfm_commands::wfm_get_item_orders,
            wfm_commands::wfm_get_item_statistics,
            wfm_commands::wfm_open_login_window,
            wfm_commands::wfm_close_login_window,
            wfm_commands::wfm_receive_jwt,
            wfm_commands::wfm_receive_tokens,
            wfm_commands::wfm_refresh_token,
            wfm_commands::wfm_set_jwt,
            wfm_commands::wfm_get_jwt,
            platform::get_platform_capabilities,
            credentials::wfm_save_credentials,
            credentials::wfm_load_credentials,
            credentials::wfm_delete_credentials,
            wfm_commands::wfm_login,
            wfm_commands::wfm_logout,
            wfm_commands::wfm_get_session,
            wfm_commands::wfm_fetch_status,
            wfm_commands::wfm_get_orders,
            wfm_commands::wfm_get_item_info,
            wfm_commands::wfm_create_order,
            wfm_commands::wfm_update_order,
            wfm_commands::wfm_delete_order,
            wfm_commands::wfm_create_riven_auction,
            wfm_commands::wfm_switch_riven_type,
            wfm_commands::wfm_get_my_riven_auctions,
            wfm_commands::wfm_delete_auction,
            wfm_commands::wfm_update_auction,
            wfm_commands::wfm_set_auction_visible,
            companion_api::scan_warframe_credentials,
            companion_api::scan_warframe_api_urls,
            companion_api::warframe_login,
            companion_api::fetch_warframe_inventory,
            companion_api::save_mastery_data,
            companion_api::get_saved_inventory,
            companion_api::get_rivens,
            rivens::get_weapon_dispositions,
            companion_api::save_api_inventory,
            syndicates::get_syndicate_stores,
            syndicates::get_research_lab_stores,
            worldstate::fetch_worldstate,
            diagnostics::get_warframe_window_rect,
            diagnostics::get_overlay_session_log,
            relic_pick::get_pending_relic_rewards,
            diagnostics::log_relic_fe,
            diagnostics::set_overlay_topmost,
            diagnostics::inject_overlay_diagnostic,
            relic_pick::debug_create_window,
            relic_pick::show_overlay_window,
            relic_pick::move_overlay_offscreen,
            relic_pick::show_test_overlay_window,
            relic_pick::hide_test_overlay_window,
            diagnostics::get_diag_folder_size,
            diagnostics::clear_diag_folder,
            diagnostics::save_auto_diag_capture,
            diagnostics::capture_diagnostics,
            image_cache::get_img_cache_dir,
            image_cache::prewarm_image_cache,
            diagnostics::open_debug_folder,
            diagnostics::clear_debug_data,
            diagnostics::get_debug_data_size,
            monitor::start_monitor,
            monitor::stop_monitor,
            monitor::poke_scan,
            monitor::set_relic_pick_enabled,
            monitor::get_monitor_status,
            catalogue::get_blueprint_names,
            platform::get_system_locale,
            catalogue::get_current_crafting,
            relic_pick::debug_detect_fissure_era,
            relic_pick::test_relic_pick_overlay,
            relic_pick::debug_ee_log_tail,
            console_login::open_console_login, // [console-login feature]
            wfcd::get_drop_data,
            pricing::get_cache_statuses,
            pricing::refresh_all_caches,
            diagnostics::start_memory_relic_debug,
            diagnostics::stop_memory_relic_debug,
        ])
        .on_window_event(|window, event| {
            let label = window.label().to_string();
            if label == "main" || label == "modular-popout" {
                let prefix = if label == "main" { "window" } else { "modularWin" };
                match event {
                    tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
                        // Persist good position/size eagerly so a subsequent minimize-then-close
                        // doesn't overwrite with sentinel coordinates (-32000,-32000).
                        let app = window.app_handle();
                        if let Some(wv) = app.get_webview_window(&label) {
                            let state = app.state::<AppState>();
                            save_window_state(&wv, &state.settings_path, prefix);
                        }
                    }
                    tauri::WindowEvent::CloseRequested { .. } => {
                        // Do NOT call save_window_state here — window position/size methods
                        // can deadlock when called from within a main-thread event handler.
                        // State is already saved on every Moved/Resized event.
                    }
                    tauri::WindowEvent::Destroyed => {
                        // Kill the process only when the main window is destroyed
                        // (prevents orphaned overlay/modular windows keeping the process alive)
                        if label == "main" {
                            std::process::exit(0);
                        }
                    }
                    _ => {}
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
