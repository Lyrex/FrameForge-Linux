//! Windows screen capture + OCR engine for Warframe relic reward detection.
//!
//! Capture strategy (automatic, works for all display modes):
//!   1. PrintWindow (GDI) — fast, window-specific, works for Windowed and Borderless Windowed.
//!      Quick brightness check: if the result is dark (avg < 30) the game is almost certainly
//!      in Fullscreen Exclusive mode and GDI can't reach the DX framebuffer.
//!   2. DXGI Desktop Duplication — captures the display output at hardware level, bypasses DWM.
//!      Works for Fullscreen Exclusive, Borderless Windowed, and Windowed.
//!      The correct monitor is chosen dynamically: whichever monitor the Warframe window is on.

// ─── Screenshot ───────────────────────────────────────────────────────────────

/// Compute average pixel brightness from a BGRA buffer (sampled every 64 pixels).
pub fn avg_brightness(pixels: &[u8]) -> u32 {
    let sum: u32 = pixels.chunks_exact(4).step_by(64)
        .map(|p| (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3)
        .sum();
    sum / (pixels.len() / 4 / 64).max(1) as u32
}

/// Main entry point. Tries PrintWindow first, falls back to DXGI if the frame is dark.
/// Returns (BGRA pixels, width, captured_height, full_height, capture_info).
/// captured_height covers the top 85% of the full window — reward cards are always in the
/// upper half of the screen. OCR line filtering (y >= 0.10 of cap_h) removes any HUD text
/// from the very top of the frame (FPS counters, ping overlays, etc.) without cropping.
/// capture_info describes which path was used and the pixel brightness, for session logging.
#[tracing::instrument(level = "info", skip_all)]
pub fn capture_warframe_reward_area() -> Option<(Vec<u8>, u32, u32, u32, String)> {
    // ── Path A: PrintWindow (Windowed / Borderless Windowed) ──────────────────
    if let Some((pixels, w, cap_h, full_h)) = capture_printwindow() {
        let avg = avg_brightness(&pixels);
        // Threshold 20: Warframe's dark-themed reward screen gives avg≈40 in Borderless
        // Windowed — that is valid content, not a failed capture. Only values near zero
        // (avg < 20) indicate Fullscreen Exclusive mode where GDI can't reach the DX buffer.
        if avg >= 20 {
            let info = format!("PrintWindow  {}×{}px (top 80%, cap {}px)  avg_brightness={}", w, full_h, cap_h, avg);
            return Some((pixels, w, cap_h, full_h, info));
        }
        // Truly dark PrintWindow — Fullscreen Exclusive or GPU bypassing GDI.
        // Try DXGI, but only use it if it's at least as bright as PrintWindow, so a
        // black DXGI frame (DXGI Desktop Duplication returning no update) doesn't win
        // over a PrintWindow frame that actually has content.
        if let Some((px2, w2, cap_h2, full_h2)) = capture_dxgi(0.85) {
            let avg2 = avg_brightness(&px2);
            if avg2 >= avg {
                let info = format!(
                    "DXGI  {}×{}px (top 80%, cap {}px)  avg_brightness={} \
                     (PrintWindow was dark: avg={})",
                    w2, full_h2, cap_h2, avg2, avg
                );
                return Some((px2, w2, cap_h2, full_h2, info));
            }
        }
        // DXGI was darker or unavailable — return PrintWindow so the caller can log it.
        let info = format!(
            "PrintWindow  {}×{}px (top 80%, cap {}px)  avg_brightness={} [DARK — DXGI also dark/failed]",
            w, full_h, cap_h, avg
        );
        return Some((pixels, w, cap_h, full_h, info));
    }

    // PrintWindow found no window (Warframe not running?) — try DXGI anyway
    if let Some((pixels, w, cap_h, full_h)) = capture_dxgi(0.85) {
        let avg = avg_brightness(&pixels);
        let info = format!(
            "DXGI  {}×{}px (top 80%, cap {}px)  avg_brightness={} (no Warframe window found)",
            w, full_h, cap_h, avg
        );
        return Some((pixels, w, cap_h, full_h, info));
    }

    None
}

/// GDI PrintWindow capture — works for Windowed and Borderless Windowed.
#[tracing::instrument(level = "debug", skip_all)]
fn capture_printwindow() -> Option<(Vec<u8>, u32, u32, u32)> {
    use std::mem;
    use windows_sys::Win32::{
        Foundation::RECT,
        Graphics::Gdi::{
            CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
            GetDIBits, GetDC, ReleaseDC, SelectObject,
            BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, RGBQUAD,
        },
        UI::WindowsAndMessaging::{FindWindowW, GetClientRect},
    };
    #[link(name = "user32")]
    extern "system" { fn PrintWindow(hwnd: isize, hdcblt: isize, nflags: u32) -> i32; }
    const PW_RENDERFULLCONTENT: u32 = 2;

    unsafe {
        let title: Vec<u16> = "Warframe\0".encode_utf16().collect();
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd == 0 { return None; }

        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(hwnd, &mut rect);
        let full_w = (rect.right - rect.left) as u32;
        let full_h = (rect.bottom - rect.top) as u32;
        if full_w < 100 || full_h < 100 { return None; }

        let cap_h = (full_h as f32 * 0.80) as u32;

        let hdc_win = GetDC(hwnd);
        let hdc_mem = CreateCompatibleDC(hdc_win);
        let hbm     = CreateCompatibleBitmap(hdc_win, full_w as i32, full_h as i32);
        let hbm_old = SelectObject(hdc_mem, hbm);

        PrintWindow(hwnd, hdc_mem, PW_RENDERFULLCONTENT);

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize:          mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth:         full_w as i32,
                biHeight:        -(cap_h as i32),
                biPlanes:        1,
                biBitCount:      32,
                biCompression:   BI_RGB,
                biSizeImage:     0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed:       0,
                biClrImportant:  0,
            },
            bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
        };
        let mut pixels = vec![0u8; (full_w * cap_h * 4) as usize];
        GetDIBits(hdc_mem, hbm, 0, cap_h, pixels.as_mut_ptr() as *mut _, &mut bmi, DIB_RGB_COLORS);

        SelectObject(hdc_mem, hbm_old);
        DeleteObject(hbm);
        DeleteDC(hdc_mem);
        ReleaseDC(hwnd, hdc_win);

        Some((pixels, full_w, cap_h, full_h))
    }
}

/// Capture the Warframe window using PrintWindow and return raw BGRA pixels + dimensions.
/// Single capture can be reused for multiple OCR regions via `ocr_pixels_rect`.
#[tracing::instrument(level = "info", skip_all)]
pub fn capture_warframe_pixels() -> Result<(Vec<u8>, u32, u32), String> {
    use std::mem;
    use windows_sys::Win32::{
        Foundation::RECT,
        Graphics::Gdi::{
            CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
            GetDIBits, GetDC, ReleaseDC, SelectObject,
            BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, RGBQUAD,
        },
        UI::WindowsAndMessaging::{FindWindowW, GetClientRect},
    };
    #[link(name = "user32")]
    extern "system" { fn PrintWindow(hwnd: isize, hdcblt: isize, nflags: u32) -> i32; }
    const PW_RENDERFULLCONTENT: u32 = 2;

    unsafe {
        let title: Vec<u16> = "Warframe\0".encode_utf16().collect();
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd == 0 { return Err("Warframe window not found".into()); }

        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(hwnd, &mut rect);
        let full_w = (rect.right  - rect.left) as u32;
        let full_h = (rect.bottom - rect.top)  as u32;
        if full_w < 100 || full_h < 100 { return Err("Window too small".into()); }

        let hdc_win = GetDC(hwnd);
        let hdc_mem = CreateCompatibleDC(hdc_win);
        let hbm     = CreateCompatibleBitmap(hdc_win, full_w as i32, full_h as i32);
        let hbm_old = SelectObject(hdc_mem, hbm);
        PrintWindow(hwnd, hdc_mem, PW_RENDERFULLCONTENT);

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: full_w as i32,
                biHeight: -(full_h as i32),
                biPlanes: 1, biBitCount: 32, biCompression: BI_RGB,
                biSizeImage: 0, biXPelsPerMeter: 0, biYPelsPerMeter: 0,
                biClrUsed: 0, biClrImportant: 0,
            },
            bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
        };
        let mut pixels = vec![0u8; (full_w * full_h * 4) as usize];
        GetDIBits(hdc_mem, hbm, 0, full_h,
                  pixels.as_mut_ptr() as *mut _, &mut bmi, DIB_RGB_COLORS);
        SelectObject(hdc_mem, hbm_old);
        DeleteObject(hbm);
        DeleteDC(hdc_mem);
        ReleaseDC(hwnd, hdc_win);
        Ok((pixels, full_w, full_h))
    }
}

/// OCR a rectangle from a pre-captured pixel buffer. All coordinates are 0.0–1.0 fractions.
/// Applies a mild contrast stretch before OCR (no upscaling — upscaling distorts numerals).
pub fn ocr_pixels_rect(
    pixels: &[u8], full_w: u32, full_h: u32,
    x_start: f32, x_end: f32, y_start: f32, y_end: f32,
) -> Result<String, String> {
    let col_s = (full_w as f32 * x_start.clamp(0.0, 1.0)) as usize;
    let col_e = ((full_w as f32 * x_end.clamp(0.0, 1.0)) as usize).min(full_w as usize);
    let row_s = (full_h as f32 * y_start.clamp(0.0, 1.0)) as usize;
    let row_e = ((full_h as f32 * y_end.clamp(0.0, 1.0)) as usize).min(full_h as usize);
    let rect_w = (col_e - col_s) as u32;
    let rect_h = (row_e - row_s) as u32;
    if rect_w < 4 || rect_h < 4 { return Err("Region too small".into()); }

    let src_stride  = full_w as usize * 4;
    let dst_stride  = rect_w as usize * 4;
    let mut cropped = vec![0u8; dst_stride * rect_h as usize];
    for row in 0..rect_h as usize {
        let src = (row_s + row) * src_stride + col_s * 4;
        let dst = row * dst_stride;
        cropped[dst..dst + dst_stride].copy_from_slice(&pixels[src..src + dst_stride]);
    }

    let (enhanced, ew, eh) = super::preprocess_for_ocr(&cropped, rect_w, rect_h);
    let bmp = super::to_bmp(&enhanced, ew, eh);
    run_windows_ocr(bmp, ew, eh).map(|(text, _)| text)
}

/// OCR a rectangle WITHOUT preprocessing — for white-on-dark text that OCRs fine raw.
pub fn ocr_pixels_rect_raw(
    pixels: &[u8], full_w: u32, full_h: u32,
    x_start: f32, x_end: f32, y_start: f32, y_end: f32,
) -> Result<String, String> {
    let col_s = (full_w as f32 * x_start.clamp(0.0, 1.0)) as usize;
    let col_e = ((full_w as f32 * x_end.clamp(0.0, 1.0)) as usize).min(full_w as usize);
    let row_s = (full_h as f32 * y_start.clamp(0.0, 1.0)) as usize;
    let row_e = ((full_h as f32 * y_end.clamp(0.0, 1.0)) as usize).min(full_h as usize);
    let rect_w = (col_e - col_s) as u32;
    let rect_h = (row_e - row_s) as u32;
    if rect_w < 4 || rect_h < 4 { return Err("Region too small".into()); }
    let src_stride = full_w as usize * 4;
    let dst_stride = rect_w as usize * 4;
    let mut cropped = vec![0u8; dst_stride * rect_h as usize];
    for row in 0..rect_h as usize {
        let src = (row_s + row) * src_stride + col_s * 4;
        let dst = row * dst_stride;
        cropped[dst..dst + dst_stride].copy_from_slice(&pixels[src..src + dst_stride]);
    }
    let bmp = super::to_bmp(&cropped, rect_w, rect_h);
    run_windows_ocr(bmp, rect_w, rect_h).map(|(text, _)| text)
}

/// Convenience: capture + OCR a vertical strip of the window (full width).
#[allow(dead_code)]
pub fn capture_and_ocr_region(y_start: f32, y_end: f32) -> Result<String, String> {
    let (pixels, w, h) = capture_warframe_pixels()?;
    ocr_pixels_rect(&pixels, w, h, 0.0, 1.0, y_start, y_end)
}

/// Convenience: capture + OCR a specific rectangle.
#[allow(dead_code)]
pub fn capture_rect_and_ocr(x_start: f32, x_end: f32, y_start: f32, y_end: f32) -> Result<String, String> {
    let (pixels, w, h) = capture_warframe_pixels()?;
    ocr_pixels_rect(&pixels, w, h, x_start, x_end, y_start, y_end)
}

/// Detect the void fissure era from the relic selection screen.
/// The era label ("LITH ERA", "MESO ERA", etc.) is displayed in the top-left quarter
/// of the screen. Returns the era key ("LITH", "MESO", "NEO", "AXI", "ALL") or None.
pub fn detect_fissure_era() -> Option<String> {
    let (pixels, w, h) = capture_warframe_pixels().ok()?;
    let text = ocr_pixels_rect(&pixels, w, h, 0.0, 0.5, 0.0, 0.25).ok()?;
    let upper = text.to_uppercase();
    // Prefer the full "LITH ERA" pattern for specificity
    for era in &["LITH", "MESO", "NEO", "AXI"] {
        if upper.contains(&format!("{} ERA", era)) {
            return Some(era.to_string());
        }
    }
    if upper.contains("ALL ERA") || upper.contains("ALL ERAS") {
        return Some("ALL".to_string());
    }
    // Fallback: bare era word (OCR might drop "ERA")
    for era in &["LITH", "MESO", "NEO", "AXI"] {
        if upper.contains(era) {
            return Some(era.to_string());
        }
    }
    if upper.contains("ALL") {
        return Some("ALL".to_string());
    }
    None
}

/// Half-resolution capture for automatic diagnostics.
/// StretchBlt writes a smaller destination bitmap so GetDIBits reads 4× less data,
/// reducing GPU pipeline stalls on high-resolution displays.
pub fn capture_screen_for_diagnostics_half() -> Result<(Vec<u8>, u32, u32), String> {
    if let Some((pixels, w, h)) = capture_screen_gdi_scaled(1, 2) {
        if avg_brightness(&pixels) >= 10 {
            return Ok((pixels, w, h));
        }
    }
    match capture_dxgi(1.0) {
        Some((pixels, w, _cap_h, full_h)) => Ok((pixels, w, full_h)),
        None => Err("Warframe window not found or capture failed".into()),
    }
}

/// GDI capture of the Warframe window at a fractional scale (num/denom).
/// Uses StretchBlt so the destination bitmap — and therefore the GetDIBits readback —
/// is (num/denom)² smaller, which reduces GPU pipeline stall time proportionally.
/// Pass (1, 1) for full resolution; (1, 2) for half resolution, etc.
/// DWM composites all windows before BitBlt reads them, so overlay windows are visible.
fn capture_screen_gdi_scaled(num: u32, denom: u32) -> Option<(Vec<u8>, u32, u32)> {
    use std::mem;
    use windows_sys::Win32::{
        Foundation::RECT,
        Graphics::Gdi::{
            StretchBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
            GetDC, GetDIBits, ReleaseDC, SelectObject,
            BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, RGBQUAD,
            SRCCOPY, HALFTONE, SetStretchBltMode,
        },
        UI::WindowsAndMessaging::{FindWindowW, GetWindowRect},
    };
    unsafe {
        let title: Vec<u16> = "Warframe\0".encode_utf16().collect();
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd == 0 { return None; }

        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetWindowRect(hwnd, &mut rect);
        let src_w = (rect.right  - rect.left) as u32;
        let src_h = (rect.bottom - rect.top)  as u32;
        if src_w < 100 || src_h < 100 { return None; }

        // Destination size after scale — at least 1 pixel each dimension.
        let dst_w = ((src_w * num) / denom).max(1);
        let dst_h = ((src_h * num) / denom).max(1);

        let hdc_screen = GetDC(0);
        let hdc_mem    = CreateCompatibleDC(hdc_screen);
        let hbm        = CreateCompatibleBitmap(hdc_screen, dst_w as i32, dst_h as i32);
        let hbm_old    = SelectObject(hdc_mem, hbm);

        // HALFTONE gives better quality when downscaling.
        SetStretchBltMode(hdc_mem, HALFTONE);
        StretchBlt(
            hdc_mem,    0, 0, dst_w as i32, dst_h as i32,  // dest
            hdc_screen, rect.left, rect.top, src_w as i32, src_h as i32, // src
            SRCCOPY,
        );

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize:        mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth:       dst_w as i32,
                biHeight:      -(dst_h as i32), // negative = top-down row order
                biPlanes:      1,
                biBitCount:    32,
                biCompression: BI_RGB,
                biSizeImage: 0, biXPelsPerMeter: 0, biYPelsPerMeter: 0,
                biClrUsed: 0, biClrImportant: 0,
            },
            bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
        };
        let mut pixels = vec![0u8; (dst_w * dst_h * 4) as usize];
        GetDIBits(hdc_mem, hbm, 0, dst_h,
                  pixels.as_mut_ptr() as *mut _, &mut bmi, DIB_RGB_COLORS);

        SelectObject(hdc_mem, hbm_old);
        DeleteObject(hbm);
        DeleteDC(hdc_mem);
        ReleaseDC(0, hdc_screen);
        Some((pixels, dst_w, dst_h))
    }
}

/// DXGI Desktop Duplication capture — works for Fullscreen Exclusive (and all other modes).
///
/// Dynamically determines which monitor the Warframe window is on so this works correctly
/// for any number of monitors, any primary/secondary arrangement, and any resolution.
/// Falls back to the primary monitor if the Warframe window can't be found.
#[tracing::instrument(level = "debug", skip_all)]
fn capture_dxgi(cap_frac: f32) -> Option<(Vec<u8>, u32, u32, u32)> {
    use windows::core::Interface; // required for .cast() on COM types
    use windows::Win32::Graphics::{
        Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
        Direct3D11::{
            D3D11CreateDevice, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ,
            D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
            ID3D11Resource, ID3D11Texture2D, D3D11_MAPPED_SUBRESOURCE,
        },
        Dxgi::{
            CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
            IDXGIResource, DXGI_OUTDUPL_FRAME_INFO,
        },
        Dxgi::Common::DXGI_SAMPLE_DESC,
    };

    // Walk every adapter → every output. We create a D3D device bound to each
    // specific adapter before calling DuplicateOutput — cross-adapter calls fail
    // on multi-GPU systems (e.g. Intel iGPU + NVIDIA dGPU) where the game runs
    // on the discrete GPU. A single device created with D3D_DRIVER_TYPE_HARDWARE
    // defaults to adapter 0 (often the iGPU), causing DuplicateOutput to succeed
    // only on the iGPU's outputs and silently miss the game on the dGPU.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;

        let mut result: Option<(Vec<u8>, u32, u32, u32)> = None;

        'outer: for ai in 0u32.. {
            let adapter = match factory.EnumAdapters(ai) { Ok(a) => a, Err(_) => break };

            // Create a D3D device bound to THIS adapter so DuplicateOutput is same-adapter.
            let adapter_iface: IDXGIAdapter = match adapter.cast() { Ok(a) => a, Err(_) => continue };
            let mut device = None;
            let mut ctx    = None;
            if D3D11CreateDevice(
                Some(&adapter_iface), D3D_DRIVER_TYPE_UNKNOWN, None,
                Default::default(), None, 7,
                Some(&mut device), None, Some(&mut ctx),
            ).is_err() { continue; }
            let device = match device { Some(d) => d, None => continue };
            let ctx    = match ctx    { Some(c) => c, None => continue };
            let unk: windows::core::IUnknown = match device.cast() { Ok(u) => u, Err(_) => continue };

            for oi in 0u32.. {
                let output: IDXGIOutput = match adapter.EnumOutputs(oi) { Ok(o) => o, Err(_) => break };
                let out1: IDXGIOutput1  = match output.cast() { Ok(o) => o, Err(_) => continue };

                let dupl = match out1.DuplicateOutput(&unk) { Ok(d) => d, Err(_) => continue };

                // Acquire current frame (500 ms timeout)
                let mut fi  = DXGI_OUTDUPL_FRAME_INFO::default();
                let mut res: Option<IDXGIResource> = None;
                if dupl.AcquireNextFrame(500, &mut fi, &mut res).is_err() { continue; }
                let res = match res { Some(r) => r, None => { let _ = dupl.ReleaseFrame(); continue } };

                // Get the desktop texture and read its dimensions
                let src: ID3D11Texture2D = match res.cast() {
                    Ok(t) => t,
                    Err(_) => { let _ = dupl.ReleaseFrame(); continue }
                };
                let mut src_desc = D3D11_TEXTURE2D_DESC::default();
                src.GetDesc(&mut src_desc);
                let full_w = src_desc.Width;
                let full_h = src_desc.Height;
                if full_w < 100 || full_h < 100 { let _ = dupl.ReleaseFrame(); continue; }

                // Create CPU-readable staging texture (full monitor size)
                let staging_desc = D3D11_TEXTURE2D_DESC {
                    Width:          full_w,
                    Height:         full_h,
                    MipLevels:      1,
                    ArraySize:      1,
                    Format:         src_desc.Format,
                    SampleDesc:     DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage:          D3D11_USAGE_STAGING,
                    BindFlags:      Default::default(),
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags:      Default::default(),
                };
                let mut staging: Option<ID3D11Texture2D> = None;
                if device.CreateTexture2D(&staging_desc, None, Some(&mut staging)).is_err() {
                    let _ = dupl.ReleaseFrame(); continue;
                }
                let staging = match staging { Some(s) => s, None => { let _ = dupl.ReleaseFrame(); continue } };

                // GPU blit → staging → map to CPU
                ctx.CopyResource(&staging.cast::<ID3D11Resource>().ok()?,
                                 &src.cast::<ID3D11Resource>().ok()?);

                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                if ctx.Map(&staging.cast::<ID3D11Resource>().ok()?, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).is_err() {
                    let _ = dupl.ReleaseFrame(); continue;
                }

                let cap_h     = ((full_h as f32 * cap_frac) as u32).max(1);
                let row_pitch = mapped.RowPitch as usize;
                let src_ptr   = mapped.pData as *const u8;

                // DXGI is typically BGRA. Swap R↔B if RGBA so OCR pipeline always gets BGRA.
                use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
                let swap_rb = src_desc.Format == DXGI_FORMAT_R8G8B8A8_UNORM;

                let mut pixels = Vec::with_capacity((full_w * cap_h * 4) as usize);
                for row in 0..(cap_h as usize) {
                    let slice = std::slice::from_raw_parts(
                        src_ptr.add(row * row_pitch), full_w as usize * 4);
                    if swap_rb {
                        for px in slice.chunks_exact(4) {
                            pixels.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                        }
                    } else {
                        pixels.extend_from_slice(slice);
                    }
                }

                ctx.Unmap(&staging.cast::<ID3D11Resource>().ok()?, 0);
                let _ = dupl.ReleaseFrame();

                result = Some((pixels, full_w, cap_h, full_h));
                break 'outer;
            }
        }

        result
    }
}

/// Captures the full desktop for diagnostics so the FrameForge overlay is visible.
/// Uses GDI BitBlt from the desktop DC — DWM composites all windows (including the
/// WebView2 overlay) before BitBlt reads them, so the overlay appears in the BMP.
/// DXGI captures GPU output before DWM overlay compositing and misses WebView2 windows.
/// Returns (BGRA pixels, width, height) or None on failure.
pub fn capture_desktop_for_diag() -> Option<(Vec<u8>, u32, u32)> {
    if let Some((pixels, w, h)) = capture_screen_gdi_scaled(1, 1) {
        if avg_brightness(&pixels) >= 5 {
            return Some((pixels, w, h));
        }
    }
    // Fallback: DXGI (fullscreen exclusive — overlay not visible there anyway).
    if let Some((pixels, w, cap_h, _full_h)) = capture_dxgi(1.0) {
        return Some((pixels, w, cap_h));
    }
    None
}

// ─── Windows OCR ──────────────────────────────────────────────────────────────

pub type OcrResult = (String, Vec<(String, f32, f32)>);

/// Run Windows.Media.Ocr on a BMP. Returns (full_text, line_positions).
/// line_positions: Vec<(line_text, x_frac)> — X centre per line from word bounding rects.
#[tracing::instrument(level = "info", skip_all)]
pub fn run_windows_ocr(bmp: Vec<u8>, img_w: u32, img_h: u32) -> Result<OcrResult, String> {
    // Ensure COM is initialized for this thread. Tokio spawn_blocking threads
    // start without a COM apartment; WinRT calls fail or return empty silently.
    // CoInitializeEx returns S_OK (first init), S_FALSE (already MTA), or
    // RPC_E_CHANGED_MODE (already STA) — all safe to ignore.
    unsafe {
        windows_sys::Win32::System::Com::CoInitializeEx(
            std::ptr::null(),
            windows_sys::Win32::System::Com::COINIT_MULTITHREADED.try_into().unwrap_or(0),
        );
    }

    use windows::{
        Foundation::Collections::IVectorView,
        Globalization::Language,
        Graphics::Imaging::BitmapDecoder,
        Media::Ocr::{OcrEngine, OcrLine},
        Storage::Streams::{DataWriter, InMemoryRandomAccessStream},
    };

    let winrt_result = (|| -> windows::core::Result<OcrResult> {
        let stream = InMemoryRandomAccessStream::new()
            .map_err(|e| windows::core::Error::new(e.code(), format!("[stream-create] {e}").as_str()))?;
        let writer = DataWriter::CreateDataWriter(&stream)
            .map_err(|e| windows::core::Error::new(e.code(), format!("[writer-create] {e}").as_str()))?;
        writer.WriteBytes(&bmp)
            .map_err(|e| windows::core::Error::new(e.code(), format!("[write-bytes] {e}").as_str()))?;
        writer.StoreAsync().map_err(|e| windows::core::Error::new(e.code(), format!("[store-async] {e}").as_str()))?.get()
            .map_err(|e| windows::core::Error::new(e.code(), format!("[store-get] {e}").as_str()))?;
        writer.FlushAsync().map_err(|e| windows::core::Error::new(e.code(), format!("[flush-async] {e}").as_str()))?.get()
            .map_err(|e| windows::core::Error::new(e.code(), format!("[flush-get] {e}").as_str()))?;
        writer.DetachStream()
            .map_err(|e| windows::core::Error::new(e.code(), format!("[detach-stream] {e}").as_str()))?;
        stream.Seek(0)
            .map_err(|e| windows::core::Error::new(e.code(), format!("[seek] {e}").as_str()))?;

        let decoder = BitmapDecoder::CreateAsync(&stream)
            .map_err(|e| windows::core::Error::new(e.code(), format!("[decoder-async] {e}").as_str()))?.get()
            .map_err(|e| windows::core::Error::new(e.code(), format!("[decoder-get] {e}").as_str()))?;
        let bitmap = decoder.GetSoftwareBitmapAsync()
            .map_err(|e| windows::core::Error::new(e.code(), format!("[bitmap-async] {e}").as_str()))?.get()
            .map_err(|e| windows::core::Error::new(e.code(), format!("[bitmap-get] {e}").as_str()))?;

        // Try en-US first, then profile languages, then any available language.
        // TryCreate* returns Err(Error::empty()) / HRESULT(0) when the language
        // pack is not installed (Windows-rs wraps the null return as Error::empty()).
        let engine = (|| -> windows::core::Result<OcrEngine> {
            if let Ok(lang) = Language::CreateLanguage(&windows::core::HSTRING::from("en-US")) {
                if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&lang) {
                    return Ok(engine);
                }
            }
            if let Ok(lang) = Language::CreateLanguage(&windows::core::HSTRING::from("en-GB")) {
                if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&lang) {
                    return Ok(engine);
                }
            }
            if let Ok(engine) = OcrEngine::TryCreateFromUserProfileLanguages() {
                return Ok(engine);
            }
            let langs = OcrEngine::AvailableRecognizerLanguages()?;
            if langs.Size()? > 0 {
                if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&langs.GetAt(0)?) {
                    return Ok(engine);
                }
            }
            Err(windows::core::Error::new(
                windows::core::HRESULT(0x80004005u32 as i32), // E_FAIL
                "[engine-create] No OCR language packs found. Install English (United States) or English (United Kingdom) in Windows Settings → Time & Language → Language & Region.",
            ))
        })().map_err(|e| windows::core::Error::new(e.code(), format!("[engine] {e}").as_str()))?;
        let result = engine.RecognizeAsync(&bitmap)
            .map_err(|e| windows::core::Error::new(e.code(), format!("[recognize-async] {e}").as_str()))?.get()
            .map_err(|e| windows::core::Error::new(e.code(), format!("[recognize-get] {e}").as_str()))?;

        let mut full = String::new();
        let mut lines_out: Vec<(String, f32, f32)> = Vec::new();
        let lines: IVectorView<OcrLine> = result.Lines()?;
        let count = lines.Size()?;
        // Inter-card gap: reward cards are separated by ~10–12% of image width.
        // Word gaps within a single item name are ≈ 3%. Splitting at 7% cleanly
        // divides "Daikyu Prime Upper Limb Nautilus Prime Systems" (which WinRT
        // merges into one line when both names share the same baseline Y) into the
        // two separate card entries the column-assignment logic expects.
        const WORD_GAP: f32 = 0.07;
        for i in 0..count {
            let line = lines.GetAt(i)?;
            let words = line.Words()?;
            let wc = words.Size()?;
            if wc == 0 {
                let text = line.Text()?.to_string();
                full.push_str(&text); full.push('\n');
                lines_out.push((text, 0.5, 0.5));
                continue;
            }
            // Walk words left-to-right; flush a sub-line whenever the horizontal
            // gap to the next word exceeds WORD_GAP.
            let mut seg_texts: Vec<String> = Vec::new();
            let mut seg_sx = 0.0f32;
            let mut seg_sy = 0.0f32;
            let mut seg_n  = 0u32;
            let mut prev_right = -1.0f32;
            for j in 0..wc {
                let w  = words.GetAt(j)?;
                let r  = w.BoundingRect()?;
                let cx = if img_w > 0 { (r.X + r.Width  / 2.0) / img_w as f32 } else { 0.5 };
                let cy = if img_h > 0 { (r.Y + r.Height / 2.0) / img_h as f32 } else { 0.5 };
                let xl = if img_w > 0 { r.X / img_w as f32 } else { 0.0 };
                let xr = if img_w > 0 { (r.X + r.Width) / img_w as f32 } else { 1.0 };
                if prev_right >= 0.0 && (xl - prev_right) > WORD_GAP && !seg_texts.is_empty() {
                    let sub = seg_texts.join(" ");
                    full.push_str(&sub); full.push('\n');
                    lines_out.push((sub, seg_sx / seg_n as f32, seg_sy / seg_n as f32));
                    seg_texts.clear(); seg_sx = 0.0; seg_sy = 0.0; seg_n = 0;
                }
                seg_texts.push(w.Text()?.to_string());
                seg_sx += cx; seg_sy += cy; seg_n += 1;
                prev_right = xr;
            }
            if !seg_texts.is_empty() {
                let sub = seg_texts.join(" ");
                full.push_str(&sub); full.push('\n');
                lines_out.push((sub, seg_sx / seg_n as f32, seg_sy / seg_n as f32));
            }
        }
        Ok((full, lines_out))
    })().map_err(|e| e.to_string());

    // ── BEGIN ocrs fallback ──────────────────────────────────────────────────
    // Remove this block when deleting ocr_fallback.rs + ocrs/rten from Cargo.toml.
    if let Err(ref e) = winrt_result {
        if e.contains("[engine]") {
            return crate::ocr_fallback::run_ocrs(&bmp, img_w, img_h);
        }
    }
    // ── END ocrs fallback ────────────────────────────────────────────────────

    winrt_result
}
