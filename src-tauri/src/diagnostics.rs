use tracing::{info, warn};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::State;
use crate::app_state::AppState;
use crate::monitor::append_to_file;
use crate::{memory_scanner, ocr, paths};

#[tauri::command]
pub(crate) fn read_scan_log(state: State<AppState>) -> Result<String, String> {
    std::fs::read_to_string(&state.log_path).map_err(|e| e.to_string())
}

#[derive(serde::Deserialize)]
pub struct ApiChange {
    pub item_name: String,
    pub old_qty: i64,
    pub new_qty: i64,
}

#[tauri::command]
pub(crate) fn log_api_changes(state: State<AppState>, changes: Vec<ApiChange>) -> Result<(), String> {
    let mut f = std::fs::OpenOptions::new()
        .create(true).append(true)
        .open(&state.changes_log_path)
        .map_err(|e| e.to_string())?;
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    for c in &changes {
        let _ = writeln!(f, "[{}] Companion API  | {} | {} → {}", ts, c.item_name, c.old_qty, c.new_qty);
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn dump_memory_probe(state: State<'_, AppState>) -> Result<String, String> {
    let log_path = state.memory_probe_path.clone();
    let lines = tokio::task::spawn_blocking(|| {
        memory_scanner::dump_inventory_regions(40)
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

/// Enable or disable logging of raw DE API responses to api_logs/.
#[tauri::command]
pub(crate) fn set_api_log(enabled: bool, state: State<'_, AppState>) {
    state.api_log_enabled.store(enabled, Ordering::SeqCst);
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
                    match memory_scanner::raw_scan_pass(&mut f) {
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
    state.api_quantities_cache.lock().map_err(|e| e.to_string())?.clear();
    state.api_mod_copies_cache.lock().map_err(|e| e.to_string())?.clear();

    // Delete cache and hint files so nothing reloads on next start
    let _ = std::fs::remove_file(&state.quantities_cache_path);
    let _ = std::fs::remove_file(&state.inventory_state_cache_path);
    let _ = std::fs::remove_file(state.log_path.with_file_name("inventory_hints.json"));
    let _ = std::fs::remove_file(state.log_path.with_file_name("mod_hints.json"));

    Ok(())
}

/// Read the riven overlay session log.
#[tauri::command]
pub(crate) fn get_riven_session_log() -> String {
    let path = paths::state_dir().join("frameforge_riven_session.txt");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|_| "(no riven session log yet — open the riven reroll screen first)".into())
}

/// Read the current overlay session log.
#[tauri::command]
pub(crate) fn get_overlay_session_log() -> String {
    let path = paths::state_dir().join("frameforge_overlay_session.txt");
    std::fs::read_to_string(&path).unwrap_or_else(|_| "(no session log yet — trigger a Void Fissure first)".into())
}

/// Frontend tracing — App.tsx and Overlay.tsx call this to write diagnostic
/// lines into the same session log that gets copied to the diagnostics folder.
#[tauri::command]
pub(crate) fn log_relic_fe(msg: String) {
    let path = paths::state_dir().join("frameforge_overlay_session.txt");
    let _ = append_to_file(&path, &format!("[FE] {}\n", msg));
}

/// Force-set the relic-overlay window to HWND_TOPMOST via SetWindowPos.
/// Called from JS on a 150 ms interval while the overlay is visible, to beat
/// Warframe's continuous HWND_TOPMOST reassertion.
#[tauri::command]
pub(crate) fn set_overlay_topmost() {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            FindWindowW, SetWindowPos,
            SWP_NOMOVE, SWP_NOSIZE, SWP_NOACTIVATE,
            HWND_TOPMOST,
        };
        let title: Vec<u16> = "FrameForge Overlay\0".encode_utf16().collect();
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd != 0 {
            SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

/// Diagnostic: position a test window ON TOP OF WARFRAME (finds Warframe's HWND
/// to guarantee the correct monitor) and inject a full-screen coloured div via
/// evaluate_script — bypasses IPC and React entirely (Rust → WebView2 direct).
/// Creates the window from Rust if the pre-declared one doesn't exist.
/// Red = WebView renders, IPC broken. Green = WebView renders, IPC ok.
/// Nothing at all = window creation failed or WebView not rendering.
#[tauri::command]
pub(crate) fn inject_overlay_diagnostic(app: tauri::AppHandle) -> String {
    use tauri::{Manager, PhysicalPosition, PhysicalSize, WebviewWindowBuilder, WebviewUrl};

    // Find Warframe's client area to anchor the diagnostic window to the right monitor.
    #[cfg(target_os = "windows")]
    let (wf_x, wf_y, wf_w, wf_h) = unsafe {
        use windows_sys::Win32::Foundation::{POINT, RECT};
        use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetClientRect};
        use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
        let title: Vec<u16> = "Warframe\0".encode_utf16().collect();
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd != 0 {
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetClientRect(hwnd, &mut r);
            let mut pt = POINT { x: 0, y: 0 };
            ClientToScreen(hwnd, &mut pt);
            (pt.x, pt.y, (r.right - r.left) as i32, (r.bottom - r.top) as i32)
        } else {
            (0, 0, 1920i32, 1080i32)
        }
    };
    #[cfg(not(target_os = "windows"))]
    let (wf_x, wf_y, wf_w, wf_h) = (0i32, 0i32, 1920i32, 1080i32);

    // Place diagnostic at the vertical centre of the Warframe client area, full width.
    let diag_x = wf_x;
    let diag_y = wf_y + wf_h / 2 - 150;
    let diag_w = wf_w.max(400) as u32;
    let diag_h = 300u32;

    let win = match app.get_webview_window("relic-overlay") {
        Some(w) => w,
        None => {
            // Pre-declared window missing — create a fresh one from Rust.
            match WebviewWindowBuilder::new(&app, "relic-overlay",
                WebviewUrl::App("index.html#overlay".into()))
                .title("FrameForge Overlay")
                .position(diag_x as f64, diag_y as f64)
                .inner_size(diag_w as f64, diag_h as f64)
                .transparent(true).decorations(false)
                .always_on_top(true).skip_taskbar(true)
                .resizable(false).focused(false)
                .build()
            {
                Ok(w) => w,
                Err(e) => return format!("create-err:{e}"),
            }
        }
    };

    let _ = win.set_position(tauri::Position::Physical(PhysicalPosition { x: diag_x, y: diag_y }));
    let _ = win.set_size(tauri::Size::Physical(PhysicalSize { width: diag_w, height: diag_h }));
    let _ = win.set_always_on_top(true);
    let _ = win.show();

    // Give WebView2 a moment to paint before we also eval.
    std::thread::sleep(std::time::Duration::from_millis(200));

    let script = r#"
        (function() {
            document.documentElement.style.cssText = 'margin:0;padding:0;width:100%;height:100%;';
            document.body.style.cssText = 'margin:0;padding:0;background:rgba(200,0,0,0.95);color:#fff;font-family:sans-serif;font-size:26px;font-weight:bold;display:flex;align-items:center;justify-content:center;height:100vh;box-sizing:border-box;';
            document.body.innerHTML = '<span>FF WEBVIEW ALIVE — IPC test pending...</span>';
            try {
                window.__TAURI_INTERNALS__.invoke('log_relic_fe', {msg:'[OV] inject_diagnostic IPC ok'});
                document.body.style.background = 'rgba(0,160,0,0.95)';
                document.body.innerHTML = '<span>FF WEBVIEW — IPC OK (you should see this in green)</span>';
            } catch(e) {
                document.body.innerHTML = '<span>FF WEBVIEW — NO IPC: ' + String(e).slice(0,100) + '</span>';
            }
        })();
    "#;
    match win.eval(script) {
        Ok(_) => format!("eval-ok wf=({wf_x},{wf_y},{wf_w},{wf_h}) diag=({diag_x},{diag_y})"),
        Err(e) => format!("eval-err:{e}"),
    }
}

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
        "api_logs"        => state.api_log_dir.clone(),
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
/// `which`: "blobs" | "api_logs" | "raw_scan" | "probe"
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
        "api_logs"        => clear_dir(&state.api_log_dir),
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
/// `which`: "blobs" | "api_logs" | "raw_scan" | "probe" | "diag" | "manual_capture" | "unmatched_paths"
#[tauri::command]
pub(crate) fn get_debug_data_size(state: State<AppState>, which: String) -> u64 {
    match which.as_str() {
        "blobs"           => dir_size_bytes(&state.blob_log_dir),
        "api_logs"        => dir_size_bytes(&state.api_log_dir),
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
        for px in row.chunks_exact(4) {
            f.write_all(&px[..3])?; // B, G, R
        }
        if padding > 0 { f.write_all(&pad[..padding])?; }
    }
    Ok(())
}

/// Capture a diagnostic bundle: scan log + screenshot of the full Warframe window
/// (including any overlay on top via GDI desktop BitBlt / DXGI fallback).
#[tauri::command]
pub(crate) async fn save_auto_diag_capture(state: State<'_, AppState>) -> Result<String, String> {
    // Reuse the frame already captured by the OCR pipeline — no second GPU readback,
    // so no GetDIBits stall that used to freeze the whole PC during fissure VFX.
    let frame = state.last_ocr_frame.lock()
        .ok()
        .and_then(|g| g.clone());
    let auto_capture_dir = state.auto_capture_dir.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let folder = auto_capture_dir.join(&ts);
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;

        let session_log = paths::state_dir().join("frameforge_overlay_session.txt");
        if session_log.exists() {
            let _ = std::fs::copy(&session_log, folder.join("ocr_session_log.txt"));
        }

        match frame {
            Some((pixels, w, h)) => {
                let _ = write_bmp(&folder.join("screenshot.bmp"), &pixels, w, h);
            }
            None => {
                let _ = std::fs::write(
                    folder.join("screenshot_note.txt"),
                    "No OCR frame captured yet — trigger a Void Fissure first.",
                );
            }
        }

        Ok(folder.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
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

        // Half-resolution capture: StretchBlt destination is 4× smaller, so GetDIBits
        // reads 4× less data — significantly reduces GPU stall time.
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
/// Uses GetClientRect + ClientToScreen so the rect matches what the OCR captures —
/// both exclude the window title bar and borders in windowed mode.
#[tauri::command]
pub(crate) fn get_warframe_window_rect() -> Result<[i32; 4], String> {
    // Linux reads the geometry from the same X11 window the capture grabs, so the
    // rect and the captured frame can never describe different areas.
    #[cfg(target_os = "linux")]
    { return ocr::warframe_window_rect(); }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    { return Err("Windows and Linux only".into()); }
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::{POINT, RECT};
        use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetClientRect};
        use windows_sys::Win32::Graphics::Gdi::ClientToScreen;

        let title: Vec<u16> = "Warframe\0".encode_utf16().collect();
        let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if hwnd == 0 { return Err("Warframe window not found".into()); }

        // Client rect is always (0,0,w,h) — convert origin to screen coords
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetClientRect(hwnd, &mut r) };
        let mut origin = POINT { x: 0, y: 0 };
        unsafe { ClientToScreen(hwnd, &mut origin) };

        Ok([origin.x, origin.y, r.right - r.left, r.bottom - r.top])
    }
}

// ── Memory Relic Debug ─────────────────────────────────────────────────────
//
// Completely independent of the EE.log + OCR flow.  Tails EE.log for all raw
// lines AND scans Warframe's process memory for relic-reward-related patterns,
// logging everything to a single file.  Purpose: determine whether memory
// holds the reward choices so we can replace or supplement OCR.

static MEM_RELIC_DEBUG_RUNNING: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub(crate) fn start_memory_relic_debug() -> Result<String, String> {
    if MEM_RELIC_DEBUG_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Memory relic debug already running".to_string());
    }
    let log_path = std::env::temp_dir().join("frameforge_mem_relic_debug.log");
    let log_str  = log_path.to_string_lossy().to_string();
    let header = format!(
        "══════════════════════════════════════════════════\n\
         MEMORY RELIC DEBUG — {}\n\
         Log: {}\n\
         ══════════════════════════════════════════════════\n\n\
         Patterns searched in memory:\n\
           \"HasFissureum\":true           — squad member in fissure (JSON-exact, live blob only)\n\
           \"VoidProjection\":{{\"ItemType\":\"  — squad member's relic (JSON-exact, live blob only)\n\
           VoidProjection               — broad scan, 256-byte context; shows all types in memory\n\
           gets reward                  — in-memory log buffer: [playerID] gets reward [itemPath]\n\
           Host has reward info for all — in-memory log buffer: fires when all players responded\n\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        log_str,
    );
    std::fs::write(&log_path, header.as_bytes()).map_err(|e| e.to_string())?;
    let lp = log_path.clone();
    std::thread::spawn(move || {
        #[cfg(target_os = "windows")]
        mem_relic_debug_loop(&lp);
        MEM_RELIC_DEBUG_RUNNING.store(false, Ordering::SeqCst);
    });
    Ok(log_str)
}

#[tauri::command]
pub(crate) fn stop_memory_relic_debug() {
    MEM_RELIC_DEBUG_RUNNING.store(false, Ordering::SeqCst);
}

#[cfg(target_os = "windows")]
fn mem_relic_debug_loop(log_path: &std::path::Path) {
    use std::collections::HashMap;
    use std::io::{Read, Seek, SeekFrom};
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW,
                PROCESSENTRY32W, TH32CS_SNAPPROCESS,
            },
            Memory::{
                VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT,
                PAGE_GUARD, PAGE_NOACCESS,
            },
            Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
        },
    };

    // Slow patterns: full memory walk every cycle (~55 s). min_addr=0 → all regions.
    const PATTERNS_SLOW: &[(&str, &[u8], usize, usize, u64)] = &[
        ("HasFissureum",         b"\"HasFissureum\":true",               128, 1024, 0),
        ("VoidProjection.relic", b"\"VoidProjection\":{\"ItemType\":\"", 128, 1024, 0),
        ("VoidProjection.raw",   b"VoidProjection",                       16,  256, 0),
        // Lua event name found in heap. Capture 512 bytes after so we can see
        // whether an item path pointer gets written nearby during the reward screen.
        ("RewardVoidProjection", b"RewardVoidProjection",                 32, 512, 0),
    ];
    // Fast patterns: only scan game-binary address space (0x7ff7_xxxx_xxxx).
    // This walk takes milliseconds and runs every loop tick (~1 s).
    // Live log-ring-buffer entries end with \r\n; static format strings end with \n\0.
    // Game binary is mapped at 0x7ff7_xxxx_xxxx on 64-bit Windows (0x0000_7ff7_...).
    // Must be < 0x0000_7fff_ffff_ffff (user-space ceiling). Use 0x0000_7f00_0000_0000
    // so we only scan the high-address region where the game exe/DLLs live.
    const LOG_BUF_MIN: u64 = 0x0000_7f00_0000_0000;
    const PATTERNS_FAST: &[(&str, &[u8], usize, usize, u64)] = &[
        // 96 bytes before "gets reward" captures the player ID preceding it.
        ("log.gets_reward",    b"gets reward /Lotus/",                   96, 256, LOG_BUF_MIN),
        ("log.all_rewards_in", b"Host has reward info for all players now!\r", 0, 64, LOG_BUF_MIN),
    ];

    fn append(path: &std::path::Path, s: &str) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(s.as_bytes());
        }
    }

    fn find_warframe_pid() -> Option<u32> {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == -1isize { return None; }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut entry) == 0 {
                CloseHandle(snap); return None;
            }
            loop {
                let name: Vec<u16> = entry.szExeFile.iter()
                    .copied().take_while(|&c| c != 0).collect();
                if let Ok(s) = String::from_utf16(&name) {
                    if s.eq_ignore_ascii_case("Warframe.x64.exe") {
                        let pid = entry.th32ProcessID;
                        CloseHandle(snap);
                        return Some(pid);
                    }
                }
                if Process32NextW(snap, &mut entry) == 0 { break; }
            }
            CloseHandle(snap);
        }
        None
    }

    fn scan_process(pid: u32, patterns: &[(&str, &[u8], usize, usize, u64)])
        -> Vec<(String, u64, u64, u64, Vec<u8>)>  // (pat_name, region_base, region_size, match_addr, context)
    {
        let mut results = Vec::new();
        unsafe {
            let proc = OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, 0, pid);
            if proc == 0 { return results; }
            let mut addr: u64 = 0;
            loop {
                let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
                let ret = VirtualQueryEx(proc, addr as *const _, &mut mbi,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>());
                if ret == 0 { break; }
                let region_base = mbi.BaseAddress as u64;
                let region_size = mbi.RegionSize as u64;
                addr = region_base.saturating_add(region_size);

                if mbi.State != MEM_COMMIT { continue; }
                if mbi.Protect & (PAGE_GUARD | PAGE_NOACCESS) != 0 { continue; }
                if region_size > 128 * 1024 * 1024 { continue; }

                let mut buf = vec![0u8; region_size as usize];
                let mut read = 0usize;
                let ok = windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory(
                    proc, region_base as *const _, buf.as_mut_ptr() as *mut _, buf.len(), &mut read,
                );
                if ok == 0 || read == 0 { continue; }
                buf.truncate(read);

                for &(name, pat, ctx_before, ctx_after, min_addr) in patterns {
                    if region_base < min_addr { continue; }
                    let mut search_from = 0usize;
                    while let Some(pos) = buf[search_from..].windows(pat.len())
                        .position(|w| w == pat)
                        .map(|p| p + search_from)
                    {
                        let match_addr = region_base + pos as u64;
                        let start = pos.saturating_sub(ctx_before);
                        let end   = (pos + ctx_after).min(buf.len());
                        let ctx   = buf[start..end].to_vec();
                        results.push((name.to_string(), region_base, region_size, match_addr, ctx));
                        search_from = pos + pat.len();
                    }
                }
            }
            CloseHandle(proc);
        }
        results
    }

    fn fmt_context(ctx: &[u8]) -> String {
        // Hex + ASCII side-by-side, 32 bytes per row.
        let mut out = String::new();
        for chunk in ctx.chunks(32) {
            let hex:   String = chunk.iter().map(|b| format!("{:02x} ", b)).collect();
            let ascii: String = chunk.iter().map(|&b| if b >= 0x20 && b < 0x7F { b as char } else { '.' }).collect();
            out.push_str(&format!("  {:<96} {}\n", hex, ascii));
        }
        out
    }

    // EE.log path
    let ee_path = match dirs::data_local_dir() {
        Some(d) => d.join("Warframe").join("EE.log"),
        None => {
            append(log_path, "[ERROR] Cannot find %LOCALAPPDATA%\n");
            return;
        }
    };

    // Open EE.log and seek to end so we only see NEW lines.
    let mut ee_file = std::fs::File::open(&ee_path).ok();
    if let Some(ref mut f) = ee_file {
        let _ = f.seek(SeekFrom::End(0));
    }
    let mut ee_leftover = String::new();

    append(log_path, "[READY] Waiting for Warframe and EE.log events…\n\n");

    // ── Slow scan thread ─────────────────────────────────────────────────
    // Runs full memory walk (~60 s) continuously in background.
    // Results sent via channel; main loop picks them up each tick.
    let (slow_tx, slow_rx) = std::sync::mpsc::channel::<Vec<(String, u64, u64, u64, Vec<u8>)>>();
    std::thread::spawn(move || {
        while MEM_RELIC_DEBUG_RUNNING.load(Ordering::SeqCst) {
            if let Some(pid) = find_warframe_pid() {
                let results = scan_process(pid, PATTERNS_SLOW);
                if slow_tx.send(results).is_err() { break; }
            } else {
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        }
    });

    // Separate prev-match maps so fast and slow don't interfere.
    let mut fast_prev: HashMap<(String, u64), Vec<u8>> = HashMap::new();
    let mut slow_prev: HashMap<(String, u64), Vec<u8>> = HashMap::new();
    let mut scan_num  = 0u32;
    let mut warned_no_wf = false;

    // ── Main loop: fast pass every second ────────────────────────────────
    while MEM_RELIC_DEBUG_RUNNING.load(Ordering::SeqCst) {
        let ts = chrono::Local::now().format("%H:%M:%S%.3f");

        // ── EE.log tail ──────────────────────────────────────────────────
        if ee_file.is_none() {
            ee_file = std::fs::File::open(&ee_path).ok();
            if let Some(ref mut f) = ee_file {
                let _ = f.seek(SeekFrom::End(0));
            }
        }
        if let Some(ref mut f) = ee_file {
            let mut chunk = String::new();
            if f.read_to_string(&mut chunk).is_ok() && !chunk.is_empty() {
                ee_leftover.push_str(&chunk);
                let mut log_buf = String::new();
                while let Some(nl) = ee_leftover.find('\n') {
                    let line = ee_leftover[..nl].trim_end_matches('\r').to_string();
                    ee_leftover = ee_leftover[nl + 1..].to_string();
                    if !line.is_empty() {
                        log_buf.push_str(&format!("[EE] {}  {}\n", ts, line));
                    }
                }
                if !log_buf.is_empty() { append(log_path, &log_buf); }
            }
        }

        let log_set = |label: &str, keys: &[(String, u64)], m: &HashMap<(String, u64), Vec<u8>>| {
            let mut s = String::new();
            for key in keys {
                if let Some(ctx) = m.get(key) {
                    s.push_str(&format!("  {} {} @ 0x{:016x}\n{}\n",
                        label, key.0, key.1, fmt_context(ctx)));
                }
            }
            s
        };

        // ── Fast pass: log-buffer only, every tick ────────────────────────
        if let Some(pid) = find_warframe_pid() {
            warned_no_wf = false;
            append(log_path, &format!("[LOG SCAN @ {}]\n", ts));
            let matches = scan_process(pid, PATTERNS_FAST);
            let mut new_keys:  Vec<(String, u64)> = Vec::new();
            let mut chg_keys:  Vec<(String, u64)> = Vec::new();
            let mut gone_keys: Vec<(String, u64)> = Vec::new();
            let mut cur_map: HashMap<(String, u64), Vec<u8>> = HashMap::new();
            for (name, _rb, _rs, addr, ctx) in &matches {
                let key = (name.clone(), *addr);
                if let Some(prev) = fast_prev.get(&key) {
                    if prev != ctx { chg_keys.push(key.clone()); }
                } else {
                    new_keys.push(key.clone());
                }
                cur_map.insert(key, ctx.clone());
            }
            for key in fast_prev.keys() {
                if !cur_map.contains_key(key) { gone_keys.push(key.clone()); }
            }
            fast_prev = cur_map;

            if !new_keys.is_empty() || !chg_keys.is_empty() || !gone_keys.is_empty() {
                let mut block = format!("\n[LOG SCAN @ {} — PID {}]\n", ts, pid);
                block.push_str(&log_set("NEW    ", &new_keys,  &fast_prev));
                block.push_str(&log_set("CHANGED", &chg_keys,  &fast_prev));
                for key in &gone_keys { block.push_str(&format!("  GONE   {} @ 0x{:016x}\n", key.0, key.1)); }
                append(log_path, &block);
            }
        } else if !warned_no_wf {
            warned_no_wf = true;
            append(log_path, &format!("[MEM @ {}] Warframe not running — will retry\n", ts));
        }

        // ── Drain slow-scan results (non-blocking) ────────────────────────
        while let Ok(matches) = slow_rx.try_recv() {
            scan_num += 1;
            let ts2 = chrono::Local::now().format("%H:%M:%S%.3f");
            let mut new_keys:  Vec<(String, u64)> = Vec::new();
            let mut chg_keys:  Vec<(String, u64)> = Vec::new();
            let mut gone_keys: Vec<(String, u64)> = Vec::new();
            let mut cur_map: HashMap<(String, u64), Vec<u8>> = HashMap::new();
            for (name, _rb, _rs, addr, ctx) in &matches {
                let key = (name.clone(), *addr);
                if let Some(prev) = slow_prev.get(&key) {
                    if prev != ctx { chg_keys.push(key.clone()); }
                } else {
                    new_keys.push(key.clone());
                }
                cur_map.insert(key, ctx.clone());
            }
            for key in slow_prev.keys() {
                if !cur_map.contains_key(key) { gone_keys.push(key.clone()); }
            }
            slow_prev = cur_map;

            if !new_keys.is_empty() || !chg_keys.is_empty() || !gone_keys.is_empty() {
                let mut block = format!("\n[MEM SCAN #{} @ {} — PID {}]\n", scan_num, ts2, find_warframe_pid().unwrap_or(0));
                block.push_str(&log_set("NEW    ", &new_keys,  &slow_prev));
                block.push_str(&log_set("CHANGED", &chg_keys,  &slow_prev));
                for key in &gone_keys { block.push_str(&format!("  GONE   {} @ 0x{:016x}\n", key.0, key.1)); }
                append(log_path, &block);
            }
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    append(log_path, "\n[STOPPED]\n");
}
