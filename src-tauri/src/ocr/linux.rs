//! Linux screen capture + OCR. Warframe runs under Proton as an X11 client,
//! so capture grabs the window over `xcb` GetImage (see `capture_warframe_bgra`)
//! and OCR runs Tesseract with bundled tessdata.

use super::preprocess_for_ocr;

// ─── Screenshot ───────────────────────────────────────────────────────────────

/// Compute average pixel brightness from a BGRA buffer (sampled every 64 pixels).
pub fn avg_brightness(pixels: &[u8]) -> u32 {
    let sum: u32 = pixels.as_chunks::<4>().0.iter().step_by(64)
        .map(|p| (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3)
        .sum();
    sum / (pixels.len() / 4 / 64).max(1) as u32
}

/// Crop a BGRA rectangle out of a full frame. All coordinates are 0.0–1.0
/// fractions.
///
/// Returns `None` when the rectangle is under four pixels on a side.
fn crop_bgra(
    pixels: &[u8], full_w: u32, full_h: u32,
    x_start: f32, x_end: f32, y_start: f32, y_end: f32,
) -> Option<(Vec<u8>, u32, u32)> {
    let col_s = (full_w as f32 * x_start.clamp(0.0, 1.0)) as usize;
    let col_e = ((full_w as f32 * x_end.clamp(0.0, 1.0)) as usize).min(full_w as usize);
    let row_s = (full_h as f32 * y_start.clamp(0.0, 1.0)) as usize;
    let row_e = ((full_h as f32 * y_end.clamp(0.0, 1.0)) as usize).min(full_h as usize);
    let rect_w = (col_e - col_s) as u32;
    let rect_h = (row_e - row_s) as u32;
    if rect_w < 4 || rect_h < 4 {
        return None;
    }
    let src_stride = full_w as usize * 4;
    let dst_stride = rect_w as usize * 4;
    let mut cropped = vec![0u8; dst_stride * rect_h as usize];
    for row in 0..rect_h as usize {
        let src = (row_s + row) * src_stride + col_s * 4;
        let dst = row * dst_stride;
        cropped[dst..dst + dst_stride].copy_from_slice(&pixels[src..src + dst_stride]);
    }
    Some((cropped, rect_w, rect_h))
}

/// Which page-segmentation mode a rectangle wants from Tesseract.
///
/// The hint is derived from the rectangle rather than passed in, because passing
/// it in means adding an argument to a signature upstream owns, at every call
/// site in `lib.rs`. Full width means the caller handed us a whole game frame —
/// the only case that wants sparse mode. Every cropped region is a panel.
///
/// ponytail: heuristic stands in for an explicit parameter. It holds while
/// "full width" and "whole frame" mean the same thing. A future caller that
/// crops vertically but keeps the full width would read as `Scattered` and get
/// nothing back; if that case appears, thread the layout through a Linux-only
/// entry point rather than widening upstream's signature.
fn layout_for(x_start: f32, x_end: f32) -> OcrLayout {
    if x_end - x_start >= 0.99 {
        OcrLayout::Scattered
    } else {
        OcrLayout::Block
    }
}

/// OCR a rectangle from a pre-captured frame, after greyscale plus a mild
/// contrast stretch. No upscaling: it distorts numerals.
pub fn ocr_pixels_rect(
    pixels: &[u8], full_w: u32, full_h: u32,
    x_start: f32, x_end: f32, y_start: f32, y_end: f32,
) -> Result<String, String> {
    let (cropped, rect_w, rect_h) =
        crop_bgra(pixels, full_w, full_h, x_start, x_end, y_start, y_end)
            .ok_or_else(|| "Region too small".to_string())?;
    let (enhanced, ew, eh) = preprocess_for_ocr(&cropped, rect_w, rect_h);
    run_ocr(&enhanced, ew, eh, layout_for(x_start, x_end)).map(|(text, _)| text)
}

/// As `ocr_pixels_rect`, but on the untouched colour pixels.
pub fn ocr_pixels_rect_raw(
    pixels: &[u8], full_w: u32, full_h: u32,
    x_start: f32, x_end: f32, y_start: f32, y_end: f32,
) -> Result<String, String> {
    let (cropped, rect_w, rect_h) =
        crop_bgra(pixels, full_w, full_h, x_start, x_end, y_start, y_end)
            .ok_or_else(|| "Region too small".to_string())?;
    run_ocr(&cropped, rect_w, rect_h, layout_for(x_start, x_end)).map(|(text, _)| text)
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

// Linux capture and OCR
// ==============================================================================
//
// Warframe runs under Proton, and on a Wayland session it is an XWayland client:
// an ordinary X11 window titled exactly "Warframe". Capture therefore goes
// through plain X11 (`xcb` GetImage on the window drawable) whether the session
// is X11 or Wayland — no desktop portal, no permission prompt, no monitor-wide
// grab, and no dependency on the compositor's screencast support.
//
// Deliberately not using a cross-platform capture crate (`xcap`) for this: its
// Linux build pulls the whole Wayland/PipeWire stack in for a code path this app
// never takes, and that stack does not currently compile against a recent
// PipeWire. The three X11 requests below are all that is actually needed.
//
// Capture is one primitive — `capture_warframe_bgra`, the window grab, in the
// BGRA byte order the shared pipeline (`to_bmp`, `preprocess_for_ocr`,
// `find_rarity_bars`, `classify_card_icon`) assumes. Everything above it —
// cropping, preprocessing, rarity bars, icon classification, catalog
// matching — feeds `run_ocr` (Tesseract) for the raw text.

/// The game's X11 window title. Warframe uses this exact string under Proton,
/// with no version suffix, which is why an equality test is safe here.
const WARFRAME_WINDOW_TITLE: &str = "Warframe";

/// Connect to the X server named by `DISPLAY`.
///
/// A fresh connection per call rather than a cached one: capture happens at most
/// a few times a second, the handshake is local-socket cheap, and a cached
/// connection would have to be re-established anyway whenever the X server or
/// XWayland restarts.
pub(crate) fn x11_connect() -> Result<xcb::Connection, String> {
    xcb::Connection::connect(None)
        .map(|(conn, _screen)| conn)
        .map_err(|e| format!("Cannot connect to the X server: {e}"))
}

/// Look up an atom by name.
fn x11_atom(conn: &xcb::Connection, name: &str) -> Result<xcb::x::Atom, String> {
    let cookie = conn.send_request(&xcb::x::InternAtom {
        only_if_exists: true,
        name: name.as_bytes(),
    });
    conn.wait_for_reply(cookie)
        .map(|reply| reply.atom())
        .map_err(|e| format!("Cannot intern the {name} atom: {e}"))
}

/// Read a window's title, preferring the EWMH `_NET_WM_NAME` (UTF-8) over the
/// legacy `WM_NAME` (Latin-1). Wine sets both; other clients may set only one.
fn x11_window_title(conn: &xcb::Connection, window: xcb::x::Window) -> Option<String> {
    let read = |property: xcb::x::Atom, r#type: xcb::x::Atom| -> Option<String> {
        let cookie = conn.send_request(&xcb::x::GetProperty {
            delete: false,
            window,
            property,
            r#type,
            long_offset: 0,
            long_length: 256,
        });
        let reply = conn.wait_for_reply(cookie).ok()?;
        let value: &[u8] = reply.value();
        (!value.is_empty()).then(|| String::from_utf8_lossy(value).into_owned())
    };

    let net_wm_name = x11_atom(conn, "_NET_WM_NAME").ok();
    let utf8_string = x11_atom(conn, "UTF8_STRING").ok();
    net_wm_name
        .zip(utf8_string)
        .and_then(|(property, r#type)| read(property, r#type))
        .or_else(|| read(xcb::x::ATOM_WM_NAME, xcb::x::ATOM_STRING))
}

/// Check if an X11 window is mapped and managed (viewable and not override-redirect).
/// Used during QueryTree fallback to filter out unmapped helper windows, popups, and tooltips.
fn x11_window_viewable(conn: &xcb::Connection, window: xcb::x::Window) -> bool {
    let cookie = conn.send_request(&xcb::x::GetWindowAttributes { window });
    conn.wait_for_reply(cookie).is_ok_and(|reply| {
        reply.map_state() == xcb::x::MapState::Viewable && !reply.override_redirect()
    })
}

/// Find the game's window. Errors when Warframe is not running, which the
/// callers surface as "capture failed" rather than as an empty frame.
///
/// Managed top-level windows are enumerated from the window manager's
/// `_NET_CLIENT_LIST_STACKING` rather than by walking the whole window tree:
/// that list holds exactly the client windows, so it cannot return a decoration
/// frame or an unmapped helper window whose title happens to match.
///
/// Window tree traversal via QueryTree is used as a fallback for window managers
/// and compositors (like Niri) that do not publish EWMH client lists.
fn warframe_window(conn: &xcb::Connection) -> Result<xcb::x::Window, String> {
    let client_list = x11_atom(conn, "_NET_CLIENT_LIST_STACKING")?;
    if client_list != xcb::x::ATOM_NONE {
        for screen in conn.get_setup().roots() {
            let cookie = conn.send_request(&xcb::x::GetProperty {
                delete: false,
                window: screen.root(),
                property: client_list,
                r#type: xcb::x::ATOM_WINDOW,
                long_offset: 0,
                long_length: 1024,
            });
            if let Ok(reply) = conn.wait_for_reply(cookie) {
                for &window in reply.value::<xcb::x::Window>() {
                    if x11_window_title(conn, window).as_deref() == Some(WARFRAME_WINDOW_TITLE) {
                        return Ok(window);
                    }
                }
            }
        }
    }

    // Fallback for Xwayland compositors (like Niri) that don't publish _NET_CLIENT_LIST_STACKING.
    // This only scans the top-level since Warframe is always found at this level.
    for screen in conn.get_setup().roots() {
        let tree_cookie = conn.send_request(&xcb::x::QueryTree {
            window: screen.root(),
        });
        if let Ok(reply) = conn.wait_for_reply(tree_cookie) {
            for &window in reply.children().iter().rev() {
                if x11_window_viewable(conn, window)
                    && x11_window_title(conn, window).as_deref() == Some(WARFRAME_WINDOW_TITLE) {
                    return Ok(window);
                }
            }
        }
    }

    Err("Warframe window not found".into())
}

/// A window's origin in root coordinates plus its size.
///
/// `GetGeometry` reports a position relative to the parent — which, for a
/// reparenting window manager, is the decoration frame rather than the desktop.
/// Translating the window's own `(x, y)` into root space and subtracting it back
/// out yields the origin of the window content itself.
fn x11_window_rect(
    conn: &xcb::Connection,
    window: xcb::x::Window,
) -> Result<(i32, i32, u32, u32), String> {
    let geometry_cookie = conn.send_request(&xcb::x::GetGeometry {
        drawable: xcb::x::Drawable::Window(window),
    });
    let geometry = conn
        .wait_for_reply(geometry_cookie)
        .map_err(|e| format!("Cannot read the Warframe window geometry: {e}"))?;

    let translate_cookie = conn.send_request(&xcb::x::TranslateCoordinates {
        src_window: window,
        dst_window: geometry.root(),
        src_x: geometry.x(),
        src_y: geometry.y(),
    });
    let translated = conn
        .wait_for_reply(translate_cookie)
        .map_err(|e| format!("Cannot translate the Warframe window position: {e}"))?;

    Ok((
        (translated.dst_x() - geometry.x()) as i32,
        (translated.dst_y() - geometry.y()) as i32,
        geometry.width() as u32,
        geometry.height() as u32,
    ))
}

/// Capture the whole game window as BGRA.
///
/// X11 `ZPixmap` data at depth 24/32 is already 4 bytes per pixel in the
/// server's byte order: B, G, R, unused on the little-endian servers this runs
/// on, which is exactly the layout the shared pipeline expects. Only the unused
/// byte needs filling, because `to_bmp` ignores it but `avg_brightness` and the
/// bar detector read whole pixels and an undefined alpha makes captures
/// non-reproducible.
#[tracing::instrument(level = "debug", skip_all)]
fn capture_warframe_bgra() -> Result<(Vec<u8>, u32, u32), String> {
    let conn = x11_connect()?;
    let window = warframe_window(&conn)?;
    let (_x, _y, width, height) = x11_window_rect(&conn, window)?;
    // A window this small is either minimised or mid-creation, and OCR on it
    // would only waste a frame.
    if width < 100 || height < 100 {
        return Err(format!("Window too small ({width}×{height})"));
    }

    let cookie = conn.send_request(&xcb::x::GetImage {
        format: xcb::x::ImageFormat::ZPixmap,
        drawable: xcb::x::Drawable::Window(window),
        x: 0,
        y: 0,
        width: width as u16,
        height: height as u16,
        plane_mask: u32::MAX,
    });
    let reply = conn
        .wait_for_reply(cookie)
        .map_err(|e| format!("Cannot capture the Warframe window: {e}"))?;

    let depth = reply.depth();
    if depth != 24 && depth != 32 {
        return Err(format!("Unsupported window depth {depth} (expected 24 or 32)"));
    }
    let expected = (width as usize) * (height as usize) * 4;
    let data = reply.data();
    if data.len() < expected {
        return Err(format!(
            "Short capture: got {} bytes for {width}×{height} (expected {expected})",
            data.len()
        ));
    }

    let mut pixels = data[..expected].to_vec();
    // MSB-first servers hand back R, G, B — rare, but a wrong guess would make
    // the rarity-bar colour tests match the wrong hues, so handle both orders.
    if conn.get_setup().image_byte_order() == xcb::x::ImageOrder::MsbFirst {
        for px in pixels.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
    }
    for px in pixels.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    Ok((pixels, width, height))
}

/// The game's window geometry in desktop coordinates, as `[x, y, w, h]`.
/// The overlay is positioned from this, so it must describe the same area the
/// capture covers — for an X11 window the capture is the whole window, and
/// Warframe under Proton is borderless, so window rect == client rect.
pub fn warframe_window_rect() -> Result<[i32; 4], String> {
    let conn = x11_connect()?;
    let window = warframe_window(&conn)?;
    let (x, y, width, height) = x11_window_rect(&conn, window)?;
    Ok([x, y, width as i32, height as i32])
}

pub fn capture_warframe_pixels() -> Result<(Vec<u8>, u32, u32), String> {
    capture_warframe_bgra()
}

/// Half-resolution capture for automatic diagnostics: a 2×2 box average, which
/// keeps small UI text legible in the saved BMP where dropping every other pixel
/// would alias it away.
pub fn capture_screen_for_diagnostics_half() -> Result<(Vec<u8>, u32, u32), String> {
    let (pixels, width, height) = capture_warframe_bgra()?;
    let (half_w, half_h) = ((width / 2).max(1), (height / 2).max(1));
    let mut out = Vec::with_capacity((half_w * half_h * 4) as usize);
    for y in 0..half_h {
        for x in 0..half_w {
            for channel in 0..4 {
                let sample = |dy: u32, dx: u32| {
                    let i = (((y * 2 + dy) * width + x * 2 + dx) * 4 + channel) as usize;
                    pixels[i] as u32
                };
                let sum = sample(0, 0) + sample(0, 1) + sample(1, 0) + sample(1, 1);
                out.push((sum / 4) as u8);
            }
        }
    }
    Ok((out, half_w, half_h))
}

/// Reward-strip capture. Only the top 80% of the window is kept: reward cards
/// never reach the lower fifth of the screen, and cropping there removes the
/// squad list and chat box from the OCR input.
#[tracing::instrument(level = "info", skip_all)]
pub fn capture_warframe_reward_area() -> Option<(Vec<u8>, u32, u32, u32, String)> {
    let (mut pixels, width, full_h) = capture_warframe_bgra().ok()?;
    let cap_h = ((full_h as f32 * 0.80) as u32).max(1);
    pixels.truncate((width * cap_h * 4) as usize);
    let avg = avg_brightness(&pixels);
    let info =
        format!("xcap/X11  {width}×{full_h}px (top 80%, cap {cap_h}px)  avg_brightness={avg}");
    Some((pixels, width, cap_h, full_h, info))
}

// ─── OCR line assembly (Linux/Tesseract) ─────────────────────────────────────
//
// Tesseract reports geometry per word in TSV. Assembling words into lines here
// yields the same (line text, position) shape the reward extractor expects.

/// One recognised word, with its horizontal extent and vertical centre already
/// expressed as fractions of the source image. Tesseract reports pixel
/// bounding boxes; normalising at the engine boundary lets the card-column
/// logic downstream stay resolution-agnostic.
pub struct OcrWord {
    pub text: String,
    pub x_left: f32,
    pub x_right: f32,
    pub cy: f32,
}

/// How much page structure the OCR engine should assume.
///
/// Tesseract picks up nothing at all on the wrong setting — a cropped riven card reads
/// perfectly as one block and returns empty as scattered text, and a full reward
/// frame does the opposite.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OcrLayout {
    /// Text scattered anywhere in a full game frame.
    Scattered,
    /// A single block of text filling a cropped region.
    Block,
}

/// Inter-card gap: reward cards are separated by ~10–12% of image width.
/// Word gaps within a single item name are ≤ 3%. Splitting at 7% cleanly
/// divides "Daikyu Prime Upper Limb Nautilus Prime Systems" (which Tesseract
/// merges into one line when the two names share a baseline Y) into the
/// two separate card entries the column-assignment logic expects.
const WORD_GAP: f32 = 0.07;

/// Turn engine lines into the `(full_text, positions)` pair the reward pipeline
/// consumes, splitting any line whose internal word gap exceeds `WORD_GAP`.
///
/// Each returned entry is `(text, x_centre, y_centre)`, averaged over the words
/// that make up that sub-line.
/// Full recognised text plus `(text, centre x, centre y)` per line segment.
pub type OcrResult = (String, Vec<(String, f32, f32)>);

fn assemble_ocr_lines(engine_lines: &[Vec<OcrWord>]) -> OcrResult {
    let mut full = String::new();
    let mut lines_out: Vec<(String, f32, f32)> = Vec::new();

    for words in engine_lines {
        // Walk words left-to-right; flush a sub-line whenever the horizontal
        // gap to the next word exceeds WORD_GAP.
        let mut seg_texts: Vec<&str> = Vec::new();
        let mut seg_sx = 0.0f32;
        let mut seg_sy = 0.0f32;
        let mut seg_n = 0u32;
        let mut prev_right = -1.0f32;
        for w in words {
            if prev_right >= 0.0 && (w.x_left - prev_right) > WORD_GAP && !seg_texts.is_empty() {
                let sub = seg_texts.join(" ");
                full.push_str(&sub);
                full.push('\n');
                lines_out.push((sub, seg_sx / seg_n as f32, seg_sy / seg_n as f32));
                seg_texts.clear();
                seg_sx = 0.0;
                seg_sy = 0.0;
                seg_n = 0;
            }
            seg_texts.push(&w.text);
            seg_sx += (w.x_left + w.x_right) / 2.0;
            seg_sy += w.cy;
            seg_n += 1;
            prev_right = w.x_right;
        }
        if !seg_texts.is_empty() {
            let sub = seg_texts.join(" ");
            full.push_str(&sub);
            full.push('\n');
            lines_out.push((sub, seg_sx / seg_n as f32, seg_sy / seg_n as f32));
        }
    }

    (full, lines_out)
}


/// Tesseract OCR entry point for a full frame. Contract: a BGRA buffer in,
/// `(full_text, per-line (text, x_centre, y_centre))` out.
///
/// Two reductions to one byte per pixel are available and the better one is
/// chosen by result, not by guesswork: the UI-colour mask is tried first and
/// plain luminance is used when it comes back empty. See `ui_text_mask`.
///
/// Word boxes come from Tesseract's TSV output, the only one of its report
/// formats that carries both per-word geometry and the block/paragraph/line
/// grouping the gap-splitting logic needs.
#[tracing::instrument(level = "debug", skip_all)]
pub fn run_ocr(
    pixels_bgra: &[u8],
    img_w: u32,
    img_h: u32,
    layout: OcrLayout,
) -> Result<OcrResult, String> {
    let expected = (img_w as usize) * (img_h as usize);
    if pixels_bgra.len() < expected * 4 {
        return Err(format!(
            "OCR buffer is {} bytes, short of the {img_w}×{img_h} BGRA claimed",
            pixels_bgra.len()
        ));
    }

    // The mask is right for interface text and blind to everything else — riven
    // card stats are drawn in an item colour no theme table can list, and callers
    // that pre-convert to greyscale (`ocr_pixels_rect`) have no colour left to
    // match. Rather than guess which case a buffer is from, read it masked and
    // keep that only if it produced text.
    if let Some(mask) = ui_text_mask(&pixels_bgra[..expected * 4]) {
        let masked = recognize_samples(&mask, img_w, img_h, layout)?;
        if reads_as_words(&masked.0) {
            return Ok(masked);
        }
    }

    let luminance: Vec<u8> = pixels_bgra[..expected * 4]
        .as_chunks::<4>().0.iter()
        .map(|px| {
            ((px[2] as u32 * 299 + px[1] as u32 * 587 + px[0] as u32 * 114) / 1000).min(255) as u8
        })
        .collect();
    recognize_samples(&luminance, img_w, img_h, layout)
}

/// Whether OCR output looks like it read text rather than shapes.
///
/// A mask that isolated nothing still leaves stray marks — panel borders, an icon
/// edge — and Tesseract reports those as one- and two-character fragments. Real
/// interface text always yields at least one run of three or more letters or
/// digits, which is also the shortest token the catalog matcher will consider.
fn reads_as_words(text: &str) -> bool {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|token| token.len() >= 3)
}

/// Directory holding the language model shipped with the app, when there is one.
///
/// Empty for a `cargo run` build, which falls back to whatever model the system
/// has installed. Every bundle carries its own copy, so this is only unset when
/// the app is run straight out of the build directory.
static BUNDLED_TESSDATA: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Point Tesseract at the app's own copy of the language model.
///
/// Without this, OCR fails to initialise on any host that has not installed
/// Tesseract's English data separately, which no Linux bundle can guarantee.
/// Called once at startup with the bundle's resource directory; a
/// directory without the model in it is ignored, so a build that skipped the
/// fetch still runs against the system copy rather than failing outright.
pub fn use_bundled_tessdata(resource_dir: &std::path::Path) {
    let dir = resource_dir.join("tessdata");
    if dir.join("eng.traineddata").is_file() {
        if let Some(dir) = dir.to_str() {
            let _ = BUNDLED_TESSDATA.set(dir.to_string());
        }
    }
}

/// Run Tesseract over one 8-bit sample per pixel.
#[tracing::instrument(level = "debug", skip_all)]
fn recognize_samples(
    samples: &[u8],
    img_w: u32,
    img_h: u32,
    layout: OcrLayout,
) -> Result<OcrResult, String> {
    use tesseract::{PageSegMode, Tesseract};

    let mut engine = Tesseract::new(BUNDLED_TESSDATA.get().map(String::as_str), Some("eng"))
        .map_err(|e| format!("Cannot initialise Tesseract (is eng.traineddata installed?): {e}"))?;

    engine.set_page_seg_mode(match layout {
        OcrLayout::Scattered => PageSegMode::PsmSparseText,
        OcrLayout::Block => PageSegMode::PsmSingleBlock,
    });

    let mut engine = engine
        .set_frame(samples, img_w as i32, img_h as i32, 1, img_w as i32)
        .map_err(|e| format!("Tesseract rejected the captured frame: {e}"))?
        .recognize()
        .map_err(|e| format!("Tesseract recognition failed: {e}"))?;
    let tsv = engine
        .get_tsv_text(0)
        .map_err(|e| format!("Cannot read Tesseract TSV output: {e}"))?;

    Ok(assemble_ocr_lines(&parse_tesseract_tsv(&tsv, img_w, img_h)))
}

/// Warframe's UI text colours: the `primary` and `secondary` of every built-in
/// interface theme, as RGB. Ported from wfinfo-ng's theme table.
///
/// The game draws interface text in exactly one of these colours, unblended, so
/// an equality test against the whole set isolates text without having to know
/// which theme the player has selected.
const UI_TEXT_COLOURS: &[(u8, u8, u8)] = &[
    (190, 169, 102), (245, 227, 173), // Vitruvian
    (153, 31, 35),   (255, 61, 51),   // Stalker
    (238, 193, 105), (236, 211, 162), // Baruuk
    (35, 201, 245),  (111, 229, 253), // Corpus
    (57, 105, 192),  (255, 115, 230), // Fortuna
    (255, 189, 102), (255, 224, 153), // Grineer
    (36, 184, 242),  (255, 241, 191), // Lotus
    (140, 38, 92),   (245, 73, 93),   // Nidus
    (20, 41, 29),    (178, 125, 5),   // Orokin
    (9, 78, 106),    (6, 106, 74),    // Tenno
    (2, 127, 217),   (255, 255, 0),   // High Contrast
    (255, 255, 255), (232, 213, 93),  // Legacy
    (158, 159, 167), (232, 227, 227), // Equinox
    (140, 119, 147), (189, 169, 237), // Dark Lotus
    (253, 132, 2),   (255, 53, 0),    // Zephyr
];

/// Isolate interface text: pixels that exactly match a theme text colour become
/// black, everything else white. `None` when no pixel matched, so the caller can
/// skip an OCR pass it knows will come back blank.
///
/// Tesseract is a document OCR engine — handed a raw game frame it tries to read
/// the artwork too, and on a 4K reward screen that buries four item names in
/// ~200 lines of noise. Deleting everything that is not interface text is the
/// difference between mostly-garbage and near-perfect names on real captures.
fn ui_text_mask(pixels_bgra: &[u8]) -> Option<Vec<u8>> {
    let mut matched = false;
    let mask: Vec<u8> = pixels_bgra
        .as_chunks::<4>().0.iter()
        .map(|px| {
            let is_ui_text = UI_TEXT_COLOURS
                .iter()
                .any(|&(r, g, b)| px[2] == r && px[1] == g && px[0] == b);
            matched |= is_ui_text;
            if is_ui_text { 0 } else { 255 }
        })
        .collect();
    matched.then_some(mask)
}

/// Group Tesseract's TSV rows into per-line word lists.
///
/// Columns are `level page block paragraph line word left top width height conf
/// text`; level 5 is a word and every coarser level repeats the same geometry
/// with `conf` = -1, so filtering on level keeps each word exactly once.
///
/// Lines are emitted in top-to-bottom, left-to-right order and their words in
/// left-to-right order; the full-frame fallback in
/// `extract_reward_items_twophase` uses line index as a stand-in for screen
/// position — sparse-text mode makes no ordering promise, so the order is
/// imposed here instead.
/// `(top, left, word)` in pixels, so sorting orders words within a line.
type PlacedWord = (i32, i32, OcrWord);

fn parse_tesseract_tsv(tsv: &str, img_w: u32, img_h: u32) -> Vec<Vec<OcrWord>> {
    // Zero dimensions would make every fraction a division by zero; the callers
    // reject sub-4-pixel rects, so this only guards against a degenerate BMP.
    if img_w == 0 || img_h == 0 {
        return Vec::new();
    }

    // Keyed by (block, paragraph, line) so words from two different text regions
    // that happen to share a baseline stay in separate lines.
    let mut lines: std::collections::BTreeMap<(i32, i32, i32), Vec<PlacedWord>> =
        std::collections::BTreeMap::new();

    for row in tsv.lines() {
        let fields: Vec<&str> = row.split('\t').collect();
        if fields.len() < 12 || fields[0] != "5" {
            continue;
        }
        let num = |i: usize| fields[i].parse::<i32>().ok();
        let (Some(block), Some(par), Some(line)) = (num(2), num(3), num(4)) else {
            continue;
        };
        let (Some(left), Some(top), Some(width), Some(height)) =
            (num(6), num(7), num(8), num(9))
        else {
            continue;
        };
        // Tesseract emits empty words for regions it segmented but could not
        // read; they carry no text for matching and would only widen word gaps.
        let text = fields[11].trim();
        if text.is_empty() {
            continue;
        }
        lines.entry((block, par, line)).or_default().push((
            top,
            left,
            OcrWord {
                text: text.to_owned(),
                x_left: left as f32 / img_w as f32,
                x_right: (left + width) as f32 / img_w as f32,
                cy: (top as f32 + height as f32 / 2.0) / img_h as f32,
            },
        ));
    }

    let mut ordered: Vec<(i32, i32, Vec<OcrWord>)> = lines
        .into_values()
        .filter_map(|mut words| {
            words.sort_by_key(|(_, left, _)| *left);
            let top = words.iter().map(|(top, _, _)| *top).min()?;
            let left = words.first().map(|(_, left, _)| *left)?;
            Some((top, left, words.into_iter().map(|(_, _, w)| w).collect()))
        })
        .collect();
    ordered.sort_by_key(|(top, left, _)| (*top, *left));
    ordered.into_iter().map(|(_, _, words)| words).collect()
}

#[cfg(test)]
mod tesseract_tests {
    use super::*;

    /// Two reward cards whose names share a baseline land in one Tesseract line.
    /// The pipeline downstream assigns text to cards by X position, so the
    /// gap-splitting in `assemble_ocr_lines` is what keeps the two names from
    /// being scored as a single item — and the TSV parser is what gives it the
    /// geometry to split on.
    #[test]
    fn tsv_words_split_into_one_sub_line_per_card() {
        // level page block par line word left top width height conf text
        // 1000 px wide: "Daikyu Prime" sits at x 100–300, "Nautilus Prime" at
        // 700–950 — a 40% gap, far beyond the 7% inter-card threshold.
        let tsv = "\
1\t1\t0\t0\t0\t0\t0\t0\t1000\t500\t-1\t\n\
5\t1\t1\t1\t1\t1\t100\t200\t80\t20\t92\tDaikyu\n\
5\t1\t1\t1\t1\t2\t200\t200\t100\t20\t90\tPrime\n\
5\t1\t1\t1\t1\t3\t700\t200\t110\t20\t88\tNautilus\n\
5\t1\t1\t1\t1\t4\t830\t200\t120\t20\t91\tPrime\n";

        let lines = parse_tesseract_tsv(tsv, 1000, 500);
        assert_eq!(lines.len(), 1, "all four words share one TSV line");

        let (full, positions) = assemble_ocr_lines(&lines);
        assert_eq!(full, "Daikyu Prime\nNautilus Prime\n");
        assert_eq!(positions.len(), 2);
        assert_eq!(positions[0].0, "Daikyu Prime");
        assert_eq!(positions[1].0, "Nautilus Prime");
        // Left card centres in the left half, right card in the right half.
        assert!(positions[0].1 < 0.4, "left centre was {}", positions[0].1);
        assert!(positions[1].1 > 0.6, "right centre was {}", positions[1].1);
        // Both baselines are at y 200–220 of 500 → ~0.42.
        assert!((positions[0].2 - 0.42).abs() < 0.01);
    }

    /// The mask is what makes Tesseract usable on a game frame, and the word
    /// check is what stops a mask that isolated nothing from being trusted over
    /// the greyscale fallback. Both directions matter, so both are pinned here.
    #[test]
    fn the_ui_colour_mask_keeps_interface_text_and_drops_artwork() {
        // Two pixels of Zephyr-theme text, one of artwork that is merely close to
        // it, and one unrelated. BGRA order, so the tuples read blue-first.
        let pixels: Vec<u8> = [
            [2u8, 132, 253, 255],  // Zephyr primary  (253,132,2) → text
            [0, 53, 255, 255],     // Zephyr secondary (255,53,0) → text
            [3, 133, 252, 255],    // one off in every channel → artwork
            [40, 30, 20, 255],     // dark background → artwork
        ]
        .concat();
        assert_eq!(ui_text_mask(&pixels), Some(vec![0, 0, 255, 255]));

        // Nothing to isolate: the caller must not waste an OCR pass on a blank.
        assert_eq!(ui_text_mask(&[40, 30, 20, 255, 41, 31, 21, 255]), None);

        // Stray marks from a mask that caught only a panel border read as
        // one- and two-character fragments; real interface text does not.
        assert!(!reads_as_words("| - \n{ ,\n1,"));
        assert!(reads_as_words("| Gauss Prime Chassis"));
        assert!(reads_as_words("MR11"));
    }
}
