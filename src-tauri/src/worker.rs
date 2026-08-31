//! The public-data cache worker, and the one place the app addresses it.
//!
//! Public reads — warframe.market prices, statistics and order books,
//! worldstate, the drop tables — go to the worker first, which collapses every
//! install's identical request into one upstream fetch. The worker is an
//! optimization, never a dependency: `get` answers `None` for anything short of
//! a body, and the caller then runs the direct upstream path it would have run
//! anyway.
//!
//! Only public data may pass through here. Logins, a user's own orders and
//! auctions, and official-API sync address their services directly, and the
//! seam takes a `Route` rather than a URL so no call site can route them by
//! accident.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The deployed worker. Overridden by `FRAMEFORGE_WORKER_URL` or the
/// `workerBaseUrl` setting, which is how a local `wrangler dev` on
/// `http://127.0.0.1:8787` gets used instead. An empty value turns the worker
/// off and leaves every fetch on its direct upstream.
const DEFAULT_BASE_URL: &str = "https://frameforge-cache.lyrex.workers.dev";

/// Short enough that a hung worker costs less than the upstream fetch that
/// follows it.
const TIMEOUT: Duration = Duration::from_secs(10);

/// A connection failure or a 5xx is a transient fault, so the worker is retried
/// soon — but not on the very next request, which would put the timeout in
/// front of every fetch while the worker is down.
const FAULT_BACKOFF: Duration = Duration::from_secs(60);

const UNAVAILABLE_HEADER: &str = "X-FrameForge-Worker";
const UNAVAILABLE_VALUE: &str = "unavailable";

/// Read-through bodies under `/v1/` are the upstream's own, so a caller parses
/// exactly what it would parse talking to the upstream direct. The worker-native
/// routes are the exception and carry shapes only the worker serves.
pub enum Route<'a> {
    WfmStatistics(&'a str),
    WfmOrders(&'a str),
    Worldstate,
    CatalogDrops,
    /// Worker-native: every tradeable item's price in one document, and the
    /// slug → name catalog that gives those prices their display names.
    Snapshot,
    WfmItems,
}

impl Route<'_> {
    fn path(&self) -> String {
        match *self {
            Route::WfmStatistics(slug) => format!("/v1/wfm/items/{slug}/statistics"),
            Route::WfmOrders(slug) => format!("/v1/wfm/items/{slug}/orders"),
            Route::Worldstate => "/v1/worldstate".to_string(),
            Route::CatalogDrops => "/v1/catalog/drops".to_string(),
            Route::Snapshot => "/v1/snapshot".to_string(),
            Route::WfmItems => "/v1/wfm-items".to_string(),
        }
    }
}

pub struct Body {
    pub bytes: Vec<u8>,
    pub etag: Option<String>,
}

/// Why a fetch through the seam ended the way it did. Every one but `Served`
/// leaves the caller on its direct upstream, and a log that cannot tell them
/// apart cannot answer whether a slow session was the worker's doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Served,
    /// The worker is off: no base URL configured.
    Disabled,
    StandingDown,
    Unreachable,
    Fault,
    /// A 4xx, which is an answer about the request rather than a fault.
    Rejected,
    Unparseable,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Served => "served",
            Outcome::Disabled => "disabled",
            Outcome::StandingDown => "standing_down",
            Outcome::Unreachable => "unreachable",
            Outcome::Fault => "fault",
            Outcome::Rejected => "rejected",
            Outcome::Unparseable => "unparseable",
        }
    }
}

/// Fetch `route` from the worker. `None` means the caller must use its direct
/// upstream path.
#[tracing::instrument(level = "debug", skip_all, fields(route = %route.path(), outcome))]
pub fn get(route: Route<'_>) -> Option<Body> {
    let (body, outcome) = client().get(route);
    tracing::Span::current().record("outcome", outcome.as_str());
    body
}

/// `get`, deserialized. A body that does not parse answers `None` like a body
/// that never arrived: the caller's upstream path fetches the same document,
/// and its answer is the one worth failing on.
#[tracing::instrument(level = "debug", skip_all, fields(route = %route.path(), outcome))]
pub fn get_json<T: serde::de::DeserializeOwned>(route: Route<'_>) -> Option<T> {
    let span = tracing::Span::current();
    // A miss is already classified by `get`'s own span, so only the parse
    // verdict is left to record here.
    let (value, outcome) = parse(&get(route)?.bytes);
    span.record("outcome", outcome.as_str());
    value
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> (Option<T>, Outcome) {
    match serde_json::from_slice(bytes) {
        Ok(value) => (Some(value), Outcome::Served),
        Err(e) => {
            tracing::debug!(error = %e, "cache worker body unusable");
            (None, Outcome::Unparseable)
        }
    }
}

/// How many public fetches this session the worker answered, and how many fell
/// through to a direct upstream — the pair that says whether the layer is
/// doing anything for this install.
pub fn tally() -> (u64, u64) {
    let client = client();
    (client.served.load(Ordering::Relaxed), client.fell_back.load(Ordering::Relaxed))
}

// ==============================================================================
// The client
// ==============================================================================

struct Reply {
    status: u16,
    /// The worker's explicit stand-down signal. A relayed upstream 503 never
    /// carries the header, so it is not mistaken for one.
    unavailable: bool,
    etag: Option<String>,
    bytes: Vec<u8>,
}

type Transport = Box<dyn Fn(&str) -> Result<Reply, String> + Send + Sync>;

struct Client {
    base: String,
    transport: Transport,
    stand_down_until: Mutex<Option<Instant>>,
    served: AtomicU64,
    fell_back: AtomicU64,
}

impl Client {
    fn get(&self, route: Route<'_>) -> (Option<Body>, Outcome) {
        let (body, outcome) = self.attempt(route);
        let counter = if outcome == Outcome::Served { &self.served } else { &self.fell_back };
        counter.fetch_add(1, Ordering::Relaxed);
        (body, outcome)
    }

    fn attempt(&self, route: Route<'_>) -> (Option<Body>, Outcome) {
        if self.base.is_empty() {
            return (None, Outcome::Disabled);
        }
        if self.standing_down() {
            return (None, Outcome::StandingDown);
        }
        let url = format!("{}{}", self.base, route.path());
        match (self.transport)(&url) {
            Ok(reply) if reply.unavailable => {
                // The signal means the worker has spent its daily budget, so
                // asking again before the budget resets can only waste a round
                // trip per fetch.
                self.stand_down("out of budget until the daily reset", until_daily_reset());
                (None, Outcome::StandingDown)
            }
            Ok(reply) if reply.status == 200 => {
                (Some(Body { bytes: reply.bytes, etag: reply.etag }), Outcome::Served)
            }
            Ok(reply) if reply.status >= 500 => {
                self.stand_down("server error", FAULT_BACKOFF);
                (None, Outcome::Fault)
            }
            // A 4xx is an answer about the request rather than a fault, and the
            // upstream will give the same one with its own detail.
            Ok(_) => (None, Outcome::Rejected),
            Err(e) => {
                tracing::debug!(error = %e, "cache worker unreachable");
                self.stand_down("unreachable", FAULT_BACKOFF);
                (None, Outcome::Unreachable)
            }
        }
    }

    fn standing_down(&self) -> bool {
        let mut until = self.stand_down_until.lock().unwrap_or_else(|e| e.into_inner());
        match *until {
            Some(deadline) if Instant::now() < deadline => true,
            Some(_) => {
                *until = None;
                tracing::info!("cache worker back in use");
                false
            }
            None => false,
        }
    }

    /// Latching is the transition worth a log line: a worker that is down for
    /// an hour would otherwise write one warning per fetch for that hour.
    fn stand_down(&self, reason: &str, for_: Duration) {
        let mut until = self.stand_down_until.lock().unwrap_or_else(|e| e.into_inner());
        if until.is_none() {
            tracing::warn!(
                reason,
                seconds = for_.as_secs(),
                "cache worker stood down; using upstreams directly"
            );
        }
        *until = Some(Instant::now() + for_);
    }
}

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| Client {
        base: base_url(),
        transport: Box::new(fetch),
        stand_down_until: Mutex::new(None),
        served: AtomicU64::new(0),
        fell_back: AtomicU64::new(0),
    })
}

/// The worker's budget resets daily, so a stand-down lasts until UTC midnight —
/// the longest the signal can mean and the shortest that avoids re-asking a
/// worker that has already said no.
fn until_daily_reset() -> Duration {
    const DAY: u64 = 24 * 60 * 60;
    // A clock reading before the epoch says nothing about how far into the day
    // the reset is, and the whole day is the safe guess: the signal already
    // means the budget is spent, so every ask before it resets is a wasted
    // round trip.
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    Duration::from_secs(DAY - now % DAY)
}

fn base_url() -> String {
    if let Ok(url) = std::env::var("FRAMEFORGE_WORKER_URL") {
        return url.trim_end_matches('/').to_string();
    }
    let settings = crate::paths::config_dir().join("settings.json");
    crate::read_settings_map(&settings)
        .ok()
        .and_then(|m| m.get("workerBaseUrl")?.as_str().map(str::to_string))
        .map(|url| url.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// Every header the client sends. The contract admits the release version and
/// nothing else: no account, session, or machine identifier belongs here, and
/// the worker keeps no record of who asked for what.
fn request_headers() -> [(&'static str, &'static str); 1] {
    [("X-FrameForge-Version", env!("CARGO_PKG_VERSION"))]
}

fn fetch(url: &str) -> Result<Reply, String> {
    let mut req = ureq::get(url).timeout(TIMEOUT);
    for (name, value) in request_headers() {
        req = req.set(name, value);
    }
    let resp = match req.call() {
        Ok(resp) => resp,
        // A status the worker chose is an answer, not a transport failure.
        Err(ureq::Error::Status(_, resp)) => resp,
        Err(e) => return Err(e.to_string()),
    };
    let status = resp.status();
    let unavailable = resp.header(UNAVAILABLE_HEADER) == Some(UNAVAILABLE_VALUE);
    let etag = resp.header("etag").map(str::to_string);
    // The drop tables alone are ~30 MB, so the cap only guards against a
    // runaway response.
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(256 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(Reply { status, unavailable, etag, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const WORKER_BODY: &str = r#"{"from":"worker"}"#;
    const UPSTREAM_BODY: &str = r#"{"from":"upstream"}"#;

    /// The URLs the transport was asked for, so a test can see whether the
    /// worker was tried at all.
    type Attempts = Arc<Mutex<Vec<String>>>;

    fn client_over(reply: impl Fn() -> Result<Reply, String> + Send + Sync + 'static) -> (Client, Attempts) {
        let attempts: Attempts = Arc::default();
        let recorded = Arc::clone(&attempts);
        let client = Client {
            base: "https://worker.test".to_string(),
            transport: Box::new(move |url| {
                recorded.lock().expect("test transport is not poisoned").push(url.to_string());
                reply()
            }),
            stand_down_until: Mutex::new(None),
            served: AtomicU64::new(0),
            fell_back: AtomicU64::new(0),
        };
        (client, attempts)
    }

    fn reply(status: u16, unavailable: bool, body: &str) -> Result<Reply, String> {
        Ok(Reply { status, unavailable, etag: None, bytes: body.as_bytes().to_vec() })
    }

    /// A call site: worker body if there is one, today's upstream fetch if not.
    fn through_seam(client: &Client, route: Route<'_>) -> String {
        match client.get(route).0 {
            Some(body) => String::from_utf8(body.bytes).expect("test bodies are UTF-8"),
            None => UPSTREAM_BODY.to_string(),
        }
    }

    #[test]
    fn a_worker_body_is_served_instead_of_the_upstream() {
        let (client, attempts) = client_over(|| reply(200, false, WORKER_BODY));
        assert_eq!(through_seam(&client, Route::Worldstate), WORKER_BODY);
        assert_eq!(attempts.lock().expect("not poisoned")[0], "https://worker.test/v1/worldstate");
    }

    #[test]
    fn an_unreachable_worker_leaves_the_upstream_result_unchanged() {
        let (client, _) = client_over(|| Err("connection refused".to_string()));
        assert_eq!(through_seam(&client, Route::WfmOrders("mirage_prime_set")), UPSTREAM_BODY);
    }

    #[test]
    fn a_worker_error_leaves_the_upstream_result_unchanged() {
        let (client, _) = client_over(|| reply(503, false, r#"{"error":"upstream_unreachable"}"#));
        assert_eq!(through_seam(&client, Route::WfmStatistics("mirage_prime_set")), UPSTREAM_BODY);
    }

    /// A transient fault is retried after the backoff, so it must not latch the
    /// way the stand-down signal does.
    #[test]
    fn a_worker_error_does_not_stand_the_worker_down_for_the_day() {
        let (client, _) = client_over(|| reply(500, false, ""));
        client.get(Route::Worldstate);
        let deadline = client.stand_down_until.lock().expect("not poisoned").expect("a fault sets a backoff");
        assert!(deadline <= Instant::now() + FAULT_BACKOFF);
    }

    #[test]
    fn the_unavailable_signal_stops_the_worker_being_asked_again() {
        let (client, attempts) = client_over(|| reply(503, true, r#"{"error":"worker_unavailable"}"#));

        assert_eq!(through_seam(&client, Route::Worldstate), UPSTREAM_BODY);
        assert_eq!(through_seam(&client, Route::Worldstate), UPSTREAM_BODY);

        assert_eq!(attempts.lock().expect("not poisoned").len(), 1, "the worker must not be asked again");
        let deadline = client.stand_down_until.lock().expect("not poisoned").expect("the signal stands the worker down");
        assert!(deadline > Instant::now() + FAULT_BACKOFF, "the signal lasts until the daily reset, not a backoff");
    }

    /// The account-carrying endpoints, which stay on their services permanently.
    /// The seam takes a `Route`, so the guard is that no route mirrors one of
    /// these paths — adding, say, a route for the user's own orders would fail
    /// here.
    #[test]
    fn account_endpoints_are_never_routed_through_the_worker() {
        const ACCOUNT_PATHS: [&str; 8] = [
            "/v1/auth/signin",
            "/auth/refresh",
            "/v2/me",
            "/v2/orders",
            "/v2/orders/my",
            "/v1/auctions/create",
            "/api/login.php",
            "/api/inventory.php",
        ];
        let routed = [
            Route::WfmStatistics("mirage_prime_set").path(),
            Route::WfmOrders("mirage_prime_set").path(),
            Route::Worldstate.path(),
            Route::CatalogDrops.path(),
            Route::Snapshot.path(),
            Route::WfmItems.path(),
        ];
        for account in ACCOUNT_PATHS {
            assert!(
                !routed.iter().any(|path| path.ends_with(account)),
                "{account} must never be addressed through the worker"
            );
        }
    }

    /// The five ways a fetch can end. Without the distinction a log says only
    /// that the upstream was used, never why.
    #[test]
    fn every_fallback_reason_is_classified_separately() {
        let (client, _) = client_over(|| reply(200, false, WORKER_BODY));
        assert_eq!(client.get(Route::Worldstate).1, Outcome::Served);

        let (client, _) = client_over(|| Err("connection refused".to_string()));
        assert_eq!(client.get(Route::Worldstate).1, Outcome::Unreachable);

        let (client, _) = client_over(|| reply(503, false, ""));
        assert_eq!(client.get(Route::Worldstate).1, Outcome::Fault);

        let (client, _) = client_over(|| reply(503, true, ""));
        assert_eq!(client.get(Route::Worldstate).1, Outcome::StandingDown);
        assert_eq!(client.get(Route::Worldstate).1, Outcome::StandingDown);

        assert_eq!(parse::<serde_json::Value>(b"not json").1, Outcome::Unparseable);
    }

    /// Entering and leaving are what a log line is worth writing for, so both
    /// have to be visible in the latch rather than inferred from a timer.
    #[test]
    fn a_stand_down_is_entered_once_and_cleared_when_it_expires() {
        let (client, _) = client_over(|| reply(500, false, ""));

        client.get(Route::Worldstate);
        assert!(client.stand_down_until.lock().expect("not poisoned").is_some());

        // Expire it in place: the seam has no clock to wind forward, and the
        // transition is the state change, not the wait.
        *client.stand_down_until.lock().expect("not poisoned") = Some(Instant::now());
        assert!(!client.standing_down(), "an expired stand-down is over");
        assert!(
            client.stand_down_until.lock().expect("not poisoned").is_none(),
            "leaving must clear the latch, so the next fault is a fresh transition"
        );
    }

    #[test]
    fn the_tally_separates_worker_answers_from_fallbacks() {
        let (client, _) = client_over(|| reply(404, false, ""));
        client.get(Route::Worldstate);
        assert_eq!(client.served.load(Ordering::Relaxed), 0);
        assert_eq!(client.fell_back.load(Ordering::Relaxed), 1);

        let (client, _) = client_over(|| reply(200, false, WORKER_BODY));
        client.get(Route::Worldstate);
        client.get(Route::Snapshot);
        assert_eq!(client.served.load(Ordering::Relaxed), 2);
        assert_eq!(client.fell_back.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn requests_carry_the_app_version_and_nothing_that_identifies_the_user() {
        assert_eq!(request_headers(), [("X-FrameForge-Version", env!("CARGO_PKG_VERSION"))]);
    }
}
