//! Linux stubs for screen capture + OCR (not yet implemented).

pub fn avg_brightness(_pixels: &[u8]) -> u32 { 0 }

pub fn capture_warframe_reward_area() -> Option<(Vec<u8>, u32, u32, u32, String)> { None }

pub fn capture_warframe_pixels() -> Result<(Vec<u8>, u32, u32), String> {
    Err("Screen capture not supported on Linux yet".into())
}

pub fn ocr_pixels_rect(
    _pixels: &[u8], _full_w: u32, _full_h: u32,
    _x_start: f32, _x_end: f32, _y_start: f32, _y_end: f32,
) -> Result<String, String> {
    Err("OCR not supported on Linux yet".into())
}

pub fn ocr_pixels_rect_raw(
    _pixels: &[u8], _full_w: u32, _full_h: u32,
    _x_start: f32, _x_end: f32, _y_start: f32, _y_end: f32,
) -> Result<String, String> {
    Err("OCR not supported on Linux yet".into())
}

pub fn capture_and_ocr_region(_y_start: f32, _y_end: f32) -> Result<String, String> {
    Err("OCR not supported on Linux yet".into())
}

pub fn capture_rect_and_ocr(_x_start: f32, _x_end: f32, _y_start: f32, _y_end: f32) -> Result<String, String> {
    Err("OCR not supported on Linux yet".into())
}

pub fn detect_fissure_era() -> Option<String> { None }

pub fn capture_screen_for_diagnostics_half() -> Result<(Vec<u8>, u32, u32), String> {
    Err("Screen capture not supported on Linux yet".into())
}

pub fn capture_desktop_for_diag() -> Option<(Vec<u8>, u32, u32)> { None }

pub type OcrResult = (String, Vec<(String, f32, f32)>);

pub fn run_windows_ocr(_bmp: Vec<u8>, _w: u32, _h: u32) -> Result<OcrResult, String> {
    Err("OCR not supported on Linux yet".into())
}
