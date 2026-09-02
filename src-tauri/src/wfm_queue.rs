use std::collections::HashMap;
use tracing::warn;
use std::sync::atomic::Ordering;
use tauri::{Emitter, State};
use crate::app_state::AppState;
use crate::cache::atomic_write;
use crate::catalogue::fix_category;
use crate::inventory_state::{load_inventory_state_cache, CachedItem};
use crate::resolver::ItemResolver;
use crate::resolver;
use crate::wfm::to_wfm_slug;

// All warframe.market price fetches are routed through a single background
// thread that enforces the ≤3 req/sec rate limit globally. The frontend enqueues
// slugs via wfm_queue_prices / wfm_queue_price_priority and listens for
// "wfm-price-update" events instead of calling fetch_wfm_price directly.

#[derive(serde::Serialize, Clone)]
struct WfmPriceUpdate {
    url_name:     String,
    sell_median:  Option<u32>,
    tradeable:    bool,
}

/// Start the WFM price queue drain thread (no-op if already running).
/// Must be called after fetch_item_list so wfcd_items is populated.
#[tauri::command]
pub(crate) fn start_wfm_queue(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if state.wfm_queue_started.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    // Pre-populate the in-memory price cache from inventory_state_cache.json so that
    // wfm_get_cached_prices() returns previously-fetched prices immediately on startup
    // and the queue drain skips slugs that already have a fresh price.
    {
        let disk = load_inventory_state_cache(&state.inventory_state_cache_path);
        for item in disk.items.values() {
            if !item.name.is_empty() {
                let slug = to_wfm_slug(&item.name);
                if !slug.is_empty() {
                    // Only insert if we have a price; None entries are kept absent so they get re-queued.
                    if let Some(p) = item.wfm_price {
                        state.wfm.cache_price(slug, Some(p));
                    }
                }
            }
        }
    }

    // Build slug → unique_name + tradeable map from a snapshot of wfcd_items.
    // Items are loaded once and the thread keeps this snapshot (items rarely change).
    let slug_map: HashMap<String, (String, bool)> = {
        let items = state.wfcd_items.lock().unwrap_or_else(|e| e.into_inner());
        let resolver = ItemResolver::from_items(&items);
        let mut m = HashMap::new();
        for item in items.iter() {
            let cat = fix_category(
                &item.name,
                &item.item_type,
                &item.product_category,
                &item.category,
                &item.unique_name,
            );
            let tradeable = item.ducats.is_some() || matches!(cat.as_str(), "Mods" | "Arcanes");
            if !tradeable {
                continue;
            }
            let Some(resolved) = resolver.by_unique(&item.unique_name) else {
                continue;
            };
            for slug in resolver::slug_variants(&resolved.slug) {
                m.insert(slug, (item.unique_name.clone(), true));
            }
        }
        m
    };

    let queue          = state.wfm_price_queue.clone();
    let priority_queue = state.wfm_priority_queue.clone();
    let wfm            = state.wfm.clone();
    let cache_path     = state.inventory_state_cache_path.clone();

    std::thread::spawn(move || {
        loop {
            // Priority queue drains first; fall back to normal queue.
            let slug = {
                let mut pq = priority_queue.lock().unwrap_or_else(|e| e.into_inner());
                pq.pop_front()
            }.or_else(|| {
                let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                q.pop_front()
            });

            let slug = match slug {
                Some(s) => s,
                None => {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    continue;
                }
            };

            // Skip if already cached (avoid redundant API calls within a session).
            if wfm.is_price_cached(&slug) { continue; }

            // Fetch — the rate limiter inside enforces the 3 req/sec limit.
            let price = match wfm.price_with_fallback(&slug) {
                Ok(p) => p,
                Err(e) => {
                    warn!(slug = %slug, error = %e, "price lookup failed, skipping");
                    continue;
                }
            };
            let tradeable = price.is_some();

            // Update in-memory cache.
            wfm.cache_price(slug.clone(), price);

            // Write price + tradeable_wfm into inventory_state_cache.json if we know the item.
            if let Some((unique_name, _)) = slug_map.get(&slug) {
                let mut inv = load_inventory_state_cache(&cache_path);
                let entry = inv.items.entry(unique_name.clone())
                    .or_insert_with(|| CachedItem { unique_name: unique_name.clone(), ..Default::default() });
                entry.wfm_price     = price;
                entry.tradeable_wfm = tradeable;
                if let Ok(json) = serde_json::to_string(&inv) {
                    let _ = atomic_write(&cache_path, json.as_bytes());
                }
            }

            // Notify the frontend.
            let _ = app.emit("wfm-price-update", WfmPriceUpdate {
                url_name: slug, sell_median: price, tradeable,
            });
        }
    });

    Ok(())
}

/// Add slugs to the normal-priority WFM price queue.
/// Slugs already cached in-memory are silently skipped.
#[tauri::command]
pub(crate) fn wfm_queue_prices(state: State<'_, AppState>, url_names: Vec<String>) {
    let mut q = state.wfm_price_queue.lock().unwrap_or_else(|e| e.into_inner());
    // Snapshot existing queue entries to deduplicate without holding a borrow during push_back.
    let already_queued: std::collections::HashSet<String> = q.iter().cloned().collect();
    for slug in url_names {
        if !state.wfm.is_price_cached(&slug) && !already_queued.contains(&slug) {
            q.push_back(slug);
        }
    }
}

/// Push a single slug to the front of the priority queue (for popup / on-demand fetches).
/// Forces a fresh fetch even if cached.
#[tauri::command]
pub(crate) fn wfm_queue_price_priority(state: State<'_, AppState>, url_name: String) {
    // Remove any existing cached entry so the drain thread fetches fresh.
    state.wfm.uncache_price(&url_name);
    state.wfm_priority_queue.lock().unwrap_or_else(|e| e.into_inner())
        .push_front(url_name);
}

/// Return the current in-memory WFM price cache (slug → price).
/// Frontend calls this on startup to populate prices without waiting for the queue.
#[tauri::command]
pub(crate) fn wfm_get_cached_prices(state: State<'_, AppState>) -> HashMap<String, Option<u32>> {
    state.wfm.cached_prices()
}
