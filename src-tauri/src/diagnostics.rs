use tracing::{info, warn};
use std::sync::atomic::Ordering;
use tauri::State;
use crate::app_state::AppState;
use crate::monitor::append_to_file;
use crate::{memory_scanner_linux, ocr};

#[tauri::command]
pub(crate) async fn dump_memory_probe(state: State<'_, AppState>) -> Result<String, String> {
    let log_path = state.memory_probe_path.clone();
    let lines = tokio::task::spawn_blocking(|| {
        memory_scanner_linux::dump_inventory_regions(40)
    }).await.map_err(|e| e.to_string())?;
    let output = lines.join("\n");
    std::fs::write(&log_path, &output).map_err(|e| e.to_string())?;
    Ok(output)
}

/// Enable or disable automatic per-pass inventory blob logging to blobs/.
#[tauri::command]
pub(crate) fn set_blob_log(enabled: bool, state: State<'_, AppState>) {
    state.blob_log_enabled.store(enabled, Ordering::SeqCst);
}


/// Returns "started" or "stopped" so the frontend can update button state.
#[tauri::command]
pub(crate) async fn toggle_raw_scan(state: State<'_, AppState>) -> Result<String, String> {
    let was_active = state.raw_scan_active.swap(true, Ordering::SeqCst);
    if was_active {
        // Already running — stop it
        state.raw_scan_active.store(false, Ordering::SeqCst);
        return Ok("stopped".to_string());
    }

    // Freshly started — truncate the output file and spawn the loop
    let out_path  = state.raw_scan_path.clone();
    let flag      = state.raw_scan_active.clone();

    // Truncate / create the file now so the frontend can see it immediately
    std::fs::write(&out_path, "").map_err(|e| e.to_string())?;

    std::thread::spawn(move || {
        let mut pass = 0u32;
        while flag.load(Ordering::SeqCst) {
            pass += 1;
            let ts = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let header = format!("\n=== Pass {} at {} ===\n", pass, ts);

            // Open for append each pass so file grows in real time
            match std::fs::OpenOptions::new().create(true).append(true).open(&out_path) {
                Ok(mut f) => {
                    use std::io::Write;
                    let _ = f.write_all(header.as_bytes());
                    match memory_scanner_linux::raw_scan_pass(&mut f) {
                        Ok(n)  => { let _ = writeln!(f, "--- pass {} done: {} strings ---", pass, n); }
                        Err(e) => { let _ = writeln!(f, "--- pass {} error: {} ---", pass, e); }
                    }
                }
                Err(e) => { warn!(error = %e, "raw_scan open failed"); }
            }

            // Sleep between passes so the user has time to navigate menus
            for _ in 0..50 {
                if !flag.load(Ordering::SeqCst) { break; }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    });

    Ok("started".to_string())
}

/// Resets the scanned inventory only. The downloaded caches are untouched — the
/// refresh button is what re-fetches those.
#[tauri::command]
pub(crate) fn clear_cache(state: State<AppState>) -> Result<(), String> {
    // Clear change log from DB
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM quantity_changes", []).map_err(|e| e.to_string())?;
    drop(conn);

    // Reset all in-memory inventory state
    state.current_quantities.lock().map_err(|e| e.to_string())?.clear();
    state.unique_quantities.lock().map_err(|e| e.to_string())?.clear();
    state.current_mods.lock().map_err(|e| e.to_string())?.clear();

    // Delete cache and hint files so nothing reloads on next start
    let _ = std::fs::remove_file(&state.quantities_cache_path);
    let _ = std::fs::remove_file(&state.inventory_state_cache_path);
    let _ = std::fs::remove_file(state.log_path.with_file_name("inventory_hints.json"));
    let _ = std::fs::remove_file(state.log_path.with_file_name("mod_hints.json"));

    Ok(())
}

/// Read the riven overlay session log.
#[tauri::command]
pub(crate) fn get_riven_session_log(state: State<'_, AppState>) -> String {
    let path = state.riven_log.clone();
    std::fs::read_to_string(&path)
        .unwrap_or_else(|_| "(no riven session log yet — open the riven reroll screen first)".into())
}

/// Read the current overlay session log.
#[tauri::command]
pub(crate) fn get_overlay_session_log(state: State<'_, AppState>) -> String {
    let path = state.overlay_log.clone();
    std::fs::read_to_string(&path).unwrap_or_else(|_| "(no session log yet — trigger a Void Fissure first)".into())
}

/// Frontend tracing — App.tsx and Overlay.tsx call this to write diagnostic
/// lines into the same session log that gets copied to the diagnostics folder.
#[tauri::command]
pub(crate) fn log_relic_fe(state: State<'_, AppState>, msg: String) {
    let path = state.overlay_log.clone();
    let _ = append_to_file(&path, &format!("[FE] {}\n", msg));
}

/// Debug helper: create a test window from Rust side to verify whether JS-side
/// WebviewWindow creation is broken. Returns Ok("created") or Err(reason).
/// Uses a URL hash (#modular) so the Tauri asset protocol serves clean index.html
/// Toggle debug categorization mode. Returns the new state (true = enabled).
#[tauri::command]
pub(crate) fn toggle_debug_categorization(state: State<AppState>) -> bool {
    let prev = state.debug_cat_enabled.fetch_xor(true, Ordering::SeqCst);
    let enabled = !prev;
    info!(debug_cat = enabled, "debug categorization toggled");
    enabled
}

// diag_dir() removed — all callers now use state.auto_capture_dir directly.

fn dir_size_bytes(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0; };
    entries.filter_map(|e| e.ok()).map(|e| {
        let p = e.path();
        if p.is_dir() { dir_size_bytes(&p) }
        else { std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) }
    }).sum()
}


#[tauri::command]
pub(crate) fn get_diag_folder_size(state: State<AppState>) -> u64 {
    dir_size_bytes(&state.auto_capture_dir)
}

/// Delete all timestamped capture folders inside the auto-capture directory.
/// Returns the size after deletion (always 0 on success).
#[tauri::command]
pub(crate) fn clear_diag_folder(state: State<AppState>) -> u64 {
    let dir = state.auto_capture_dir.clone();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let p = entry.path();
            if p.is_dir() { let _ = std::fs::remove_dir_all(&p); }
            else          { let _ = std::fs::remove_file(&p); }
        }
    }
    0
}

#[tauri::command]
pub(crate) fn open_debug_folder(state: State<AppState>, which: String) -> Result<(), String> {
    let path: std::path::PathBuf = match which.as_str() {
        "blobs"           => state.blob_log_dir.clone(),
        "raw_scan"        => state.raw_scan_path.parent().ok_or("no parent")?.to_path_buf(),
        "probe"           => state.memory_probe_path.parent().ok_or("no parent")?.to_path_buf(),
        "diag"            => state.auto_capture_dir.clone(),
        "manual_capture"  => state.manual_capture_dir.clone(),
        "unmatched_paths" => state.unmatched_paths_dir.clone(),
        _ => return Err("Unknown debug folder".into()),
    };
    std::fs::create_dir_all(&path).ok();
    tauri_plugin_opener::open_path(&path, None::<&str>)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Clear debug data for a specific category.
/// `which`: "blobs" | "raw_scan" | "probe" | "unmatched_paths" | "manual_capture"
#[tauri::command]
pub(crate) fn clear_debug_data(state: State<AppState>, which: String) -> Result<(), String> {
    let clear_dir = |dir: &std::path::Path| {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.filter_map(|e| e.ok()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    };
    match which.as_str() {
        "blobs"           => clear_dir(&state.blob_log_dir),
        "raw_scan"        => { let _ = std::fs::remove_file(&state.raw_scan_path); }
        "probe"           => { let _ = std::fs::remove_file(&state.memory_probe_path); }
        "unmatched_paths" => clear_dir(&state.unmatched_paths_dir),
        "manual_capture"  => {
            if let Ok(entries) = std::fs::read_dir(&state.manual_capture_dir) {
                for e in entries.filter_map(|e| e.ok()) {
                    let p = e.path();
                    if p.is_dir() { let _ = std::fs::remove_dir_all(&p); }
                    else          { let _ = std::fs::remove_file(&p); }
                }
            }
        }
        _ => return Err("Unknown debug data type".into()),
    }
    Ok(())
}

/// Return the byte size of a debug folder or file.
/// `which`: "blobs" | "raw_scan" | "probe" | "diag" | "manual_capture" | "unmatched_paths"
#[tauri::command]
pub(crate) fn get_debug_data_size(state: State<AppState>, which: String) -> u64 {
    match which.as_str() {
        "blobs"           => dir_size_bytes(&state.blob_log_dir),
        "raw_scan"        => std::fs::metadata(&state.raw_scan_path).map(|m| m.len()).unwrap_or(0),
        "probe"           => std::fs::metadata(&state.memory_probe_path).map(|m| m.len()).unwrap_or(0),
        "diag"            => dir_size_bytes(&state.auto_capture_dir),
        "manual_capture"  => dir_size_bytes(&state.manual_capture_dir),
        "unmatched_paths" => dir_size_bytes(&state.unmatched_paths_dir),
        _ => 0,
    }
}

/// Write BGRA pixels as an uncompressed 24-bit BGR BMP file.
/// BMP is lossless and writes in microseconds regardless of resolution —
/// PNG compression at 2560×1440 blocks for 1–3 s and froze the overlay.
/// 24-bit BGR (BI_RGB) uses a standard 54-byte header with no colour masks,
/// opening correctly in every image viewer.
pub(crate) fn write_bmp(path: &std::path::Path, bgra: &[u8], w: u32, h: u32) -> std::io::Result<()> {
    use std::io::Write;
    // 24-bit BGR rows must be padded to a 4-byte boundary.
    let row_bytes  = (w as usize) * 3;
    let padding    = (4 - (row_bytes % 4)) % 4;
    let padded_row = row_bytes + padding;
    let pixel_data_size = padded_row * h as usize;
    let file_size = 54usize + pixel_data_size;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    // BMP file header (14 bytes)
    f.write_all(b"BM")?;
    f.write_all(&(file_size as u32).to_le_bytes())?;
    f.write_all(&[0u8; 4])?;            // reserved
    f.write_all(&54u32.to_le_bytes())?; // pixel data starts immediately after 54-byte header
    // BITMAPINFOHEADER (40 bytes)
    f.write_all(&40u32.to_le_bytes())?;
    f.write_all(&w.to_le_bytes())?;
    f.write_all(&(h as i32).wrapping_neg().to_le_bytes())?; // negative height = top-down
    f.write_all(&1u16.to_le_bytes())?;  // colour planes
    f.write_all(&24u16.to_le_bytes())?; // bits per pixel
    f.write_all(&0u32.to_le_bytes())?;  // BI_RGB — no compression, no extra masks
    f.write_all(&(pixel_data_size as u32).to_le_bytes())?;
    f.write_all(&[0u8; 16])?;           // XPelsPerMeter, YPelsPerMeter, ClrUsed, ClrImportant
    // Pixel data: drop alpha channel (BGRA → BGR), pad each row to 4-byte boundary.
    let pad = [0u8; 4];
    for row in bgra.chunks_exact(w as usize * 4) {
        for px in row.as_chunks::<4>().0 {
            f.write_all(&px[..3])?; // B, G, R
        }
        if padding > 0 { f.write_all(&pad[..padding])?; }
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn capture_diagnostics(state: State<'_, AppState>) -> Result<String, String> {
    let log_path          = state.log_path.clone();
    let changes_path      = state.changes_log_path.clone();
    let manual_capture_dir = state.manual_capture_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let folder = manual_capture_dir.join(&ts);
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;

        if log_path.exists()     { let _ = std::fs::copy(&log_path,     folder.join("scan_log.txt")); }
        if changes_path.exists() { let _ = std::fs::copy(&changes_path, folder.join("changes_log.txt")); }

        // Half resolution keeps the bundle small while UI text stays legible.
        match ocr::capture_screen_for_diagnostics_half() {
            Ok((pixels_bgra, w, h)) => { let _ = write_bmp(&folder.join("screenshot.bmp"), &pixels_bgra, w, h); }
            Err(e) => { let _ = std::fs::write(folder.join("screenshot_error.txt"), &e); }
        }

        Ok(folder.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Returns the Warframe game CLIENT AREA as [x, y, width, height] in screen pixels.
/// The rect comes from the same X11 window the capture grabs, so the rect and
/// the captured frame can never describe different areas — both exclude the
/// window title bar and borders.
#[tauri::command]
pub(crate) fn get_warframe_window_rect() -> Result<[i32; 4], String> {
    ocr::warframe_window_rect()
}
