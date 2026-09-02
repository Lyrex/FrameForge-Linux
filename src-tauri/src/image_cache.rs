use tracing::debug;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;
use crate::app_state::AppState;

/// A download cut short by a dropped connection leaves a file the filesystem is
/// perfectly happy with and the webview renders as a broken image forever, so
/// the header is what decides whether a cached image counts as one.
fn looks_like_image(data: &[u8]) -> bool {
    data.starts_with(b"\x89PNG\r\n\x1a\n")
        || data.starts_with(&[0xFF, 0xD8, 0xFF])
        || data.starts_with(b"GIF8")
        || (data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP")
}

/// Whether a fully read cached file is a complete image. The header identifies
/// the format; the trailer is what a download cut short by a dropped connection
/// loses, and a truncated file keeps its header.
fn image_complete(data: &[u8]) -> bool {
    if !looks_like_image(data) {
        return false;
    }
    if data.starts_with(b"\x89PNG") {
        // A PNG ends with the IEND chunk: its name followed by a 4-byte CRC.
        return data.len() >= 12 && data[data.len() - 8..data.len() - 4] == *b"IEND";
    }
    if data.starts_with(&[0xFF, 0xD8]) {
        return data.ends_with(&[0xFF, 0xD9]);
    }
    if data.starts_with(b"GIF8") {
        return data.ends_with(&[0x3B]);
    }
    // WEBP: the RIFF size field counts everything after the first 8 bytes.
    let size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    data.len() >= size.saturating_add(8)
}

/// Whether the cached file at `path` is worth serving. Reads only the header —
/// this runs once per catalogued image on every startup — so truncation past
/// the header slips through here and is caught at serve time instead.
fn cached_image_ok(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; 12];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok()
        && looks_like_image(&buf)
}

/// Minimal HTTP file server for the local image cache.
/// Accepts GET /{filename} and serves files from `cache_dir`.
pub(crate) async fn serve_image_files(listener: tokio::net::TcpListener, cache_dir: PathBuf) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let cache_dir = Arc::new(cache_dir);
    loop {
        let Ok((mut stream, _)) = listener.accept().await else { continue };
        let dir = Arc::clone(&cache_dir);
        tokio::spawn(async move {
            let mut buf = vec![0u8; 512];
            let n = match stream.read(&mut buf).await {
                Ok(n) if n > 0 => n,
                _ => return,
            };
            let req = std::str::from_utf8(&buf[..n]).unwrap_or("");
            let filename = match req.lines().next()
                .and_then(|l| l.strip_prefix("GET /"))
                .and_then(|l| l.split_whitespace().next())
            {
                Some(f) if !f.is_empty() && !f.contains("..") && !f.contains('/') && !f.contains('\\') => f,
                _ => {
                    let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n").await;
                    return;
                }
            };
            let path = dir.join(filename);
            match tokio::fs::read(&path).await {
                // A corrupt file is thrown away rather than served: the 404
                // sends the caller to the CDN, and the next prewarm refetches it.
                Ok(data) if !image_complete(&data) => {
                    let _ = tokio::fs::remove_file(&path).await;
                    let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
                }
                Ok(data) => {
                    let mime = if filename.ends_with(".png") { "image/png" }
                        else if filename.ends_with(".jpg") || filename.ends_with(".jpeg") { "image/jpeg" }
                        else if filename.ends_with(".webp") { "image/webp" }
                        else { "application/octet-stream" };
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: public, max-age=86400\r\n\r\n",
                        mime, data.len()
                    );
                    let _ = stream.write_all(header.as_bytes()).await;
                    let _ = stream.write_all(&data).await;
                }
                Err(_) => {
                    let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
                }
            }
        });
    }
}

/// Returns the base URL of the local image server, e.g. "http://127.0.0.1:51234".
/// Frontend uses this as `${baseUrl}/${imageName}` to load cached images from disk.
#[tauri::command]
pub(crate) fn get_img_cache_dir(state: State<AppState>) -> String {
    let port = *state.img_server_port.lock().unwrap();
    format!("http://127.0.0.1:{}", port)
}

/// Download images for all craftable items that aren't already cached to disk.
/// Returns immediately — downloads happen on background threads (8 in parallel).
/// Safe to call every startup; already-cached files are skipped via existence check.
#[tauri::command]
pub(crate) async fn prewarm_image_cache(state: tauri::State<'_, AppState>) -> Result<(), String> {
    use std::collections::HashSet;
    use std::sync::Arc;
    let items: Vec<_> = state.wfcd_items.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let recipe_names: HashSet<String> = state.recipes.lock()
        .unwrap_or_else(|e| e.into_inner()).keys().cloned().collect();
    let cache_dir = Arc::new(state.img_cache_dir.clone());

    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let names: Vec<String> = items.iter()
            .filter(|i| recipe_names.contains(&i.unique_name))
            .filter_map(|i| i.image_name.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|n| {
                let path = cache_dir.join(n);
                if cached_image_ok(&path) {
                    return false;
                }
                // Whatever is there is not an image; a refetch needs the name
                // free of it either way.
                let _ = std::fs::remove_file(&path);
                true
            })
            .collect();

        if names.is_empty() { return; }
        debug!(count = names.len(), "prewarming images in background");

        for chunk in names.chunks(8) {
            let handles: Vec<_> = chunk.iter().map(|name| {
                let dir = Arc::clone(&cache_dir);
                let name = name.clone();
                std::thread::spawn(move || {
                    let url = format!("https://cdn.warframestat.us/img/{}", name);
                    if let Ok(resp) = ureq::get(&url).call() {
                        let mut buf = Vec::new();
                        if resp.into_reader().read_to_end(&mut buf).is_ok() && looks_like_image(&buf) {
                            let _ = std::fs::write(dir.join(&name), buf);
                        }
                    }
                })
            }).collect();
            for h in handles { let _ = h.join(); }
        }
        debug!("prewarm complete");
    }); // intentionally not awaited — fire and forget

    Ok(())
}

#[cfg(test)]
mod image_validation_tests {
    use super::looks_like_image;

    #[test]
    fn the_header_decides_what_counts_as_an_image() {
        assert!(looks_like_image(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"));
        assert!(looks_like_image(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!looks_like_image(b""));
        assert!(!looks_like_image(b"<!DOCTYPE html>"));
        // A WEBP header that stops before the format tag.
        assert!(!looks_like_image(b"RIFF\0\0\0\0"));
    }
}
