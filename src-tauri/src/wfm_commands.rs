use tracing::warn;
use tauri::{Emitter, Manager, State};
use crate::app_state::AppState;
use crate::cache::atomic_write;
use crate::catalogue::fix_category;
use crate::credentials::wfm_delete_credentials;
use crate::inventory_state::{load_inventory_state_cache, CachedItem};
use crate::resolver::ItemResolver;
use crate::wfm::{to_wfm_slug, WfmItem, WfmPrice, WfmRivenAttribute};

// The WFM client lives in `wfm.rs`; the command handlers below are thin adapters
// over `state.wfm`. Session acquisition (this login webview) stays here at
// the Tauri boundary.

/// Open warframe.market signin in an embedded WebView.
/// Emits `wfm-login-window-closed` if the window is closed before auth completes.
#[tauri::command]
pub(crate) fn wfm_open_login_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window("wfm-login") {
        let _ = existing.set_focus();
        return Ok(());
    }
    let win = open_wfm_webview(&app, "https://warframe.market/auth/signin")?;
    let app2 = app.clone();
    win.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let _ = app2.emit("wfm-login-window-closed", ());
        }
    });
    Ok(())
}

/// Close the WFM login popup programmatically (e.g. after an auto-timeout).
#[tauri::command]
pub(crate) fn wfm_close_login_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("wfm-login") {
        let _ = win.close();
    }
    Ok(())
}

/// Opens a configured WebView at `start_url` with the shared injection script.
fn open_wfm_webview(app: &tauri::AppHandle, start_url: &str) -> Result<tauri::WebviewWindow, String> {
    static SCRIPT: &str = r#"
(function() {
  // ── Anti-detection ──────────────────────────────────────────────────────────
  // WebView2 signals that Steam/Xbox/Discord use to show blank pages:
  //   navigator.webdriver      = true   → automation flag
  //   window.chrome.webview    = object → WebView2-specific object
  //   navigator.userAgentData  exposes brand "Microsoft Edge WebView2"
  //   navigator.languages      often missing or wrong
  try { Object.defineProperty(navigator, 'webdriver', { get: function(){ return undefined; } }); } catch(e) {}
  try { if (window.chrome && window.chrome.webview) { delete window.chrome.webview; } } catch(e) {}
  try {
    Object.defineProperty(navigator, 'languages', { get: function(){ return ['en-US','en']; } });
  } catch(e) {}
  // Override userAgentData so brands list looks like real Chrome, not WebView2.
  try {
    var _uaBrands = [
      { brand: 'Google Chrome',  version: '125' },
      { brand: 'Chromium',       version: '125' },
      { brand: 'Not/A)Brand',    version: '24'  },
    ];
    var _uaData = {
      brands:   _uaBrands,
      mobile:   false,
      platform: 'Windows',
      getHighEntropyValues: function(hints) {
        return Promise.resolve({
          architecture:    'x86',
          bitness:         '64',
          brands:          _uaBrands,
          fullVersionList: [
            { brand: 'Google Chrome',  version: '125.0.6422.141' },
            { brand: 'Chromium',       version: '125.0.6422.141' },
            { brand: 'Not/A)Brand',    version: '24.0.0.0'       },
          ],
          mobile:          false,
          model:           '',
          platform:        'Windows',
          platformVersion: '15.0.0',
          uaFullVersion:   '125.0.6422.141',
          wow64:           false,
        });
      },
      toJSON: function() {
        return { brands: _uaBrands, mobile: false, platform: 'Windows' };
      },
    };
    Object.defineProperty(navigator, 'userAgentData', { get: function(){ return _uaData; } });
  } catch(e) {}

  // ── Nav bar (only on external OAuth pages so user can always go back) ───────
  if (location.hostname !== 'warframe.market' && !location.hostname.endsWith('.warframe.market')) {
    function injectNavBar() {
      if (document.getElementById('__ff_nav') || !document.body) return;
      var bar = document.createElement('div');
      bar.id = '__ff_nav';
      bar.style.cssText = 'position:fixed;top:0;left:0;right:0;z-index:2147483647;height:32px;background:#1a1a2e;border-bottom:1px solid #333;display:flex;align-items:center;gap:6px;padding:0 8px;font-family:sans-serif;font-size:12px;color:#ccc;';
      function btn(label, action) {
        var b = document.createElement('button');
        b.textContent = label;
        b.style.cssText = 'background:#2a2a4a;border:1px solid #444;color:#ccc;padding:2px 10px;border-radius:4px;cursor:pointer;font-size:12px;';
        b.onmouseenter = function(){ b.style.background='#3a3a5a'; };
        b.onmouseleave = function(){ b.style.background='#2a2a4a'; };
        b.onclick = action;
        return b;
      }
      bar.appendChild(btn('← Back', function(){ history.back(); }));
      bar.appendChild(btn('⌂ Login page', function(){ window.location.href='https://warframe.market/auth/signin'; }));
      var lbl = document.createElement('span');
      lbl.style.cssText = 'flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;opacity:.5;font-size:11px;';
      lbl.textContent = location.hostname;
      bar.appendChild(lbl);
      var s = document.createElement('style');
      s.textContent = 'html{margin-top:32px!important}';
      document.head.appendChild(s);
      document.body.insertBefore(bar, document.body.firstChild);
    }
    if (document.body) injectNavBar(); else window.addEventListener('DOMContentLoaded', injectNavBar);
  }

  // ── WFM-only: token capture ─────────────────────────────────────────────────
  if (location.hostname !== 'warframe.market' && !location.hostname.endsWith('.warframe.market')) return;

  // Strip target="_blank" from all links before the user clicks them.
  // WebView2 fires NewWindowRequested at the native level before any JavaScript
  // click handler can call preventDefault — so a capture-phase interceptor is
  // always too late. Removing the target attribute in advance means WebView2
  // never sees target="_blank" and treats every link as a same-window navigation.
  // This keeps Steam/Xbox/Discord OAuth flows inside this configured window
  // (Chrome UA, anti-detection) instead of spawning a blank unconfigured popup.
  function stripTargets(root) {
    (root || document).querySelectorAll('a[target]').forEach(function(a) {
      a.removeAttribute('target');
      a.removeAttribute('rel');
    });
  }
  if (document.body) { stripTargets(); } else { window.addEventListener('DOMContentLoaded', function() { stripTargets(); }); }
  new MutationObserver(function(mutations) {
    mutations.forEach(function(m) {
      m.addedNodes.forEach(function(n) {
        if (n.nodeType !== 1) return;
        if (n.tagName === 'A') { n.removeAttribute('target'); n.removeAttribute('rel'); }
        if (n.querySelectorAll) { stripTargets(n); }
      });
    });
  }).observe(document.documentElement, { childList: true, subtree: true });

  // Backup: override window.open() for any JS-triggered popups.
  var _origOpen = window.open;
  window.open = function(url, target, features) {
    if (url && typeof url === 'string' && url.length > 0) {
      window.location.href = url;
      return null;
    }
    return _origOpen.apply(this, arguments);
  };

  var _clientId = '', _deviceId = '';
  function sendTokens(d, v1Jwt) {
    if (!d || !d.accessToken || window.__wfmDone) return;
    window.__wfmDone = true;
    setTimeout(function() {
      var csrfMeta = document.querySelector('meta[name="csrf-token"]');
      var csrf = csrfMeta ? csrfMeta.getAttribute('content') : '';
      if (window.__TAURI__) {
        window.__TAURI__.core.invoke('wfm_receive_tokens', {
          accessToken:  d.accessToken,
          refreshToken: d.refreshToken || '',
          clientId:     _clientId,
          deviceId:     _deviceId,
          v1Jwt:        v1Jwt || null,
          csrfToken:    csrf || null,
        }).catch(function() {});
      }
    }, 500);
  }
  var origFetch = window.fetch;
  window.fetch = function(input, init) {
    var url = typeof input === 'string' ? input : (input && input.url) || '';
    if (url.includes('/auth/signin') && init && init.body) {
      try { var b = JSON.parse(init.body); _clientId = b.clientId||''; _deviceId = b.deviceId||''; } catch(e) {}
    }
    var p = origFetch.apply(this, arguments);
    if (url.includes('/auth/')) {
      p.then(function(r) {
        var v1Jwt = r.headers.get('Authorization') || '';
        if (v1Jwt.startsWith('JWT ')) v1Jwt = v1Jwt.slice(4);
        r.clone().json().then(function(j) {
          if (j && j.data && j.data.accessToken) sendTokens(j.data, v1Jwt || null);
        }).catch(function(){});
      }).catch(function(){});
    }
    return p;
  };
  // Also capture device_id from the URL — used by OAuth flows that start
  // at /auth/steam?device_id=... instead of via the email/password form.
  try {
    var _urlDeviceId = new URLSearchParams(location.search).get('device_id');
    if (_urlDeviceId) _deviceId = _urlDeviceId;
  } catch(e) {}

  var origOpen = XMLHttpRequest.prototype.open;
  var origSend = XMLHttpRequest.prototype.send;
  var _xhrUrl = '';
  XMLHttpRequest.prototype.open = function(m, u) { _xhrUrl = u || ''; return origOpen.apply(this, arguments); };
  XMLHttpRequest.prototype.send = function(body) {
    if (_xhrUrl.includes('/auth/')) {
      var self = this;
      self.addEventListener('load', function() {
        try { var j = JSON.parse(self.responseText); if (j && j.data) sendTokens(j.data); } catch(e) {}
      });
      if (body) { try { var b = JSON.parse(body); _clientId = b.clientId||_clientId; _deviceId = b.deviceId||_deviceId; } catch(e) {} }
    }
    return origSend.apply(this, arguments);
  };
})();
"#;

    build_wfm_webview(app, start_url, SCRIPT)
}




fn build_wfm_webview(app: &tauri::AppHandle, url: &str, script: &str) -> Result<tauri::WebviewWindow, String> {
    tauri::WebviewWindowBuilder::new(
        app,
        "wfm-login",
        tauri::WebviewUrl::External(url.parse()
            .map_err(|e| format!("URL parse: {}", e))?),
    )
    .title("Log in to warframe.market")
    .inner_size(520.0, 760.0)
    .resizable(true)
    .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36")
    .devtools(true)
    .initialization_script(script)
    .build()
    .map_err(|e| format!("Window create: {}", e))
}

/// Legacy — the new injection script calls wfm_receive_tokens directly.
/// Kept so older injected scripts that only captured the JWT still work.
#[tauri::command]
pub(crate) fn wfm_receive_jwt(app: tauri::AppHandle, state: State<AppState>, jwt: String) -> Result<(), String> {
    wfm_receive_tokens(app, state, jwt, String::new(), String::new(), String::new(), None, None)
}

/// Receive tokens captured by the WebView injection script.
/// Calls /v2/me to get the username, stores session, closes login window.
#[tauri::command]
#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn wfm_receive_tokens(
    app: tauri::AppHandle, state: State<AppState>,
    access_token: String, refresh_token: String,
    client_id: String, device_id: String,
    #[allow(non_snake_case)] v1Jwt: Option<String>,
    #[allow(non_snake_case)] csrfToken: Option<String>,
) -> Result<(), String> {
    let (username, _status) = state.wfm.adopt_tokens(
        access_token, refresh_token, client_id, device_id,
        v1Jwt.unwrap_or_default(), csrfToken,
    )?;
    if let Some(win) = app.get_webview_window("wfm-login") { let _ = win.close(); }
    let _ = app.emit("wfm-auth-complete", &username);
    Ok(())
}

/// Use the stored refresh token to silently get a new access token.
#[tauri::command]
pub(crate) fn wfm_refresh_token(state: State<AppState>) -> Result<(), String> {
    state.wfm.refresh()
}

/// Restore a session from saved token data (JSON string).
/// Returns (username, status) so the frontend can set both in one step.
#[tauri::command]
pub(crate) fn wfm_set_jwt(state: State<AppState>, jwt: String) -> Result<(String, String), String> {
    // `jwt` is the JSON bundle saved by wfm_save_credentials (or, for old saves,
    // a bare access token — restore_from_json handles both).
    state.wfm.restore_from_json(&jwt)
}

/// Log in via v1 signin (current recommended method per WFM Discord).
/// Token is returned in the set-cookie header: "JWT=eyJ...; Path=/; ..."
/// Use it as: Authorization: Bearer <token>
#[tauri::command]
pub(crate) fn wfm_login(state: State<AppState>, email: String, password: String) -> Result<String, String> {
    state.wfm.login(&email, &password)
}

// The popup sends the info, statistics, and orders requests at the same time.
// Tauri runs sync commands one after another on the IPC thread. Each command
// here is async and runs on the blocking pool, so the three requests overlap.

/// Fetch current in-game buy and sell orders for an item, sorted by price.
/// When `mod_rank` is provided the results are filtered to that specific rank only.
#[tauri::command]
pub(crate) async fn wfm_get_item_orders(state: State<'_, AppState>, url_name: String, mod_rank: Option<u32>) -> Result<serde_json::Value, String> {
    let wfm = state.wfm.clone();
    tauri::async_runtime::spawn_blocking(move || wfm.item_orders(&url_name, mod_rank))
        .await
        .map_err(|e| e.to_string())?
}

/// Fetch 90-day price statistics for an item (daily medians for the chart).
#[tauri::command]
pub(crate) async fn wfm_get_item_statistics(state: State<'_, AppState>, url_name: String) -> Result<serde_json::Value, String> {
    let wfm = state.wfm.clone();
    tauri::async_runtime::spawn_blocking(move || wfm.item_statistics(&url_name))
        .await
        .map_err(|e| e.to_string())?
}

/// Clear the stored WFM session.
///
/// A saved token outlives the in-memory session: the next launch restores it
/// before the user sees anything, so a logout that only cleared memory would
/// appear not to have happened. The delete sits here rather than at the call
/// site so every route to a logout inherits it.
#[tauri::command]
pub(crate) async fn wfm_logout(state: State<'_, AppState>) -> Result<(), String> {
    state.wfm.clear_session();
    wfm_delete_credentials().await
}

/// Return (username, status) for the current session, or None if not logged in.
#[tauri::command]
pub(crate) fn wfm_get_session(state: State<AppState>) -> Option<(String, String)> {
    state.wfm.identity()
}

/// Fetch the user's actual current status from WFM (`/v2/me`).
/// Returns one of: "online" | "ingame" | "invisible" | "offline".
/// Call this after session restore so the UI reflects what WFM actually has,
/// not just the hardcoded default.
#[tauri::command]
pub(crate) fn wfm_fetch_status(state: State<AppState>) -> Result<String, String> {
    state.wfm.fetch_status()
}

/// Return the current session token data as JSON for saving.
#[tauri::command]
pub(crate) fn wfm_get_jwt(state: State<AppState>) -> Option<String> {
    state.wfm.token_json()
}

/// Fetch the authenticated user's active buy + sell orders.
#[tauri::command]
pub(crate) fn wfm_get_orders(state: State<AppState>) -> Result<serde_json::Value, String> {
    state.wfm.my_orders()
}

/// Set WFM online status via WebSocket.
/// Connects, authenticates, sends status with 6-hour duration, then disconnects.
/// The duration means status persists even after the connection closes.
/// Values: "online" | "ingame" | "invisible"
#[tauri::command]
pub(crate) async fn wfm_set_status(state: State<'_, AppState>, status: String) -> Result<(), String> {
    // The WebSocket round-trip is blocking; run it off the async runtime.
    let wfm = state.wfm.clone();
    tokio::task::spawn_blocking(move || wfm.set_status(&status))
        .await
        .map_err(|e| format!("Task: {}", e))?
}

/// Debug: return the raw JSON from any authenticated WFM endpoint.
#[tauri::command]
pub(crate) fn wfm_debug_dump(state: State<AppState>, path: String) -> Result<String, String> {
    state.wfm.debug_dump(&path)
}

/// Collect known riven attribute url_names by sampling real auction listings.
/// /v1/riven/attributes was removed; this scrapes url_names from search results instead.
/// Exposed so the browser console can call: window.__wfmAttrs()
#[tauri::command]
pub(crate) fn wfm_get_riven_attributes(state: State<AppState>) -> Result<Vec<String>, String> {
    state.wfm.riven_attributes()
}

/// Get the internal WFM item ID for a URL slug (needed to create orders).
/// Also returns `modMaxRank` from the local WFCD item cache when the item is a mod,
/// so the frontend never needs a second network request to detect this.
#[tauri::command]
pub(crate) async fn wfm_get_item_info(state: State<'_, AppState>, url_name: String) -> Result<serde_json::Value, String> {
    let wfm = state.wfm.clone();
    let mut data = tauri::async_runtime::spawn_blocking(move || wfm.item_info(&url_name))
        .await
        .map_err(|e| e.to_string())??;

    // Enrich with modMaxRank from inventory_state_cache.json — the canonical source.
    // Match by display name since url_name ↔ unique_name conversion isn't 1:1.
    if let Some(wfm_name) = data["i18n"]["en"]["name"].as_str()
        .or_else(|| data["name"].as_str())
    {
        let wfm_name_lc = wfm_name.to_lowercase();
        let inv = load_inventory_state_cache(&state.inventory_state_cache_path);
        if let Some(max_rank) = inv.items.values()
            .find(|item| item.name.to_lowercase() == wfm_name_lc)
            .and_then(|item| item.mod_max_rank)
        {
            data["modMaxRank"] = serde_json::json!(max_rank);
        }
    }

    Ok(data)
}

/// Create a new buy or sell order. `mod_rank` must be set for mods — WFM returns 400 without it.
#[tauri::command]
pub(crate) fn wfm_create_order(state: State<AppState>, item_id: String, order_type: String, platinum: u32, quantity: u32, visible: bool, mod_rank: Option<u32>) -> Result<serde_json::Value, String> {
    state.wfm.create_order(&item_id, &order_type, platinum, quantity, visible, mod_rank)
}

/// Update an existing order's price, quantity, or visibility.
#[tauri::command]
pub(crate) fn wfm_update_order(state: State<AppState>, order_id: String, platinum: u32, quantity: u32, visible: bool) -> Result<serde_json::Value, String> {
    state.wfm.update_order(&order_id, platinum, quantity, visible)
}

/// Delete an order.
#[tauri::command]
pub(crate) fn wfm_delete_order(state: State<AppState>, order_id: String) -> Result<(), String> {
    state.wfm.delete_order(&order_id)
}

/// Post a revealed riven as an auction on warframe.market.
#[tauri::command]
pub(crate) fn wfm_create_riven_auction(
    state: State<AppState>,
    weapon_url_name: String,
    riven_name: String,
    mastery_level: u32,
    mod_rank: u8,
    re_rolls: u32,
    polarity: String,
    attributes: Vec<WfmRivenAttribute>,
    starting_price: u32,
    buyout_price: Option<u32>,
    minimal_reputation: u32,
    note: String,
    visible: bool,
    is_direct_sell: bool,
) -> Result<serde_json::Value, String> {
    let json = state.wfm.create_riven_auction(
        &weapon_url_name, &riven_name, mastery_level, mod_rank, re_rolls, &polarity,
        &attributes, starting_price, buyout_price, minimal_reputation, &note, visible, is_direct_sell,
    )?;
    record_new_auction_id(&state, &json);
    Ok(json)
}

/// Fetch the current user's active riven auctions from warframe.market.
/// Tries v2 /auctions/my first (returns all including hidden); falls back to the v1 profile
/// endpoint which only returns visible auctions.
#[tauri::command]
pub(crate) async fn wfm_get_my_riven_auctions(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let stored_ids = state.auction_ids.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let wfm = state.wfm.clone();
    tauri::async_runtime::spawn_blocking(move || wfm.my_riven_auctions(&stored_ids))
        .await
        .map_err(|e| e.to_string())?
}

fn save_auction_ids(state: &State<AppState>) {
    let ids = state.auction_ids.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Ok(json) = serde_json::to_string(&ids) {
        let _ = atomic_write(&state.auction_ids_path, json.as_bytes());
    }
}

/// Record a newly created auction's id so hidden auctions survive restarts.
/// FrameForge-created auctions can be hidden, and the WFM profile endpoint only
/// lists visible ones, so their ids are the only way to fetch them back.
fn record_new_auction_id(state: &State<AppState>, json: &serde_json::Value) {
    if let Some(id) = json["payload"]["auction"]["id"].as_str() {
        let mut ids = state.auction_ids.lock().unwrap_or_else(|e| e.into_inner());
        if !ids.contains(&id.to_string()) {
            ids.push(id.to_string());
            drop(ids);
            save_auction_ids(state);
        }
    }
}

/// Switch a riven auction between Auction and Direct Sale types.
/// The close-and-recreate lives in `Wfm`; here we reconcile the stored auction
/// ids — drop the closed one, record its replacement.
#[tauri::command]
pub(crate) fn wfm_switch_riven_type(
    state: State<AppState>,
    auction_id: String,
    new_is_direct_sell: bool,
    starting_price: u32,
    buyout_price: Option<u32>,
    visible: bool,
) -> Result<serde_json::Value, String> {
    let json = state.wfm.switch_riven_type(&auction_id, new_is_direct_sell, starting_price, buyout_price, visible)?;
    // The old listing is now closed; drop its id and record the replacement.
    state.auction_ids.lock().unwrap_or_else(|e| e.into_inner()).retain(|id| id != &auction_id);
    save_auction_ids(&state);
    record_new_auction_id(&state, &json);
    Ok(json)
}

/// Delete a riven auction via the /close endpoint.
#[tauri::command]
pub(crate) fn wfm_delete_auction(state: State<AppState>, auction_id: String) -> Result<(), String> {
    state.wfm.delete_auction(&auction_id)?;
    state.auction_ids.lock().unwrap_or_else(|e| e.into_inner()).retain(|id| id != &auction_id);
    save_auction_ids(&state);
    Ok(())
}

/// Update a riven auction's starting price, buyout price, and visibility.
/// Sends PUT /v1/auctions/entry/{id}. Pass buyout_price=None to clear the buyout.
#[tauri::command]
pub(crate) fn wfm_update_auction(state: State<AppState>, auction_id: String, starting_price: u32, buyout_price: Option<u32>, visible: bool) -> Result<(), String> {
    state.wfm.update_auction(&auction_id, starting_price, buyout_price, visible)
}

/// Toggle visibility of a riven auction (visible / hidden).
#[tauri::command]
pub(crate) fn wfm_set_auction_visible(state: State<AppState>, auction_id: String, visible: bool) -> Result<(), String> {
    state.wfm.set_auction_visible(&auction_id, visible)
}

/// Fetch warframe.market item list using v2 API (v1 /items returns 404).
#[tauri::command]
pub(crate) fn fetch_wfm_items(state: State<AppState>) -> Result<Vec<WfmItem>, String> {
    state.wfm.items()
}

/// Fetch 48-hour median sell price for a single item from warframe.market.
/// Tries the slug as-is first, then retries with the Blueprint suffix added or
/// removed — WFM is inconsistent about whether component blueprints include it.
#[tauri::command]
pub(crate) fn fetch_wfm_price(state: State<AppState>, url_name: String) -> Result<WfmPrice, String> {
    let sell_median = state.wfm.price_with_fallback(&url_name)?.map(|p| p as f64);
    Ok(WfmPrice { url_name, sell_median, buy_median: None })
}

/// Fetch the 48-hour median sell price for an item by display name.
/// Results are cached in AppState so the overlay and main window share them.
/// Returns None when the item is not listed on warframe.market.
#[tauri::command]
pub(crate) fn get_item_price(item_name: String, state: State<AppState>) -> Result<Option<u32>, String> {
    // 1. Check relics.run bulk price cache (no network call needed)
    {
        let prices = state.relics_run_prices.lock().map_err(|e| e.to_string())?;
        let key = item_name.to_lowercase();
        if let Some(&price) = prices.get(&key) {
            return Ok(Some(price));
        }
    }

    let slug = to_wfm_slug(&item_name);

    if let Some(cached) = state.wfm.cached_price(&slug) {
        return Ok(cached);
    }

    // Only strip "_blueprint" here — never append it. This is called with inventory
    // display names, where a prime component's name carries "Blueprint" but WFM lists
    // it without the suffix. A non-blueprint name must NOT fall back to a _blueprint
    // slug, or a frame would be priced as its blueprint.
    let price = match state.wfm.price_for_slug(&slug)? {
        Some(p) => Some(p),
        None => match slug.strip_suffix("_blueprint") {
            Some(base) => state.wfm.price_for_slug(base)?,
            None => None,
        },
    };
    state.wfm.cache_price(slug, price);

    // Persist WFM price into the inventory cache file so it survives restarts.
    // Only write for tradeable items: prime parts/blueprints (have ducats) and mods/arcanes.
    if let Some(plat) = price {
        let cache_path = &state.inventory_state_cache_path;
        let mut inv = load_inventory_state_cache(cache_path);
        let items = state.wfcd_items.lock().map_err(|e| e.to_string())?;
        // Key the cache entry on the canonical unique_name, not the display string.
        let unique = ItemResolver::from_items(&items)
            .by_display(&item_name)
            .map(|r| r.unique_name.clone());
        if let Some(item) = unique.and_then(|u| items.iter().find(|i| i.unique_name == u)) {
            let cat = fix_category(&item.name, &item.item_type, &item.product_category, &item.category, &item.unique_name);
            let tradeable = item.ducats.is_some() || matches!(cat.as_str(), "Mods" | "Arcanes");
            if tradeable {
                inv.items.entry(item.unique_name.clone())
                    .or_insert_with(|| CachedItem { unique_name: item.unique_name.clone(), ..Default::default() })
                    .wfm_price = Some(plat);
                if let Ok(json) = serde_json::to_string(&inv) {
                    let _ = atomic_write(cache_path, json.as_bytes());
                }
            }
        }
    }

    Ok(price)
}
