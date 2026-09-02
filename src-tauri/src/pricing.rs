use std::collections::HashMap;
use tracing::warn;
use tauri::{Manager, State};
use crate::app_state::AppState;
use crate::wfm::to_wfm_slug;
use crate::{cache, refresh};

/// Re-fetch bulk prices from FrameForgePricing, ignoring the cache's age.
/// Updates both relics_run_prices and the WFM price cache in-place.
#[tauri::command]
pub(crate) async fn refresh_bulk_prices(state: State<'_, AppState>) -> Result<(), String> {
    let (prices, source, warning) = tauri::async_runtime::spawn_blocking(|| {
        cache::get_or_refresh(BULK_PRICES_CACHE, std::time::Duration::ZERO, fetch_relics_run_data)
    })
    .await
    .map_err(|e| e.to_string())?;

    // The user asked for new prices, so a stale copy is not an answer even
    // though it is good enough for the background refresh.
    if source != cache::Source::Refreshed {
        return Err(warning.unwrap_or_else(|| "Failed to fetch bulk prices.".to_string()));
    }
    let prices = prices.ok_or("Failed to fetch bulk prices.")?;

    apply_bulk_prices(&state, prices, true);
    Ok(())
}

/// Background-refresh entry point. Unlike the manual command it defers to any
/// per-item WFM lookup that has already priced a slug more precisely.
pub(crate) fn refresh_bulk_prices_task(app: &tauri::AppHandle, force: bool) -> Result<(), String> {
    let ttl = if force { std::time::Duration::ZERO } else { BULK_PRICES_TTL };
    let (prices, _, warning) = cache::get_or_refresh(BULK_PRICES_CACHE, ttl, fetch_relics_run_data);
    match prices {
        Some(prices) if warning.is_none() => {
            apply_bulk_prices(&app.state::<AppState>(), prices, false);
            Ok(())
        }
        _ => Err(warning.unwrap_or_else(|| "bulk prices unavailable".into())),
    }
}

/// Per-cache freshness for the status chip: which rung each cache last answered
/// from, when it was last updated, and what went wrong if anything did.
#[tauri::command]
pub(crate) fn get_cache_statuses() -> HashMap<String, cache::CacheStatus> {
    cache::statuses()
}

/// Bring every cache due at once, ignoring both TTLs and ETags. The scheduler
/// picks this up on its next tick, so the work happens off the UI thread.
#[tauri::command]
pub(crate) fn refresh_all_caches() {
    refresh::force_all();
}

/// Publish bulk prices into the shared state. `overwrite_slugs` decides whether
/// a slug already priced by a per-item WFM lookup gets replaced — a manual
/// refresh replaces, the background one defers to the fresher single lookup.
fn apply_bulk_prices(state: &AppState, prices: BulkPrices, overwrite_slugs: bool) {
    if prices.by_name.is_empty() {
        return;
    }
    *state.relics_run_prices.lock().unwrap_or_else(|e| e.into_inner()) = prices.by_name;
    for (slug, price) in prices.by_slug {
        if overwrite_slugs || !state.wfm.is_price_cached(&slug) {
            state.wfm.cache_price(slug, Some(price));
        }
    }
}

const PRICING_BASE: &str = "https://raw.githubusercontent.com/WyrmStudios/FrameForgePricing/main";

pub(crate) const BULK_PRICES_CACHE: &str = "bulk-prices-v1.json";

/// The mirror republishes a few times a day; an hour keeps a long session from
/// tallying a relic run against yesterday's plat.
const BULK_PRICES_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

/// Median sell prices keyed two ways: display name for the relic-run tally,
/// WFM slug for seeding the per-item price cache.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct BulkPrices {
    pub(crate) by_name: HashMap<String, u32>,
    pub(crate) by_slug: HashMap<String, u32>,
}

/// Fetch items.json + the latest price_history from the FrameForgePricing mirror.
///
/// Both files are served without a usable ETag, and the pair only makes sense
/// together, so the conditional-GET slot goes unused here.
#[tracing::instrument(level = "debug", skip_all)]
fn fetch_relics_run_data(_etag: Option<&str>) -> Result<cache::Fetched<BulkPrices>, String> {
    // items.json gives the authoritative name → WFM slug mapping for every tradeable item.
    let items: Vec<serde_json::Value> = ureq::get(&format!("{}/items.json", PRICING_BASE))
        .call()
        .map_err(|e| format!("items.json: {e}"))?
        .into_json()
        .map_err(|e| format!("items.json: {e}"))?;
    let name_to_slug: HashMap<String, String> = items
        .into_iter()
        .filter_map(|v| {
            let name = v["i18n"]["en"]["name"].as_str()?.to_lowercase();
            let slug = v["slug"].as_str()?.to_string();
            Some((name, slug))
        })
        .collect();

    let price_json: serde_json::Value =
        ureq::get(&format!("{}/price_history_latest.json", PRICING_BASE))
            .call()
            .map_err(|e| format!("price_history_latest.json: {e}"))?
            .into_json()
            .map_err(|e| format!("price_history_latest.json: {e}"))?;

    let mut prices = BulkPrices::default();

    if let Some(obj) = price_json.as_object() {
        for (name, records) in obj {
            let price = records.as_array()
                .and_then(|arr| arr.iter()
                    .find(|r| r["order_type"].as_str() == Some("closed"))
                    .and_then(|r| r["median"].as_f64()));
            if let Some(p) = price {
                let price_u32 = p.round() as u32;
                let name_lower = name.to_lowercase();
                // Use authoritative slug from items.json; heuristic fallback for unknown items.
                let slug = name_to_slug.get(&name_lower)
                    .cloned()
                    .unwrap_or_else(|| to_wfm_slug(&name_lower));
                prices.by_name.insert(name_lower, price_u32);
                prices.by_slug.insert(slug, price_u32);
            }
        }
    }

    if prices.by_name.is_empty() {
        return Err("price history contains no closed-order medians".to_string());
    }
    Ok(cache::Fetched::New(prices, None))
}
