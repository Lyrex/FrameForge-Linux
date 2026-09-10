use tracing::debug;
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

/// Whether the cached file at `path` is worth serving. Reads only the header —
/// this runs once per catalogued image on every startup. Downloads land via
/// rename, so a file with a valid header is a whole file.
fn cached_image_ok(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; 12];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok()
        && looks_like_image(&buf)
}

#[tauri::command]
pub(crate) fn get_img_cache_dir(state: State<AppState>) -> String {
    state.img_cache_dir.to_string_lossy().into_owned()
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

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(10)))
            .build()
            .into();

        for chunk in names.chunks(8) {
            let handles: Vec<_> = chunk.iter().map(|name| {
                let dir = Arc::clone(&cache_dir);
                let name = name.clone();
                let agent = agent.clone();
                std::thread::spawn(move || {
                    let url = format!("https://cdn.warframestat.us/img/{}", name);
                    if let Ok(resp) = agent.get(&url).call() {
                        let mut buf = Vec::new();
                        if resp.into_body().into_reader().take(5 * 1024 * 1024).read_to_end(&mut buf).is_ok() && looks_like_image(&buf) {
                            let _ = std::fs::create_dir_all(&*dir);
                            // A dropped connection would otherwise leave a
                            // truncated file under the real name.
                            let part = dir.join(format!("{name}.part"));
                            if std::fs::write(&part, buf).is_ok() {
                                let _ = std::fs::rename(&part, dir.join(&name));
                            }
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
