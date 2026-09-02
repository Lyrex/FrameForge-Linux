use tauri::{Manager, State};
use crate::app_state::AppState;
use crate::cache;
use crate::rivens::WfmScanSlot;
use crate::wfm::{to_wfm_slug, Wfm, WfmTopItem};

const WFM_TOP_CACHE: &str = "wfm-top-v1.json";
const WFM_TOP_TTL: std::time::Duration = std::time::Duration::from_secs(3 * 3600);

/// Arcane name, WFM slug and image, as handed to a scan.
type ArcaneCandidate = (String, String, Option<String>);

/// Return the top 10 most-traded items on warframe.market by 7-day total value.
/// Queries Prime Sets and Arcanes from the local WFCD catalog (already loaded).
/// Results are cached for 3 hours so repeated tab opens are instant.
#[tauri::command]
pub(crate) async fn get_wfm_top_items(state: State<'_, AppState>) -> Result<Vec<WfmTopItem>, String> {
    // In-memory cache, fresh within the TTL — the client owns it.
    if let Some(items) = state.wfm.cached_top_items(WFM_TOP_TTL) {
        return Ok(items);
    }

    // Disk cache — survives app restarts. An empty list is a scan that found
    // nothing, which is never worth serving.
    let now_secs = cache::now_unix();
    let disk = cache::load::<Vec<WfmTopItem>>(WFM_TOP_CACHE).filter(|c| !c.data.is_empty());
    if let Some(cached) = &disk {
        if now_secs.saturating_sub(cached.retrieved_at_unix) < WFM_TOP_TTL.as_secs() {
            state.wfm.set_top_items(cached.data.clone());
            cache::set_status(WFM_TOP_CACHE, cache::CacheStatus {
                source:       cache::Source::Fresh,
                last_updated: Some(cached.retrieved_at_unix),
                warning:      None,
            });
            return Ok(cached.data.clone());
        }
    }

    // Collect arcane candidates from WFCD without holding the lock across await points.
    // Prime Sets come from WFM's own item list (fetched inside the scan) so that we get
    // canonical slugs — WFCD doesn't have set-level entries.
    let arcane_candidates: Vec<ArcaneCandidate> = {
        let items = state.wfcd_items.lock().map_err(|e| e.to_string())?;
        items.iter()
            .filter(|i| i.category == "Arcanes")
            .map(|i| (i.name.clone(), to_wfm_slug(&i.name), i.image_name.clone()))
            .collect()
    };

    // Only one scan at a time: a second one would compete for the same rate-limiter
    // budget and take twice as long for both.
    let scan_slot = WfmScanSlot::claim();

    // An expired copy beats an empty tab for the minute and a half a scan takes,
    // so hand it over and rescan behind it.
    if let Some(cached) = disk {
        if let Some(slot) = scan_slot {
            let wfm = state.wfm.clone();
            std::thread::spawn(move || {
                let _slot = slot;
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    finish_wfm_top_scan(&wfm, scan_wfm_top_items(&wfm, &arcane_candidates));
                }));
            });
        }
        cache::set_status(WFM_TOP_CACHE, cache::CacheStatus {
            source:       cache::Source::Refreshing,
            last_updated: Some(cached.retrieved_at_unix),
            warning:      None,
        });
        return Ok(cached.data);
    }

    // Nothing to show, so the caller has to wait for a scan either way.
    let Some(_slot) = scan_slot else {
        for _ in 0..120u32 {  // poll every 5 s, max 10 minutes
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if let Some(items) = state.wfm.cached_top_items(WFM_TOP_TTL) {
                return Ok(items);
            }
        }
        return Err("WFM top items scan timed out".to_string());
    };

    // Run blocking ureq calls on the thread pool — keeps the async runtime free
    let wfm = state.wfm.clone();
    let scan_result =
        tokio::task::spawn_blocking(move || scan_wfm_top_items(&wfm, &arcane_candidates)).await;

    let results = scan_result.map_err(|e| e.to_string())?;
    finish_wfm_top_scan(&state.wfm, results.clone());
    Ok(results)
}

/// Background-refresh entry point. A scan walks the whole WFM item list under a
/// rate limiter and takes minutes, so it runs on its own thread rather than
/// holding up every other refresh; the schedule, not a backoff, retries it.
pub(crate) fn refresh_wfm_top(app: &tauri::AppHandle, force: bool) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !force && state.wfm.cached_top_items(WFM_TOP_TTL).is_none() {
        // A restart empties the in-memory copy while the disk one is still
        // good; adopting it is what keeps a launch from scanning.
        let now_secs = cache::now_unix();
        if let Some(cached) = cache::load::<Vec<WfmTopItem>>(WFM_TOP_CACHE)
            .filter(|c| !c.data.is_empty()
                && now_secs.saturating_sub(c.retrieved_at_unix) < WFM_TOP_TTL.as_secs())
        {
            state.wfm.set_top_items(cached.data);
        }
    }
    if !force && state.wfm.cached_top_items(WFM_TOP_TTL).is_some() {
        return Ok(());
    }
    let Some(slot) = WfmScanSlot::claim() else {
        return Ok(());
    };
    let arcane_candidates: Vec<ArcaneCandidate> = {
        let items = state.wfcd_items.lock().unwrap_or_else(|e| e.into_inner());
        items.iter()
            .filter(|i| i.category == "Arcanes")
            .map(|i| (i.name.clone(), to_wfm_slug(&i.name), i.image_name.clone()))
            .collect()
    };
    let wfm = state.wfm.clone();
    std::thread::spawn(move || {
        let _slot = slot;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            finish_wfm_top_scan(&wfm, scan_wfm_top_items(&wfm, &arcane_candidates));
        }));
    });
    Ok(())
}

fn scan_wfm_top_items(wfm: &Wfm, arcane_candidates: &[ArcaneCandidate]) -> Vec<WfmTopItem> {
    let prime_sets = wfm.prime_sets();
    let mut out: Vec<WfmTopItem> = Vec::new();

    for (name, url_name) in &prime_sets {
        if let Some((price, daily_vol)) = wfm.stats_7day(url_name) {
            out.push(WfmTopItem {
                name:           name.clone(),
                url_name:       url_name.clone(),
                image_name:     None,
                unit_price:     price,
                daily_volume:   daily_vol,
                total_value_7d: (price as f64 * daily_vol * 7.0) as u64,
            });
        }
    }

    for (name, slug, image_name) in arcane_candidates {
        if let Some((price, daily_vol)) = wfm.stats_7day(slug) {
            out.push(WfmTopItem {
                name:           name.clone(),
                url_name:       slug.clone(),
                image_name:     image_name.clone(),
                unit_price:     price,
                daily_volume:   daily_vol,
                total_value_7d: (price as f64 * daily_vol * 7.0) as u64,
            });
        }
    }

    out.sort_by(|a, b| b.total_value_7d.cmp(&a.total_value_7d));
    out.truncate(10);
    out
}

fn finish_wfm_top_scan(wfm: &Wfm, results: Vec<WfmTopItem>) {
    // A scan that priced nothing means warframe.market was unreachable, not that
    // nothing trades. Both readers skip an empty list anyway, so storing one only
    // throws away the copy that still has items in it.
    if results.is_empty() {
        cache::set_status(WFM_TOP_CACHE, cache::CacheStatus {
            source:       cache::Source::Stale,
            last_updated: cache::load::<Vec<WfmTopItem>>(WFM_TOP_CACHE).map(|c| c.retrieved_at_unix),
            warning:      Some("warframe.market top items: scan returned nothing".into()),
        });
        return;
    }
    if let Err(e) = cache::store(WFM_TOP_CACHE, None, &results) {
        tracing::warn!("cannot write WFM top items cache: {e}");
    }
    cache::set_status(WFM_TOP_CACHE, cache::CacheStatus {
        source:       cache::Source::Refreshed,
        last_updated: Some(cache::now_unix()),
        warning:      None,
    });
    wfm.set_top_items(results);
}
