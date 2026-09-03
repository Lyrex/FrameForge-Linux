//! Screen capture + OCR for Warframe relic reward detection.
//!
//! Warframe runs under Proton as an X11 client, so capture grabs the window
//! over `xcb` GetImage (see `capture_warframe_bgra`) and OCR runs Tesseract
//! with bundled tessdata.

// ─── Screenshot ───────────────────────────────────────────────────────────────

/// Compute average pixel brightness from a BGRA buffer (sampled every 64 pixels).
fn avg_brightness(pixels: &[u8]) -> u32 {
    let sum: u32 = pixels.chunks_exact(4).step_by(64)
        .map(|p| (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3)
        .sum();
    sum / (pixels.len() / 4 / 64).max(1) as u32
}

/// Grayscale + contrast stretch on BGRA pixels.
/// Converting to grayscale is the key step: element icons (❄ Cold, 🔥 Heat, ☠ Toxin)
/// are colored glyphs — OCR rejects these lines as graphics. After grayscale
/// they become neutral-brightness shapes, so OCR reads the white text on either
/// side of the icon instead of dropping the whole line.
fn preprocess_for_ocr(pixels: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let mut out = pixels.to_vec();
    for px in out.chunks_mut(4) {
        // Standard luminance: 0.299 R + 0.587 G + 0.114 B (BGRA order)
        let gray = ((px[2] as u32 * 299 + px[1] as u32 * 587 + px[0] as u32 * 114) / 1000)
            .min(255) as u8;
        // Mild contrast stretch [20, 235] → [0, 255]
        let v = ((gray as i32 - 20) * 255 / 215).clamp(0, 255) as u8;
        px[0] = v;
        px[1] = v;
        px[2] = v;
    }
    (out, width, height)
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

// ─── BMP encoding ─────────────────────────────────────────────────────────────

/// Encode BGRA pixels as a 24-bit BGR BMP.
pub fn to_bmp(pixels_bgra: &[u8], width: u32, height: u32) -> Vec<u8> {
    let row_bytes = width * 3;
    let padding   = (4 - row_bytes % 4) % 4;
    let row_stride = row_bytes + padding;
    let image_size = row_stride * height;
    let file_size  = 54 + image_size;

    let mut bmp = Vec::with_capacity(file_size as usize);
    // File header
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&file_size.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&54u32.to_le_bytes());
    // Info header
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(width as i32).to_le_bytes());
    bmp.extend_from_slice(&(-(height as i32)).to_le_bytes()); // top-down
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&24u16.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    bmp.extend_from_slice(&image_size.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    // Pixel rows (BGRA → BGR + padding)
    for row in 0..height {
        for col in 0..width {
            let i = ((row * width + col) * 4) as usize;
            bmp.push(pixels_bgra[i]);
            bmp.push(pixels_bgra[i + 1]);
            bmp.push(pixels_bgra[i + 2]);
        }
        for _ in 0..padding { bmp.push(0); }
    }
    bmp
}

// ─── Word matching helpers ────────────────────────────────────────────────────

fn lev_dist(a: &str, b: &str) -> usize {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let (m, n) = (a.len(), b.len());
    if m.abs_diff(n) > 3 { return 99; }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            curr[j] = if a[i-1] == b[j-1] { prev[j-1] }
                      else { 1 + prev[j].min(curr[j-1]).min(prev[j-1]) };
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

/// Check whether `catalog_word` appears in `ocr_words` via:
///   1. Exact match
///   2. Prefix match: OCR truncated ("prime"→"pri", "voruna"→"vor")
///   3. Suffix substring: "neuroptics" → OCR gives "rüroptics"/"tearoptics" which
///      both contain "optics" — the distinctive suffix is preserved even when the
///      prefix is garbled. Check last 5+ chars as a substring in any OCR word.
///   4. Levenshtein ≤ 1 (or ≤ 2 for ≥8-char words) for single-char typos
///   5. Sliding-window inside longer merged tokens ("Sevagotfirime")
fn word_found_in_set(
    catalog_word: &str,
    ocr_words: &std::collections::HashSet<String>,
) -> bool {
    if ocr_words.contains(catalog_word) { return true; }
    if catalog_word.len() < 4 { return false; }

    // Prefix: OCR word is the leading portion of the catalog word
    for ocr_w in ocr_words {
        if ocr_w.len() >= 3 && catalog_word.starts_with(ocr_w.as_str()) { return true; }
    }

    // Suffix substring: check if last N chars of catalog word appear inside any OCR word
    // Handles "neuroptics" → "rüroptics" because both contain "optics"
    // Guard: reject when the suffix appears at exactly position 1 — that means an OCR
    // word is a prefix-stripped version of the catalog word (e.g. "bronco" contains
    // suffix "ronco" of "akbronco" at position 1, which is a false positive).
    if catalog_word.len() >= 6 {
        let suffix_len = (catalog_word.len() / 2).max(5); // half the word, min 5 chars
        let suffix = &catalog_word[catalog_word.len() - suffix_len..];
        if ocr_words.iter().any(|w| w.find(suffix).map_or(false, |p| p != 1)) { return true; }
    }

    // Edit budget by word length. 4 chars is the shortest word this fuzzy-matches
    // at all; below that the guard above has already returned. One edit in a 4-char
    // word is a quarter of it, enough to land on a different real catalog word
    // instead of a damaged read of this one: "limb" reaches "limbo", "gara" reaches
    // "galatine", "khra" reaches "khora", "star" reaches "stars". Those short words
    // are usually the only part of a reward name that identifies it, so a wrong hit
    // scores a full catalog entry for an item that was never on screen.
    let max_dist = if catalog_word.len() >= 8 {
        2
    } else if catalog_word.len() >= 5 {
        1
    } else {
        0
    };
    let wb = catalog_word.as_bytes();
    for ocr_w in ocr_words {
        // Full-word Levenshtein — reject pure prefix/suffix insertions (len_diff == dist && >= 2)
        // e.g. dist("akbronco","bronco")=2 with len_diff=2 is just "ak" prepended, not a typo.
        // Also require OCR word ≥4 chars: 3-char HUD noise ("RAM","FPS","GPU") must not
        // fuzzy-match 4-char catalog words ("gram","fang"…) regardless of screen position.
        if ocr_w.len() >= 4 {
            let dist = lev_dist(catalog_word, ocr_w);
            let len_diff = (catalog_word.len() as isize - ocr_w.len() as isize).unsigned_abs();
            if dist <= max_dist && !(len_diff == dist && len_diff >= 2) { return true; }
        }
        // Sliding window (merged tokens — e.g. OCR reads "SevagothPrime" as one word).
        // Require the OCR token to be at least 4 chars longer than the catalog word so
        // standalone words that differ by 1 char (e.g. "band" ↔ "hand" inside "handle")
        // don't produce false matches. Genuine merges are always 4+ chars longer.
        let ob = ocr_w.as_bytes();
        if ob.len() >= wb.len() + 4 {
            for (win_start, win) in ob.windows(wb.len()).enumerate() {
                let errs = wb.iter().zip(win.iter()).filter(|(a, b)| a != b).count();
                // Guard: reject exact suffix matches where the catalog word cleanly
                // terminates a longer OCR word (e.g. "fang" ending "sarofang").
                // "Sarofang" is a single correctly-read word; "fang" appearing at its
                // tail is a lexical coincidence, not an OCR merge artifact.
                // Only guard exact matches (errs == 0) — fuzzy matches are always valid.
                // win_start >= 3 avoids blocking short prefixes like "ak" in "akbronco".
                if errs == 0 && win_start + wb.len() == ob.len() && win_start >= 3 { continue; }
                if errs <= max_dist { return true; }
            }
        }
    }
    false
}

// ─── Catalog matching ─────────────────────────────────────────────────────────

/// Normalise OCR text for catalog matching.
/// ASCII letters are lowercased. Common diacritics are mapped to their ASCII
/// base (é→e, ü→u, …) so fuzzy matching still works when OCR returns
/// accented surrogates instead of plain letters. Everything else → space.
fn normalise(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii() { return c.to_ascii_lowercase(); }
            match c {
                'À'|'Á'|'Â'|'Ã'|'Ä'|'Å'|'à'|'á'|'â'|'ã'|'ä'|'å' => 'a',
                'È'|'É'|'Ê'|'Ë'|'è'|'é'|'ê'|'ë' => 'e',
                'Ì'|'Í'|'Î'|'Ï'|'ì'|'í'|'î'|'ï' => 'i',
                'Ò'|'Ó'|'Ô'|'Õ'|'Ö'|'ò'|'ó'|'ô'|'õ'|'ö' => 'o',
                'Ù'|'Ú'|'Û'|'Ü'|'ù'|'ú'|'û'|'ü' => 'u',
                'Ñ'|'ñ' => 'n',
                'Ç'|'ç' => 'c',
                'Ý'|'ý'|'ÿ' => 'y',
                _ => ' ',
            }
        })
        .collect()
}

// ─── Rarity bar detection ─────────────────────────────────────────────────────

/// Scan the captured image for the coloured rarity bars below each reward card.
/// Returns (card_x_centers, bar_y_frac) where centers are fractions of image width.
///
/// Uses column aggregation: for each X column, count how many rows in the search
/// band have bar-coloured pixels. Columns that are consistently orange or teal
/// across many rows score high. This is far more robust than row-by-row detection
/// because it tolerates thin bars, color gradients, and single-row noise.
/// Returns `(Some((centers, bar_y_frac)), diagnostic_string)`.
/// `centers` are fractions of image width — the diamond icon X per card.
/// The diagnostic string is always populated for session log inclusion.
fn find_rarity_bars(pixels: &[u8], pix_w: u32, pix_h: u32) -> (Option<(Vec<f32>, f32)>, String) {
    let x_lo = (pix_w as f32 * 0.05) as u32;
    let x_hi = (pix_w as f32 * 0.95) as u32;
    // In the full 48% capture (no crops), bar position varies with UI scale:
    //   • Large window / small UI (e.g. 1440p native): bars at ~65–70% of cap_h
    //   • Smaller window / larger UI: bars at ~88–93% of cap_h
    // Starting at 55% skips the card artwork (weapon/helmet icons) while
    // catching both configurations. Upper bound stays at 0.97 to cover all cases.
    let y_lo = (pix_h as f32 * 0.55) as u32;
    let y_hi = (pix_h as f32 * 0.97) as u32;

    let scan_w = (x_hi - x_lo) as usize;

    // Rarity colours (BGRA). Permissive — Warframe's UI
    // background is very dark (avg_brightness often 30–40), so bar pixels can
    // be quite dim. The diamond/arrow icon at each card's centre is near-white.
    //   Orange/bronze : R dominant over B
    //   Silver/teal   : B/G dominant, cool cast
    //   Gold/rare     : warm, R > G > B
    //   Diamond icon  : near-white, brightest point in the bar
    #[inline]
    fn is_bar_pixel(b: u32, g: u32, r: u32) -> bool {
        let lum = (r + g + b) / 3;
        if lum < 25 { return false; }
        let is_orange = r > 80  && r > b + 20;
        let is_teal   = b > 65  && g > 50  && b > r + 8;
        let is_gold   = r > 100 && g > 80  && b < r.saturating_sub(10);
        // Near-white only — card artwork at 1440p has mid-lum coloured pixels that
        // trigger the old r/g/b>70 check and create false bar peaks.
        let max_ch = r.max(g).max(b);
        let min_ch = r.min(g).min(b);
        let is_bright = lum > 160 && max_ch - min_ch < 50;
        is_orange || is_teal || is_gold || is_bright
    }

    // ── Step 1: Column projection ────────────────────────────────────────────
    //
    // For each X column sum how many rows in the search band contain a
    // bar-coloured pixel.  Accumulating vertically makes this robust to:
    //   • Thin bars    — even a 1-px-tall bar contributes to every column it covers
    //   • Small icons  — the rarity diamond is only ~20-30 px wide but several
    //                    rows tall; rows accumulate into a clear column peak
    //   • Colour noise — one mis-classified pixel doesn't ruin a whole column
    //
    // The previous per-row scan required ≥25 % of scan width (~430 px) lit in a
    // SINGLE row.  With only the small diamond icons present (~4 × 25 px = 100 px)
    // NO row ever reached that threshold → "0 coloured rows" in the log.
    let mut col_score = vec![0u32; scan_w];
    for y in y_lo..y_hi {
        for (xi, x) in (x_lo..x_hi).enumerate() {
            let i = ((y * pix_w + x) * 4) as usize;
            if i + 2 < pixels.len()
                && is_bar_pixel(pixels[i] as u32, pixels[i+1] as u32, pixels[i+2] as u32)
            {
                col_score[xi] += 1;
            }
        }
    }

    let max_col = col_score.iter().max().copied().unwrap_or(0);
    if max_col < 2 {
        return (None, format!(
            "no bars — column projection: max_col={} (need ≥2; y={:.0}–{:.0}%)",
            max_col,
            y_lo as f32 / pix_h as f32 * 100.0,
            y_hi as f32 / pix_h as f32 * 100.0,
        ));
    }

    // ── Step 2: Threshold + gap bridging + segment counting ──────────────────
    //
    // A column is "lit" when its score ≥ max_col/4.
    // Relative threshold handles both full-width bars (many columns, lower peak)
    // and icon-only bars (few columns but a taller, sharper peak).
    let col_threshold = (max_col / 4).max(2);
    let mut lit: Vec<bool> = col_score.iter().map(|&s| s >= col_threshold).collect();

    // Bridge tiny dark notches within one arrow (≤1 % of scan width).
    // Inter-card gaps are ~10 % of scan width and will NOT be bridged.
    let bridge = (scan_w / 100).max(3);
    {
        let mut xi = 0;
        while xi < scan_w {
            if !lit[xi] {
                let gap_start = xi;
                while xi < scan_w && !lit[xi] { xi += 1; }
                let gap_len = xi - gap_start;
                if gap_len <= bridge && gap_start > 0 && xi < scan_w {
                    for gxi in gap_start..xi { lit[gxi] = true; }
                }
            } else {
                xi += 1;
            }
        }
    }

    // Each continuous lit segment = one rarity bar = one reward card.
    // The rarity indicator is a small downward-pointing arrow (~30 px wide at 1080p).
    // min_band = 0.7% of scan width — passes arrows of ~10 px and above.
    let min_band = (scan_w / 150).max(6);
    let mut bands: Vec<(usize, usize)> = Vec::new();
    let mut in_band = false;
    let mut band_start = 0usize;
    for xi in 0..scan_w {
        match (lit[xi], in_band) {
            (true,  false) => { band_start = xi; in_band = true; }
            (false, true)  => {
                if xi - band_start >= min_band { bands.push((band_start, xi)); }
                in_band = false;
            }
            _ => {}
        }
    }
    if in_band && scan_w - band_start >= min_band { bands.push((band_start, scan_w)); }

    let lit_count = lit.iter().filter(|&&b| b).count();
    if bands.is_empty() {
        return (None, format!(
            "no bars — {} lit columns (threshold={}/{}), no segment ≥{}px (bridge={}px)",
            lit_count, col_threshold, max_col, min_band, bridge
        ));
    }
    if bands.len() > 4 {
        return (None, format!(
            "no bars — {} segments after bridging (expected 1–4); max_col={}, threshold={}",
            bands.len(), max_col, col_threshold
        ));
    }

    // ── Step 3: Bar Y position (for icon classifier) ─────────────────────────
    //
    // Restrict the row scan to lit X columns only, then find the row with the
    // most bar pixels.  classify_card_icon uses bar_y to locate the icon region
    // above the rarity bar for each card.
    let lit_xs: Vec<u32> = (0..scan_w as u32)
        .filter(|&xi| lit[xi as usize])
        .map(|xi| x_lo + xi)
        .collect();

    let mut best_row_y = (y_lo + y_hi) / 2; // fallback: geometric centre
    let mut best_row_cnt = 0u32;
    for y in y_lo..y_hi {
        let mut cnt = 0u32;
        for &x in &lit_xs {
            let i = ((y * pix_w + x) * 4) as usize;
            if i + 2 < pixels.len()
                && is_bar_pixel(pixels[i] as u32, pixels[i+1] as u32, pixels[i+2] as u32)
            {
                cnt += 1;
            }
        }
        if cnt > best_row_cnt { best_row_cnt = cnt; best_row_y = y; }
    }

    // ── Step 4: Card X center — peak column within each band ─────────────────
    //
    // The diamond/arrow icon sits at the exact centre of each card.
    // The column with the highest accumulated score within each band is the
    // most reliably lit X → use it as the card center.
    let centers: Vec<f32> = bands.iter().map(|(s, e)| {
        let best_xi = (*s..*e)
            .max_by_key(|&xi| col_score[xi])
            .unwrap_or((s + e) / 2);
        (x_lo as f32 + best_xi as f32) / pix_w as f32
    }).collect();

    let bar_y = best_row_y as f32 / pix_h as f32;
    let diag = format!(
        "{} bars — centers x=[{}], bar_y={:.2} ({:.0}%), max_col={}px, threshold={}px, lit={}px",
        bands.len(),
        centers.iter().map(|x| format!("{:.3}", x)).collect::<Vec<_>>().join(", "),
        bar_y, bar_y * 100.0, max_col, col_threshold, lit_count,
    );
    (Some((centers, bar_y)), diag)
}

// ─── Icon component classifier ────────────────────────────────────────────────

/// What the card icon looks like, used to constrain catalog matching.
#[derive(Debug, Clone, PartialEq)]
pub enum IconType {
    /// Generic REUSED component shape — same icon appears across many primes.
    /// e.g. all neuroptics share the same helmet silhouette, all barrels look alike.
    /// The TEXT below identifies WHICH prime it belongs to.
    Component(&'static str), // "neuroptics" | "systems" | "chassis" |
                              // "barrel" | "stock" | "receiver" | "handle" |
                              // "blade" | "grip" | "upper limb" | "lower limb"
    /// Full 3D model of a unique warframe or weapon.
    /// Every prime has its own unique render → card always shows "[Name] Prime Blueprint".
    /// The TEXT (or partial text) gives us the [Name].
    FullModel,
    /// Forma spiral (distinctively blue)
    Forma,
    /// Could not classify
    Unknown,
}

/// Classify the reward card icon using an 8×8 spatial brightness grid.
///
/// Features extracted:
///   fill_ratio — fraction of grid cells above threshold (dense = full model)
///   aspect     — bounding-box width / height (> 1 wide, < 1 tall)
///   cm_y       — vertical centre-of-mass (0 = top, 1 = bottom)
///   symmetry   — left / right balance (1 = symmetric)
///   blue_dom   — blue channel dominance (Forma indicator)
///
/// Rule set (in priority order):
///   ① Forma        — blue channel dominates → blue spiral icon
///   ② FullModel    — high fill + even spread → complete warframe/weapon render;
///                    text gives "[Name] Prime Blueprint"
///   ③ neuroptics   — bright top half, symmetric, roughly square (helmet shape)
///   ④ systems      — bright central region, compact, somewhat circular (gear)
///   ⑤ chassis      — large central region, wider, lower CoM (torso)
///   ⑥ barrel       — wide aspect ratio (elongated horizontal part)
///   ⑦ handle       — tall aspect ratio (elongated vertical / melee handle)
///   ⑧ blade        — low symmetry, moderate aspect (flat asymmetric part)
///   ⑨ upper/lower limb — low fill, arc-shaped (bow components)
///   Unknown        — ambiguous; fall back to text-only matching
fn classify_card_icon(
    pixels: &[u8], pix_w: u32, pix_h: u32,
    x_left: f32, x_right: f32, bar_y: f32,
) -> IconType {
    // Card icon sits between the card top and the rarity bar.
    // In the capture buffer the icon occupies roughly bar_y-0.28 → bar_y-0.04.
    let iy_top = ((bar_y - 0.28).max(0.0) * pix_h as f32) as u32;
    let iy_bot = ((bar_y - 0.04).min(1.0) * pix_h as f32) as u32;
    let ix_lo  = (x_left  * pix_w as f32) as u32;
    let ix_hi  = (x_right * pix_w as f32).min(pix_w as f32) as u32;
    if ix_hi <= ix_lo || iy_bot <= iy_top { return IconType::Unknown; }

    const G: usize = 8;
    let mut lum  = [[0.0f32; G]; G];
    let mut blue = [[0.0f32; G]; G];
    let mut cnt  = [[0u32;  G]; G];

    for y in iy_top..iy_bot {
        let gy = (((y - iy_top) as f32 / (iy_bot - iy_top) as f32) * G as f32)
                     .min(G as f32 - 1.0) as usize;
        for x in ix_lo..ix_hi {
            let gx = (((x - ix_lo) as f32 / (ix_hi - ix_lo) as f32) * G as f32)
                         .min(G as f32 - 1.0) as usize;
            let i = ((y * pix_w + x) * 4) as usize;
            if i + 2 >= pixels.len() { continue; }
            let b = pixels[i]     as f32;
            let g = pixels[i + 1] as f32;
            let r = pixels[i + 2] as f32;
            lum [gy][gx] += (r + g + b) / 3.0;
            blue[gy][gx] += b;
            cnt [gy][gx] += 1;
        }
    }
    for gy in 0..G { for gx in 0..G {
        let c = cnt[gy][gx];
        if c > 0 { lum[gy][gx] /= c as f32; blue[gy][gx] /= c as f32; }
    }}

    let avg_lum  = lum.iter().flatten().sum::<f32>()  / (G*G) as f32;
    let avg_blue = blue.iter().flatten().sum::<f32>() / (G*G) as f32;

    // ① Forma: blue channel clearly stronger than average luminance
    if avg_blue > 75.0 && avg_blue > avg_lum * 1.35 { return IconType::Forma; }

    // Threshold: cells are "bright" if > 40 % of the peak cell
    let peak = lum.iter().flatten().cloned().fold(0.0f32, f32::max);
    let thr  = peak * 0.40;

    let mut bright_rows = [false; G];
    let mut bright_cols = [false; G];
    let mut n_bright = 0usize;
    let mut cx_sum   = 0.0f32;
    let mut cy_sum   = 0.0f32;

    for gy in 0..G { for gx in 0..G {
        if lum[gy][gx] > thr {
            bright_rows[gy] = true;
            bright_cols[gx] = true;
            n_bright += 1;
            cx_sum += gx as f32;
            cy_sum += gy as f32;
        }
    }}
    if n_bright == 0 { return IconType::Unknown; }

    // Centre-of-mass (0 = top/left, 1 = bottom/right)
    let cm_x = cx_sum / n_bright as f32 / (G-1) as f32;
    let cm_y = cy_sum / n_bright as f32 / (G-1) as f32;

    // Bounding box of bright region
    let row_lo = bright_rows.iter().position(|&b| b).unwrap_or(0)    as f32 / (G-1) as f32;
    let row_hi = bright_rows.iter().rposition(|&b| b).unwrap_or(G-1) as f32 / (G-1) as f32;
    let col_lo = bright_cols.iter().position(|&b| b).unwrap_or(0)    as f32 / (G-1) as f32;
    let col_hi = bright_cols.iter().rposition(|&b| b).unwrap_or(G-1) as f32 / (G-1) as f32;

    let bb_h   = (row_hi - row_lo).max(0.01);
    let bb_w   = (col_hi - col_lo).max(0.01);
    let aspect = bb_w / bb_h;            // > 1 wide,  < 1 tall
    let fill   = n_bright as f32 / (G*G) as f32;  // 0 – 1

    // Left / right symmetry score
    let l: f32 = (0..G).map(|gy| (0..G/2).map(|gx| lum[gy][gx]).sum::<f32>()).sum();
    let r: f32 = (0..G).map(|gy| (G/2..G).map(|gx| lum[gy][gx]).sum::<f32>()).sum();
    let symmetry = 1.0 - (l - r).abs() / (l + r + 0.001);

    let _ = cm_x; // reserved for future use

    // ② FullModel — complete warframe pose or full weapon render.
    //    Fills the card frame densely and relatively evenly.
    //    Text below gives "[Name]" → result is "[Name] Prime Blueprint".
    if fill > 0.55 && avg_lum > 70.0 { return IconType::FullModel; }

    // ③ Neuroptics — helmet silhouette, rounded top.
    //    CoM upper half, symmetric left/right, roughly square bounding box.
    if cm_y < 0.45 && symmetry > 0.72 && (0.5..=2.0).contains(&aspect) {
        return IconType::Component("neuroptics");
    }

    // ④ Systems — round mechanical ring / gear.
    //    Central CoM, compact, relatively symmetric and circular.
    if cm_y > 0.35 && cm_y < 0.65 && symmetry > 0.68 && (0.6..=1.7).contains(&aspect) && fill > 0.20 {
        return IconType::Component("systems");
    }

    // ⑤ Chassis — larger torso / body piece.
    //    CoM centre-to-low, more filled, wider than neuroptics.
    if cm_y > 0.42 && fill > 0.28 && (0.7..=2.2).contains(&aspect) {
        return IconType::Component("chassis");
    }

    // ⑥ Barrel / Stock / Receiver — elongated horizontal.
    //    Bounding box much wider than tall (aspect > 2).
    if aspect > 2.0 { return IconType::Component("barrel"); }

    // ⑦ Handle / Grip — elongated vertical (melee handle).
    //    Bounding box much taller than wide (aspect < 0.5).
    if aspect < 0.5 { return IconType::Component("handle"); }

    // ⑧ Blade — flat, angular, asymmetric.
    //    Moderate aspect but low left/right symmetry.
    if symmetry < 0.60 && (0.7..=3.0).contains(&aspect) {
        return IconType::Component("blade");
    }

    // ⑨ Upper / Lower Limb — curved bow piece (arc = low fill, hollow centre).
    if fill < 0.22 && (0.7..=2.5).contains(&aspect) {
        return if cm_y < 0.50 {
            IconType::Component("upper limb")
        } else {
            IconType::Component("lower limb")
        };
    }

    IconType::Unknown
}

/// Given a word set from OCR text, extract the most likely item NAME
/// (strip known non-name words: "prime", "blueprint", component names, "owned", etc.)
fn extract_item_name_words(words: &std::collections::HashSet<String>) -> Vec<String> {
    const SKIP: &[&str] = &[
        "prime", "blueprint", "owned", "crafted", "bl", "neuroptics", "systems",
        "chassis", "barrel", "stock", "receiver", "handle", "blade", "grip",
        "limb", "upper", "lower", "string", "link", "carapace", "cerebrum",
        "forma", "riven", "sliver", "ayatan",
    ];
    words.iter()
        .filter(|w| w.len() >= 3 && !SKIP.contains(&w.as_str()))
        .cloned()
        .collect()
}

/// Sanity-check detected bar centers.
/// Rejects detections caused by card artwork (orange forma gear, gold weapons)
/// which produce centers that are bunched together or out of range.
/// Valid 4-card centers span ~0.52 (e.g. 0.24→0.76); false-positive clusters
/// span much less (e.g. 0.372→0.706 = 0.334, seen with forma-heavy rewards).
fn bar_centers_are_valid(centers: &[f32]) -> bool {
    let n = centers.len();
    if n == 0 { return false; }
    // Outermost centers must be in a plausible screen zone.
    // Upper bound raised to 0.90 — at 2560×1440 the rightmost card genuinely
    // lands past 0.85 (observed at x=0.862 in a live session log).
    if centers[0] < 0.15 || centers[n - 1] > 0.90 { return false; }
    if n < 2 { return true; }
    // Reject if any two adjacent bars are closer than 0.08.
    // The expected gap between cards is ~0.17 (4-card layout).
    // Bars within 0.08 of each other are a double-detection of the same bar
    // or a false positive from card artwork — they'd leave one column with no
    // OCR text and another column absorbing text from two cards at once.
    for pair in centers.windows(2) {
        if pair[1] - pair[0] < 0.08 { return false; }
    }
    // Cards sit on a fixed pitch, so the gaps between real bars are equal to within
    // a pixel or two. The span check below only looks at the outermost pair, so a
    // lopsided set gets through whenever its two ends happen to land the right
    // distance apart. [0.204, 0.316, 0.655] spans 0.451 against the 0.46 expected
    // of three cards, but its gaps differ threefold. The columns built from it put
    // two cards' text into a single column and cross their component names.
    if n >= 3 {
        let gaps: Vec<f32> = centers.windows(2).map(|p| p[1] - p[0]).collect();
        let mean = gaps.iter().sum::<f32>() / gaps.len() as f32;
        // A third of the ~0.13 card pitch. Wide enough for the jitter of locating
        // the small rarity arrows, and well under a card's width.
        if gaps.iter().any(|g| (g - mean).abs() > 0.04) { return false; }
    }
    let span = centers[n - 1] - centers[0];
    // Expected spans per card count (measured from real captures)
    let expected = match n {
        2 => 0.34f32,
        3 => 0.46,
        _ => 0.52, // 4 cards
    };
    (span - expected).abs() < 0.10
}

/// Evenly-distributed card X centers (fraction of image width) for N cards.
/// Calibrated from bar-detected centers on 1920×1080 captures: 4-card spread
/// is 0.31→0.69 (spacing ≈0.127), not the old 0.24→0.76.
/// Used as the fallback when rarity bar detection fails.
fn hardcoded_card_centers(n: usize) -> Vec<f32> {
    match n {
        1 => vec![0.50],
        2 => vec![0.435, 0.565],
        3 => vec![0.37, 0.50, 0.63],
        _ => vec![0.31, 0.44, 0.56, 0.69], // 4 cards (default / full squad)
    }
}

// ─── Matching helpers (standalone fns — no closure capture issues) ────────────

fn build_word_set(texts: &[String]) -> std::collections::HashSet<String> {
    let corrected = texts.join(" ")
        .replace('@', "bl").replace(')', "d").replace('&', " p");
    normalise(&corrected).chars()
        .map(|c| if c.is_ascii_alphabetic() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| w.len() >= 3)
        .map(|s| s.to_string())
        .collect()
}

fn score_item(display_name: &str, words: &std::collections::HashSet<String>) -> f32 {
    let norm = normalise(display_name);
    // Deduplicate so repeated words (e.g. "prime" twice in "Kavasa Prime Kubrow Collar Kavasa
    // Prime Band") don't inflate the matched/n_ocr ratio and outscore shorter correct matches.
    let mut seen = std::collections::HashSet::new();
    let item_words: Vec<&str> = norm.split_whitespace()
        .filter(|&w| seen.insert(w))
        .collect();
    if item_words.is_empty() { return 0.0; }
    let n_catalog = item_words.len() as f32;
    let n_ocr = words.len() as f32;
    let matched = item_words.iter()
        .filter(|&&w| word_found_in_set(w, words))
        .count();

    // Use max of two coverage ratios:
    //  • matched/n_catalog — what fraction of the catalog item's words appear in OCR
    //  • matched/n_ocr     — what fraction of the OCR words the catalog item explains
    // This prevents long item names from being penalised when they explain ALL the OCR
    // words. E.g. OCR {assis, blueprint}: "Xaku Prime Chassis Blueprint" (4 words)
    // matches both via suffix and exact → max(2/4, 2/2) = 1.0, beating "Forma Blueprint"
    // (2 words, matches 1/2) whose pure catalog-coverage score 0.50 previously won.
    let base = (matched as f32 / n_catalog)
        .max(if n_ocr > 0.0 { matched as f32 / n_ocr } else { 0.0 });

    // Length-affinity bonus for unmatched catalog words.
    // OCR almost always preserves word length (substitutes chars, not inserts them),
    // so prefer catalog words whose length is close to the OCR word length.
    // Max bonus per unmatched word is 0.08/n — always less than one matched word (1/n).
    let len_bonus: f32 = item_words.iter()
        .filter(|&&w| !word_found_in_set(w, words))
        .map(|&cw| {
            words.iter()
                .map(|ow| {
                    let diff = (cw.len() as isize - ow.len() as isize).unsigned_abs();
                    if diff == 0 { 0.08_f32 } else if diff == 1 { 0.04 } else { 0.0 }
                })
                .fold(0.0_f32, f32::max)
        })
        .sum::<f32>() / n_catalog;

    base + len_bonus
}

// ─── Reward item extraction ───────────────────────────────────────────────────

/// Relic reward detection.
///
/// 1. Find rarity bars → card X positions + bar Y (reliable visual anchor).
/// 2. Full-frame raw OCR → text with line X positions.
/// 3. Assign each OCR line to the nearest card (by X).
/// 4. Per-card word set → prefix + fuzzy match against relic catalog.
/// 5. Full-frame fallback if bar detection fails.
#[tracing::instrument(level = "info", skip_all)]
pub fn extract_reward_items_twophase(
    pixels: &[u8], pix_w: u32, pix_h: u32, _game_h: u32,
    catalog: &[(String, String)],
    capture_info: &str,
    hint_squad_size: Option<usize>,
    player_names: &[String],
) -> (bool, bool, Vec<String>, Vec<f32>, String) {

    // ── 1. Raw OCR ────────────────────────────────────────────────────────────
    let engine_output = run_ocr(pixels, pix_w, pix_h, OcrLayout::Scattered);

    let (raw_full, ocr_lines) =
        match engine_output {
            Ok(r) => r,
            Err(e) => return (false, false, vec![], vec![],
                format!("├─ Capture  : {}\n└─ OCR error: {}", capture_info, e)),
        };
    if raw_full.len() < 4 {
        // Save the captured BMP — open in photo viewer to diagnose:
        //   Black image  → capture grabbed an unmapped or obscured window
        //   Game content → OCR engine issue (tessdata missing/language)
        let debug_bmp = std::env::temp_dir().join("frameforge_capture_debug.bmp");
        let _ = std::fs::write(&debug_bmp, to_bmp(pixels, pix_w, pix_h));
        let avg = avg_brightness(pixels);
        let kind = if avg < 30 { "dark-frame" } else { "ocr-empty" };
        return (false, false, vec![], vec![], format!(
            "├─ Capture  : {}\n└─ OCR      : returned no text ({}, avg={})\n   Saved: {}",
            capture_info, kind, avg, debug_bmp.display()
        ));
    }

    // Relic selection screens show relics named with a quality tier
    // ("Axi N9 Intact Relic", "Neo R3 Flawless Relic", etc.).
    // The reward screen never shows quality words — but endless missions show
    // "1 Relic Opened" in the Endless Bonus tracker, which a bare " relic" check
    // incorrectly matched, causing every reward-screen attempt to be skipped.
    {
        let lower = raw_full.to_lowercase();
        const QUALITY: &[&str] = &["intact", "exceptional", "flawless", "radiant"];
        if lower.contains(" relic") && QUALITY.iter().any(|q| lower.contains(q)) {
            return (false, true, vec![], vec![], format!(
                "├─ Capture  : {}\n└─ OCR      : relic selection screen detected (skipped)",
                capture_info
            ));
        }
    }

    match_reward_items(
        pixels, pix_w, pix_h, &raw_full, &ocr_lines,
        catalog, capture_info, hint_squad_size, player_names,
    )
}

/// Post-OCR reward matching: rarity bars → card columns → catalog match → fill.
///
/// Split out from `extract_reward_items_twophase` so the pure text logic can be
/// driven with recorded OCR lines, independent of the platform-specific capture
/// and OCR path. `pixels` is still needed for the rarity-bar and card-icon
/// probes; pass the captured frame, or an empty slice when replaying recorded
/// lines (bar detection then returns no bars and the text path is exercised).
fn match_reward_items(
    pixels: &[u8], pix_w: u32, pix_h: u32,
    raw_full: &str,
    ocr_lines: &[(String, f32, f32)],
    catalog: &[(String, String)],
    capture_info: &str,
    hint_squad_size: Option<usize>,
    player_names: &[String],
) -> (bool, bool, Vec<String>, Vec<f32>, String) {

    // ── 2. Find card positions from rarity bars ───────────────────────────────
    // Rarity bars are always present regardless of Owned/Crafted labels.
    // If detection fails, fall back to X-gap grouping of OCR lines.
    let (bar_result, bar_diag) = find_rarity_bars(pixels, pix_w, pix_h);

    let (card_centers, _bar_y_frac): (Vec<f32>, f32) = match &bar_result {
        Some((centers, by)) => (centers.clone(), *by),
        None => (vec![], 0.0),
    };

    // Fixed cutoff near the bottom of the capture.
    // is_player_name handles name filtering; the bar-based cutoff was unreliable
    // (bar detection often placed bar_y too high, deleting valid item text).
    let ocr_y_max: f32 = 0.57;

    // Returns true if `text` resembles a known player name (≥80% char similarity).
    // Handles typical OCR garbling: "Dragonivan65" → "Dragonivan650", trailing symbols, etc.
    let is_player_name = |text: &str| -> bool {
        let t = text.trim().to_lowercase();
        if t.is_empty() { return false; }
        for name in player_names {
            let n = name.to_lowercase();
            // Fast path: exact substring in either direction
            if t.contains(&n) || n.contains(&t) { return true; }
            // Fuzzy path: Levenshtein ≤ 20% of the longer string
            let max_len = t.len().max(n.len());
            let threshold = (max_len / 5).max(1);
            if lev_dist(&t, &n) <= threshold { return true; }
        }
        false
    };

    // Returns true if the line is a card UI badge (ownership/craft status), not an item name.
    // Matches "Owned", "Crafted", "@ 4 Owned", "8 Crafted", "Unranked", etc.
    // Strips "@", numbers, and symbols — if only stopwords remain, it's a badge line.
    let is_ui_badge = |text: &str| -> bool {
        const BADGE_WORDS: &[&str] = &["owned", "crafted", "unranked", "mastered"];
        let meaningful: Vec<&str> = text.split_whitespace()
            .filter(|w| !w.starts_with('@') && w.parse::<u32>().is_err()
                    && w.len() > 1  // single chars like "O" (OCR mis-read of "0") are not meaningful
                    && w.chars().any(|c| c.is_alphabetic()))
            .collect();
        !meaningful.is_empty()
            && meaningful.iter().all(|w| BADGE_WORDS.contains(&w.to_lowercase().as_str()))
    };

    // ── 2b. Card count — prime+forma word count ──────────────────────────────
    // Every fissure reward is a prime item ("Prime" in name) or Forma Blueprint.
    // OCR frequently garbles "Prime" into "+rime", "Prtme", or merges it with the
    // next word ("Primeteüroptics").  Count any word that is "prime"-like:
    //   • starts with "prim"         → catches merged tokens like "primete..."
    //   • within edit-distance 1     → catches "+rime", "pnme", "prlme" etc.
    //   • "forma" or ≤1 edit of it  → catches "rorma", "torma" etc.
    let raw_norm = normalise(raw_full);
    let is_prime_like = |w: &str| -> bool {
        if w.starts_with("prim") && w.len() >= 4 { return true; }
        if w == "pri" { return true; }  // OCR truncation: "Lavos Prime" → "Lavos Pri"
        if w.len() >= 3 && w.len() <= 9 { return lev_dist(w, "prime") <= 1; }
        false
    };
    let is_forma_like = |w: &str| -> bool {
        if w == "forma" { return true; }
        if w.len() >= 3 && w.len() <= 7 { return lev_dist(w, "forma") <= 1; }
        false
    };
    let prime_count = raw_norm.split_whitespace().filter(|&w| is_prime_like(w)).count();
    let forma_count  = raw_norm.split_whitespace().filter(|&w| is_forma_like(w)).count();

    // Count distinct x-position clusters in OCR output.
    // Each card's text groups at a consistent x — gaps > 10% of width mark a new card.
    // Uses centroid-based clustering (not single-linkage) so that a single off-centre
    // OCR line between two adjacent card columns doesn't bridge them together.
    // Example: cards at 0.41 and 0.59 with a bridge line at 0.50 →
    //   single-linkage: 0.50-0.41=0.09 < 0.10 (merged), 0.59-0.50=0.09 < 0.10 (merged) → 1 cluster
    //   centroid:       0.50-0.41=0.09 < 0.10 (extend, center→0.455), 0.59-0.455=0.135 > 0.10 → 2 clusters
    let ocr_cluster_count: usize = {
        // Filter to lines that are (a) long enough to be item text and
        // (b) NOT in the top 10% of the capture. The reward cards never appear
        // there — only the game title bar and screen-edge HUD overlays (FPS
        // counters, GPU widgets) do. Excluding them prevents spurious x-clusters.
        let mut xs: Vec<f32> = ocr_lines.iter()
            .filter(|(t, _, y)| t.trim().len() >= 3 && *y >= 0.10 && *y < ocr_y_max && !is_player_name(t) && !is_ui_badge(t))
            .map(|(_, x, _)| *x)
            .collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if xs.is_empty() { 0 }
        else {
            let mut count = 1usize;
            let mut cluster_sum = xs[0];
            let mut cluster_n   = 1usize;
            for &x in &xs[1..] {
                let center = cluster_sum / cluster_n as f32;
                if x - center > 0.10 {
                    count += 1;
                    cluster_sum = x;
                    cluster_n   = 1;
                } else {
                    cluster_sum += x;
                    cluster_n   += 1;
                }
            }
            count.min(4)
        }
    };
    // EE hint (squad size from EE.log) is authoritative when OCR word-count undercounts.
    // e.g. 4-player run where OCR only sees 3 "Prime" tokens → use 4 from hint so that
    // hardcoded_card_centers(4) spreads columns wide enough to separate adjacent cards.
    let word_card_count = (prime_count + forma_count)
        .max(ocr_cluster_count)
        .max(hint_squad_size.unwrap_or(0))
        .clamp(1, 4);

    // ── 2c. Assign OCR lines to card columns ──────────────────────────────────
    // Use bar centers only when:
    //   • count matches prime+forma (guards against partial detection), AND
    //   • centers pass the spacing sanity check (guards against false positives
    //     from card artwork — orange/gold item renders trigger is_bar_pixel and
    //     produce bunched centers like [0.37, 0.50, 0.62, 0.71] instead of the
    //     expected even spread [0.24, 0.41, 0.59, 0.76]).
    let bars_trusted = !card_centers.is_empty()
        && card_centers.len() == word_card_count
        && bar_centers_are_valid(&card_centers);
    let active_centers: Vec<f32> = if bars_trusted {
        card_centers.clone()
    } else {
        hardcoded_card_centers(word_card_count)
    };

    // ── Raw OCR lines log (all lines, accepted + skipped with reason) ────────
    let raw_ocr_log: String = {
        let mut lines_log = Vec::new();
        for (i, (text, x, y)) in ocr_lines.iter().enumerate() {
            let tl = text.to_lowercase();
            let skip = if *y < 0.10 {
                Some(format!("y={:.2} < 0.10 top-HUD cutoff", y))
            } else if *y >= ocr_y_max {
                Some(format!("y={:.2} >= {:.2} below-bar cutoff", y, ocr_y_max))
            } else if is_player_name(text) {
                Some("player name".into())
            } else if is_ui_badge(text) {
                Some("UI badge".into())
            } else if tl.contains("booster") || tl.contains("relic opened") || tl.contains("endless bonus") {
                Some("endless bonus UI".into())
            } else {
                None
            };
            let entry = match skip {
                Some(r) => format!("  [{:>2}] {:>4} x={:.2} y={:.2}  ✗ {} — \"{}\"",
                    i, "", x, y, r, text.trim()),
                None    => format!("  [{:>2}] {:>4} x={:.2} y={:.2}  ✓ \"{}\"",
                    i, "", x, y, text.trim()),
            };
            lines_log.push(entry);
        }
        lines_log.join("\n")
    };

    let columns: Vec<(Vec<String>, f32)> = {
        let mut cols: Vec<(Vec<String>, f32)> =
            active_centers.iter().map(|&cx| (Vec::new(), cx)).collect();
        for (text, x, y) in ocr_lines {
            if *y < 0.10 || *y >= ocr_y_max || is_player_name(text) || is_ui_badge(text) { continue; }
            let idx = active_centers.iter().enumerate()
                .min_by(|(_, a), (_, b)| {
                    (x - *a).abs().partial_cmp(&(x - *b).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i)
                .unwrap_or(0);
            cols[idx].0.push(text.clone());
        }
        cols
    };

    // ── 3a. Per-card matching (only when rarity bars gave reliable columns) ─────
    // X-gap fallback columns are unreliable: OCR clusters all right-side card text
    // into the same column (wrong X positions), so per-column matching on fallback
    // columns produces wrong items. Only use per-column when bars were detected.
    let mut items: Vec<String> = Vec::new();
    let mut positions: Vec<f32> = Vec::new();

    let (_bar_y_frac, have_bars) = match &bar_result {
        Some((_, by)) => (*by, true),
        None => (0.0f32, false),
    };

    let mut col_match_log: Vec<String> = Vec::new();

    for (col_idx, (col_texts, cx)) in columns.iter().enumerate() {
        if items.len() >= active_centers.len() { break; }
        let words = build_word_set(col_texts);

        // Log what OCR text this column contains
        let col_preview: Vec<&str> = col_texts.iter().take(4).map(|s| s.trim()).collect();
        if words.is_empty() {
            col_match_log.push(format!(
                "  Col[{}] x={:.2}: (no words) — skipped\n    OCR: {:?}",
                col_idx, cx, col_preview));
            continue;
        }

        // ── Text-based scoring ───────────────────────────────────────────────
        let mut best_score = 0.0f32;
        let mut best_word_count = 0usize;
        let mut best_unique: Option<String> = None;
        let mut top3: Vec<(f32, String)> = Vec::new(); // for logging
        for (unique_name, display_name) in catalog {
            if display_name.len() < 5 { continue; }
            let s = score_item(display_name, &words);
            let wc = normalise(display_name).split_whitespace().count();
            if s > best_score || (s >= best_score - 1e-6 && wc > best_word_count) {
                best_score = s;
                best_word_count = wc;
                best_unique = Some(unique_name.clone());
            }
            // Collect top-3 for logging (keep sorted, max 3)
            if s > 0.0 {
                top3.push((s, display_name.clone()));
                top3.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                top3.truncate(3);
            }
        }
        let top3_str = top3.iter()
            .map(|(s, n)| format!("{:.2} \"{}\"", s, n))
            .collect::<Vec<_>>().join(" · ");

        // ── Icon-based fallback when text match is weak ──────────────────────
        let mut icon_log = String::new();
        if best_score < 0.67 && have_bars {
            let bar_y = _bar_y_frac;
            let half_w = if columns.len() > 1 { 0.56 / columns.len() as f32 / 2.0 } else { 0.10 };
            let icon_type = classify_card_icon(
                pixels, pix_w, pix_h,
                (cx - half_w).max(0.0), (cx + half_w).min(1.0), bar_y
            );
            let name_words = extract_item_name_words(&words);
            let component_filter: Option<&str> = match &icon_type {
                IconType::Component(c) => Some(c),
                IconType::Forma        => Some("forma"),
                IconType::FullModel    => Some("blueprint"),
                IconType::Unknown      => None,
            };
            icon_log = format!("\n    Icon: text={:.2} < 0.67 → classifier={:?}{}",
                best_score, icon_type,
                component_filter.map(|c| format!(" suffix=\"{}\"", c)).unwrap_or_default());

            if let Some(comp) = component_filter {
                let comp_norm = normalise(comp);
                let mut icon_best_score = 0.0f32;
                let mut icon_best_unique: Option<String> = None;
                let mut icon_top3: Vec<(f32, String)> = Vec::new();
                for (unique_name, display_name) in catalog {
                    if display_name.len() < 5 { continue; }
                    let dn = normalise(display_name);
                    if !dn.contains(comp_norm.as_str()) { continue; }
                    let name_matched = name_words.iter()
                        .filter(|nw| dn.contains(nw.as_str()))
                        .count();
                    let s = if name_words.is_empty() { 0.5 }
                            else { name_matched as f32 / name_words.len() as f32 };
                    if s > icon_best_score {
                        icon_best_score = s;
                        icon_best_unique = Some(unique_name.clone());
                    }
                    if s > 0.0 {
                        icon_top3.push((s, display_name.clone()));
                        icon_top3.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                        icon_top3.truncate(3);
                    }
                }
                let icon_top3_str = icon_top3.iter()
                    .map(|(s, n)| format!("{:.2} \"{}\"", s, n))
                    .collect::<Vec<_>>().join(" · ");
                icon_log += &format!("\n    Icon top3: {}", icon_top3_str);
                if icon_best_score >= 0.4 {
                    icon_log += &format!("\n    Icon accepted: {:.2} → \"{}\"",
                        icon_best_score,
                        icon_best_unique.as_ref().and_then(|u| catalog.iter().find(|(k,_)| k==u)).map(|(_,n)| n.as_str()).unwrap_or("?"));
                    best_score = icon_best_score;
                    best_unique = icon_best_unique;
                } else {
                    icon_log += "\n    Icon rejected (score < 0.40)";
                }
            }
        }

        // ── Log this column ──────────────────────────────────────────────────
        let best_display = best_unique.as_ref()
            .and_then(|u| catalog.iter().find(|(k, _)| k == u))
            .map(|(_, n)| n.as_str())
            .unwrap_or("—");
        let col_preview: Vec<&str> = col_texts.iter().map(|s| s.trim()).collect();
        let words_str: String = {
            let mut ws: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
            ws.sort();
            ws.join(", ")
        };
        col_match_log.push(format!(
            "  Col[{}] x={:.2}: score={:.2} → \"{}\"\n    OCR: {:?}\n    Words: {{{}}}\n    Top3: {}{}",
            col_idx, cx, best_score, best_display, col_preview, words_str, top3_str, icon_log
        ));

        // Require 0.67 for per-column. Items where only "prime"+"blueprint" match
        // score exactly 0.667 (still rejected). A specific word matched via suffix
        // or Levenshtein + one generic word scores ≥0.69 and is now accepted,
        // preventing the fallback which can cross-contaminate words from other columns.
        if best_score < 0.67 {
            // Unknown item (WFCD not yet updated or OCR garbled).
            // Emit raw OCR text with a "?:" prefix so the overlay can still show something.
            let raw = col_texts.iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .take(3)
                .collect::<Vec<_>>()
                .join(" ");
            if !raw.is_empty() {
                items.push(format!("?:{}", raw));
                positions.push(*cx);
            }
            continue;
        }
        let unique = match best_unique { Some(u) => u, None => continue };
        // No dedup here — each column is a distinct physical card.
        // Two players cracking the same relic legitimately show the same reward twice.
        // The `seen` set is only used in section 3b (full-frame fallback) where we
        // don't have column separation.
        items.push(unique);
        positions.push(*cx);
        let _ = col_idx;
    }

    // ── 3b. Full-frame fill ───────────────────────────────────────────────────
    // Determine expected card count — take the max of all three signals so that
    // any one reliable source prevents early lock-in:
    //   • EE.log squad size  (ground truth when available)
    //   • prime+forma count  (fuzzy word count from OCR)
    //   • rarity bar count   (visual, only when bars passed spacing validation)
    // IMPORTANT: only include bar count when bars_trusted. Rejected bars can give
    // wrong counts (e.g. 4 bars detected on a 3-card screen) that keep the OCR
    // loop retrying forever on a number it can never reach.
    let estimated_cards = hint_squad_size
        .unwrap_or(0)
        .max(word_card_count)
        .max(if bars_trusted { card_centers.len() } else { 0 })
        .max(1);

    // The fill may only recover cards the OCR actually saw. Every real card
    // leaves text in its column, so the number of columns that carried text is
    // the ceiling on how many items can exist. estimated_cards can exceed it when
    // a signal over-counts — a hovered card's description tooltip adds a stray
    // "Prime" token, or item artwork trips a phantom rarity bar — and the fill was
    // padding that surplus with a catalog item assembled from the other cards'
    // shared "prime"/"chassis"/"blueprint" words (fuzzy matching even lets an
    // absent model name like "trinity" register), inventing a reward for a column
    // that showed nothing. Cap the fill at the columns with text; estimated_cards
    // is left untouched so is_complete still waits for every card to be read
    // across retries rather than locking on a partial frame.
    //
    // Trade-off: a card whose text collapsed into a neighbour's column is no
    // longer recovered by guessing from the whole frame. That is rare once the
    // columns are spread apart, and a fabricated reward is the worse outcome.
    let fill_limit = estimated_cards.min(
        columns.iter().filter(|(t, _)| !build_word_set(t).is_empty()).count()
    );

    if items.len() < fill_limit {
        // Apply the same y-range filter used in column matching: exclude top-HUD
        // lines (y < 0.10) AND lines below the rarity bars (y >= ocr_y_max).
        // Without the bar cutoff, player names just below the bars (e.g. "HAR180::")
        // leak into the word set and produce false item matches like "Harrow Prime Blueprint".
        let all_words = build_word_set(
            &ocr_lines.iter()
                .filter(|(_, _, y)| *y >= 0.10 && *y < ocr_y_max)
                .map(|(t, _, _)| t.clone())
                .collect::<Vec<_>>()
        );

        // Words that appear in almost every reward and carry no item-specific
        // information. Excluded when finding which OCR line "anchors" each item
        // (for left-to-right ordering), but still used in scoring.
        const GENERIC: &[&str] = &["prime", "owned", "crafted", "blueprint"];

        // Find candidates with score ≥ 0.80 and sort by their first OCR line index.
        // OCR reads left-to-right, so line index approximates screen position.
        // Example: "Dual Zoren Prime Blueprint" → key word "zoren" → OCR line 1
        //          "Forma Blueprint"             → key word "forma"  → OCR line 4
        //          "Venato Prime Handle"         → key word "venato" → OCR line 6
        // Sorting by these indices gives the correct left→right overlay order
        // without requiring accurate X positions from OCR bounding rects.
        let mut candidates: Vec<(usize, f32, usize, String)> = Vec::new(); // (line_idx, score, name_len, unique)
        for (unique_name, display_name) in catalog {
            if display_name.len() < 5 { continue; }
            let s = score_item(display_name, &all_words);
            if s < 0.80 { continue; }

            let norm_dn = normalise(display_name);
            let key_words: Vec<&str> = norm_dn.split_whitespace()
                .filter(|w| w.len() >= 4 && !GENERIC.contains(w))
                .collect();

            // Find the earliest OCR line that contains one of this item's key words
            let first_line = if key_words.is_empty() {
                500usize // no unique identifier → sort after items with known positions
            } else {
                ocr_lines.iter().enumerate()
                    .find(|(_, (line_text, _, _))| {
                        let lt = normalise(line_text);
                        key_words.iter().any(|&w| lt.contains(w))
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(999) // not found in OCR → last priority
            };

            candidates.push((first_line, s, display_name.len(), unique_name.clone()));
        }
        // Primary: OCR line order (left → right). Secondary: score. Tertiary: name length.
        candidates.sort_by(|a, b|
            a.0.cmp(&b.0)
                .then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
                .then(b.2.cmp(&a.2))
        );

        // Seed base-name dedup from items already found by per-column matching.
        // Also track per-column duplicate counts: an item that appeared in N different
        // columns is legitimately repeated N times (4 players cracking the same relic).
        // We only re-allow it in the fill if it genuinely appeared multiple times.
        let mut seen_bases: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut per_col_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for un in &items {
            *per_col_counts.entry(un.clone()).or_insert(0) += 1;
            if let Some((_, dn)) = catalog.iter().find(|(u, _)| u == un) {
                let norm = normalise(dn);
                let ws: Vec<&str> = norm.split_whitespace().collect();
                if ws.len() >= 2 { seen_bases.insert(ws[..ws.len()-1].join(" ")); }
            }
        }

        for (_, _, _, unique) in candidates {
            if items.len() >= fill_limit { break; }
            let dn = match catalog.iter().find(|(u, _)| *u == unique) {
                Some((_, n)) => n.clone(),
                None => continue,
            };
            let dk = normalise(&dn);
            let current_count = items.iter().filter(|u| *u == &unique).count();
            let col_count = per_col_counts.get(&unique).copied().unwrap_or(0);
            let is_exact_duplicate = current_count > 0;
            let ws: Vec<&str> = dk.split_whitespace().collect();

            if is_exact_duplicate {
                // Only allow adding another copy if per-column matching confirmed
                // the same item in ≥2 columns (genuine multi-player duplicate).
                // Prevents filling missing-column gaps with re-copies of already-found items.
                if col_count < 2 || current_count >= col_count { continue; }
            } else {
                // Sibling dedup: block a DIFFERENT item from the same base name
                // (e.g. "Dual Zoren Prime Handle" blocked if "Dual Zoren Prime Blueprint" found)
                if ws.len() >= 2 {
                    let base = ws[..ws.len()-1].join(" ");
                    if seen_bases.contains(&base) { continue; }
                    seen_bases.insert(base);
                }
            }
            items.push(unique);
        }

        // Assign positions using the estimated card count for even spacing.
        // Cards are evenly distributed across the central ~70% of the screen.
        if !items.is_empty() {
            let n = estimated_cards.max(items.len());
            let spacing = 0.70 / (n as f32 + 1.0);
            positions = (0..items.len())
                .map(|i| 0.15 + spacing * (i as f32 + 1.0))
                .collect();
        }
    }

    // ── Diagnostic string ─────────────────────────────────────────────────────
    let col_mode = if bars_trusted { "bar columns (validated)" }
                   else if have_bars { "hardcoded (bars rejected)" }
                   else { "hardcoded (no bars)" };
    let ff_items: Vec<&str> = items.iter().map(|s| {
        catalog.iter().find(|(u,_)| u == s).map(|(_,n)| n.as_str()).unwrap_or(s.as_str())
    }).collect();
    // is_complete = true means "found all cards expected for this squad size".
    // lib.rs uses this to decide when to stop retrying OCR.
    // Only confirmed catalog matches count toward completion — "?:" unknowns are
    // noise or garbled text and must not trigger an early lock-in.
    let n_confirmed = items.iter().filter(|s| !s.starts_with("?:")).count();
    let is_complete = n_confirmed > 0 && n_confirmed >= estimated_cards;
    let expected_src = match (hint_squad_size, !card_centers.is_empty()) {
        (Some(h), _) if h >= word_card_count && h >= card_centers.len() => "EE.log",
        (_, true) if card_centers.len() >= word_card_count => "bars",
        _ if ocr_cluster_count > prime_count + forma_count => "x-clusters",
        _ => "prime+forma",
    };
    let ee_hint_str = match hint_squad_size {
        Some(n) => format!("{} players (from EE.log)", n),
        None    => "(not available — VoidProjections sequence not seen yet)".into(),
    };
    let debug = format!(
        "├─ Capture  : {}\n\
         ├─ OCR      : {} chars, {} lines\n\
         ├─ Bars     : {}\n\
         ├─ Prime/Forma: {}p + {}f + {}x = {} cards\n\
         ├─ EE hint  : {}\n\
         ├─ Expected : {} cards (from {}){}\n\
         ├─ Raw lines:\n{}\n\
         ├─ Match    : {} — {} formed\n\
         {}\n\
         └─ Items    : {:?}",
        capture_info,
        raw_full.len(), ocr_lines.len(),
        bar_diag,
        prime_count, forma_count, ocr_cluster_count, word_card_count,
        ee_hint_str,
        estimated_cards, expected_src,
        if is_complete { " ✅ complete" } else { " ⚡ partial" },
        raw_ocr_log,
        col_mode, columns.len(),
        col_match_log.join("\n"),
        ff_items,
    );

    (is_complete, false, items, positions, debug)
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
        for px in pixels.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
    }
    for px in pixels.chunks_exact_mut(4) {
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
fn assemble_ocr_lines(engine_lines: &[Vec<OcrWord>]) -> (String, Vec<(String, f32, f32)>) {
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
) -> Result<(String, Vec<(String, f32, f32)>), String> {
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
        .chunks_exact(4)
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
) -> Result<(String, Vec<(String, f32, f32)>), String> {
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
        .chunks_exact(4)
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
fn parse_tesseract_tsv(tsv: &str, img_w: u32, img_h: u32) -> Vec<Vec<OcrWord>> {
    // Zero dimensions would make every fraction a division by zero; the callers
    // reject sub-4-pixel rects, so this only guards against a degenerate BMP.
    if img_w == 0 || img_h == 0 {
        return Vec::new();
    }

    // Keyed by (block, paragraph, line) so words from two different text regions
    // that happen to share a baseline stay in separate lines.
    let mut lines: std::collections::BTreeMap<(i32, i32, i32), Vec<(i32, i32, OcrWord)>> =
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

    // ==========================================================================
    // Reward extraction corpus
    // ==========================================================================
    //
    // Everything above this line pins one component in isolation. This runs the
    // whole reward pipeline — capture-shaped pixels in, item names out — over
    // labelled screenshots of real reward screens, which is the only way to tell
    // whether a change to bar detection or catalog scoring actually helps.

    /// Images the pipeline currently gets wrong, with the reason. Listed rather
    /// than ignored so the test fails in BOTH directions: a regression adds an
    /// entry, and fixing a bug without deleting its entry is also a failure.
    const KNOWN_MISSES: &[(&str, &str)] = &[];

    /// End-to-end reward extraction against the labelled corpus.
    ///
    /// The corpus is not vendored: it is megabytes of real reward screens that
    /// already live in the sibling `wfinfo-ng` checkout, so the test skips when
    /// it is missing rather than failing. `WFINFO_TEST_IMAGES` overrides the
    /// location. Expect roughly one OCR pass per image — this is the slow test.
    #[test]
    fn reward_extraction_matches_the_labelled_corpus() {
        let dir = std::path::PathBuf::from(
            std::env::var("WFINFO_TEST_IMAGES").unwrap_or_else(|_| {
                format!("{}/../../wfinfo-ng/test-images", env!("CARGO_MANIFEST_DIR"))
            }),
        );
        let manifest = match std::fs::read_to_string(dir.join("manifest.json")) {
            Ok(m) => m,
            Err(_) => {
                eprintln!(
                    "skipping: no reward corpus at {} (set WFINFO_TEST_IMAGES)",
                    dir.display()
                );
                return;
            }
        };
        let manifest: serde_json::Value =
            serde_json::from_str(&manifest).expect("corpus manifest is JSON we generated");
        let images = manifest["images"]
            .as_object()
            .expect("corpus manifest always has an images object");

        // Nothing in the pipeline reads a unique name — it is only the key it
        // looks the display name back up by — so each name doubles as its key.
        let catalog: Vec<(String, String)> =
            include_str!("../tests/fixtures/relic_rewards.txt")
                .lines()
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(|n| (n.to_string(), n.to_string()))
                .collect();

        // The corpus labels a 2× Forma card "Forma Blueprint" while the catalog
        // carries a distinct "2X Forma Blueprint" entry that the pipeline
        // correctly prefers. Fold the two together instead of recording a miss
        // for an answer that is more precise than the label.
        let fold = |n: &str| n.trim_start_matches("2X ").to_string();

        let mut misses: Vec<(String, String)> = Vec::new();
        for (file, spec) in images {
            let image = match tauri::image::Image::from_path(dir.join(file)) {
                Ok(i) => i,
                Err(e) => panic!("corpus image {file} does not decode: {e}"),
            };
            let (width, full_h) = (image.width(), image.height());

            // Match what the live capture hands the pipeline: BGRA for the top
            // 80% of the game window. Bar detection reads absolute proportions,
            // so a full-height frame would not exercise the real geometry.
            let mut bgra = image.rgba().to_vec();
            for px in bgra.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            let cap_h = ((full_h as f32 * 0.80) as u32).max(1);
            bgra.truncate((width * cap_h * 4) as usize);

            // The squad size is what the live path gets from EE.log; the corpus
            // has no player names to filter out.
            let hint_squad = spec["reward_count"].as_u64().map(|n| n as usize);
            let (_, _, items, _, diag) = extract_reward_items_twophase(
                &bgra, width, cap_h, full_h, &catalog, file, hint_squad, &[],
            );

            let mut got: Vec<String> = items.iter().map(|n| fold(n)).collect();
            let mut want: Vec<String> = spec["items"]
                .as_array()
                .expect("every corpus entry lists its items")
                .iter()
                .map(|v| fold(v.as_str().expect("item names are strings")))
                .collect();
            // Compare as a multiset: which column a name landed in is a separate
            // concern, and a crossed pair still changes the names themselves.
            got.sort();
            want.sort();
            if got != want {
                misses.push((
                    file.clone(),
                    format!("wanted {want:?}, got {got:?}\n{diag}"),
                ));
            }
        }

        let mut missed: Vec<&str> = misses.iter().map(|(f, _)| f.as_str()).collect();
        let mut known: Vec<&str> = KNOWN_MISSES.iter().map(|(f, _)| *f).collect();
        missed.sort();
        known.sort();
        for (file, detail) in &misses {
            eprintln!("── {file}\n{detail}");
        }
        assert_eq!(
            missed, known,
            "corpus results moved; see the per-image detail above. \
             Known misses and their causes: {KNOWN_MISSES:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ocr_words(words: &[&str]) -> std::collections::HashSet<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn four_character_words_must_be_read_exactly() {
        for (catalog_word, on_screen) in [
            ("limb", "limbo"),
            ("gara", "galatine"),
            ("khra", "khora"),
            ("star", "stars"),
        ] {
            assert!(
                !word_found_in_set(catalog_word, &ocr_words(&[on_screen])),
                "{catalog_word:?} must not match {on_screen:?}"
            );
        }
    }

    #[test]
    fn longer_words_keep_their_edit_tolerance() {
        assert!(word_found_in_set("blueprint", &ocr_words(&["bluepnnt"])));
        assert!(word_found_in_set("tenora", &ocr_words(&["tenova"])));
        assert!(word_found_in_set("limb", &ocr_words(&["lim"])));
    }

    #[test]
    fn bar_centers_must_be_evenly_spaced() {
        assert!(!bar_centers_are_valid(&[0.204, 0.316, 0.655]));
        assert!(bar_centers_are_valid(&[0.27, 0.50, 0.73]));
        assert!(bar_centers_are_valid(&[0.24, 0.41, 0.59, 0.76]));
    }

    /// Hovering a reward card makes the game render its description tooltip
    /// ("A prime weapon-crafting component.") and repeat the card title in caps.
    /// Those lines add stray "Prime" tokens that push the card-count estimate to
    /// four on a three-card screen, and the full-frame fill then fabricates a
    /// catalog item — a reward that was never on screen — to reach that count.
    ///
    /// The OCR lines below are the exact ones the live pipeline read from a
    /// three-player run whose overlay showed a phantom "Trinity Prime Chassis
    /// Blueprint" alongside the three real rewards.
    #[test]
    fn a_hovered_card_tooltip_does_not_fabricate_a_fourth_reward() {
        let lines: &[(&str, f32, f32)] = &[
            ("0", 0.05, 0.01),
            ("99%", 0.09, 0.01),
            ("C", 0.11, 0.01),
            ("15%", 0.15, 0.01),
            ("53\"", 0.16, 0.01),
            ("——— = VOID FISSURE/REWARDS", 0.19, 0.08),
            ("3", 0.50, 0.19),
            ("& Crafted", 0.59, 0.28),
            ("® Owned", 0.34, 0.28),
            ("® Owned", 0.46, 0.28),
            ("Lavos Prime Chassis", 0.50, 0.49),
            ("Lex Prime Barrel", 0.37, 0.52),
            ("Blueprint", 0.50, 0.52),
            ("2 X Forma Blueprint", 0.61, 0.52),
            ("LEX PRIME BARREL", 0.32, 0.57),
            ("teOwl12 5a", 0.52, 0.59),
            ("Falcon1719+", 0.62, 0.59),
            ("N", 0.47, 0.64),
            ("©@ 1 Owned", 0.30, 0.63),
            ("A prime weapon-crafting component.", 0.34, 0.70),
            ("Can be exchanged for", 0.33, 0.76),
            ("15 Ducats", 0.43, 0.76),
            ("Steel Path Bonus", 0.53, 0.77),
            ("+1 Steel Essence", 0.53, 0.80),
            ("Endless Bonus Affinity Booster | 1 Relic Opened", 0.51, 0.89),
        ];
        let ocr_lines: Vec<(String, f32, f32)> =
            lines.iter().map(|(t, x, y)| (t.to_string(), *x, *y)).collect();
        // prime_count scans the whole-frame text, so raw_full must carry every token.
        let raw_full = lines.iter().map(|(t, _, _)| *t).collect::<Vec<_>>().join(" ");

        // The three real rewards, plus the sibling and look-alike families that
        // share their component words — enough for the full-frame fill to prefer a
        // fabricated fourth (as the live catalog did). Each name doubles as its own
        // key, matching the live catalog shape.
        let catalog: Vec<(String, String)> = [
            "Lex Prime Barrel", "Lex Prime Blueprint", "Lex Prime Receiver",
            "Lavos Prime Blueprint", "Lavos Prime Chassis Blueprint",
            "Lavos Prime Neuroptics Blueprint", "Lavos Prime Systems Blueprint",
            "Forma Blueprint", "2X Forma Blueprint",
            "Trinity Prime Blueprint", "Trinity Prime Chassis Blueprint",
            "Trinity Prime Neuroptics Blueprint", "Trinity Prime Systems Blueprint",
            "Atlas Prime Chassis Blueprint", "Acceltra Prime Barrel",
            "Afuris Prime Barrel", "Boltor Prime Barrel",
        ]
        .iter()
        .map(|n| (n.to_string(), n.to_string()))
        .collect();

        let player_names: Vec<String> = [
            "Vireo_", "q-lox", "grimlo1994", "Duo-vertex",
            "yubblenix", "TheVortexKnave1", "Falcon1719", "PrivateOwl12",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        // A small black frame yields no rarity bars, so matching takes the same
        // hardcoded-column path the live capture used after its (bunched) bars
        // were rejected. The phantom is produced by the text fill, not the bars.
        let pixels = vec![0u8; 8 * 8 * 4];

        let (_complete, _skip, items, _positions, diag) = match_reward_items(
            &pixels, 8, 8, &raw_full, &ocr_lines,
            &catalog, "replay", None, &player_names,
        );

        // The catalog carries a distinct "2X Forma Blueprint"; either spelling is
        // the same real card.
        let fold = |n: &str| n.trim_start_matches("2X ").to_string();
        let mut got: Vec<String> = items.iter().map(|n| fold(n)).collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                "Forma Blueprint".to_string(),
                "Lavos Prime Chassis Blueprint".to_string(),
                "Lex Prime Barrel".to_string(),
            ],
            "expected exactly the three real rewards, no fabricated fourth\n{diag}"
        );
    }
}
