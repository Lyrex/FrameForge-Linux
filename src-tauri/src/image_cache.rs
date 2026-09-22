use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;
use tracing::debug;

use crate::app_state::AppState;

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
            match tokio::fs::read(dir.join(filename)).await {
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
    let cache_dir = Arc::new(state.img_cache_dir.clone());

    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let names: Vec<String> = items.iter()
            .filter_map(|i| i.image_name.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|n| !cache_dir.join(n).exists())
            .collect();

        if names.is_empty() { return; }
        debug!(count = names.len(), "prewarming images in background");

        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(10))
            .build();

        for chunk in names.chunks(8) {
            let handles: Vec<_> = chunk.iter().map(|name| {
                let dir = Arc::clone(&cache_dir);
                let name = name.clone();
                let agent = agent.clone();
                std::thread::spawn(move || {
                    let url = format!("https://cdn.warframestat.us/img/{}", name);
                    if let Ok(resp) = agent.get(&url).call() {
                        let mut buf = Vec::new();
                        // Limit to 5 MB to prevent memory exhaustion from malformed responses
                        if resp.into_reader().take(5 * 1024 * 1024).read_to_end(&mut buf).is_ok() {
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
