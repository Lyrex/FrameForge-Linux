//! Disk cache with a stale-while-revalidate ladder.
//!
//! Every cached payload carries the time it was retrieved and the ETag it came
//! with, so a conditional GET can be built from it; a 304 then bumps the file's
//! mtime instead of rewriting the payload. `get_or_refresh` walks four rungs in
//! order (fresh copy, successful refetch, stale copy, nothing) and reports
//! which one answered so the UI can say how old what it shows is.
//!
//! The schema version belongs in the file name ("catalogue-v1.json"): a payload
//! whose shape changed simply misses instead of deserializing into the wrong
//! thing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::paths;

pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);

    // Security software (Defender, Proton Drive) can block new files in the
    // data directory for an unsigned dev binary while still allowing writes to
    // existing ones. A direct write is not atomic, but it beats losing user
    // state that nothing can rebuild.
    if let Err(e) = std::fs::write(&tmp, data) {
        let _ = std::fs::remove_file(&tmp);
        warn!(
            path = %path.display(),
            "atomic_write/write_tmp failed ({e}), falling back to direct write"
        );
        return std::fs::write(path, data);
    }

    // Must open with write access: FlushFileBuffers (sync_all) requires it on
    // Windows and returns ERROR_ACCESS_DENIED on a read-only handle.
    if let Err(e) = std::fs::OpenOptions::new()
        .write(true)
        .open(&tmp)
        .and_then(|f| f.sync_all())
    {
        let _ = std::fs::remove_file(&tmp);
        return Err(std::io::Error::new(e.kind(), format!("sync: {e}")));
    }

    // Windows Defender and file-sync tools
    // can hold the destination briefly without FILE_SHARE_DELETE, making
    // MoveFileExW return ERROR_ACCESS_DENIED.  A 50–150 ms pause outlasts most
    // scan windows.
    let mut result = Err(std::io::Error::other("rename: no attempts"));
    for delay_ms in [0u64, 50, 150] {
        if delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
        result = std::fs::rename(&tmp, path)
            .map_err(|e| std::io::Error::new(e.kind(), format!("rename: {e}")));
        if result.is_ok() {
            return result;
        }
    }
    let _ = std::fs::remove_file(&tmp);
    result
}

#[derive(Serialize, Deserialize)]
pub struct Cached<T> {
    pub retrieved_at_unix: u64,
    pub etag: Option<String>,
    pub data: T,
}

/// Which rung of the ladder produced the data the caller is holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Cache was inside its TTL, or the server confirmed it with a 304.
    Fresh,
    Refreshed,
    /// Cache is past its TTL and the refetch is still running.
    Refreshing,
    /// Cache is past its TTL and the refetch failed.
    Stale,
    /// Nothing on disk and the fetch failed. The caller has to invent something.
    Fallback,
}

pub enum Fetched<T> {
    New(T, Option<String>),
    NotModified,
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheStatus {
    pub source: Source,
    pub last_updated: Option<u64>,
    pub warning: Option<String>,
}

static STATUSES: Mutex<Option<HashMap<String, CacheStatus>>> = Mutex::new(None);

/// One lock per cache name, held for the length of a `get_or_refresh`. Two
/// callers after the same cache take turns, and the second one finds what the
/// first stored. A slow download of one cache never blocks another.
static REFRESHING: Mutex<Option<HashMap<String, Arc<Mutex<()>>>>> = Mutex::new(None);

fn refresh_lock(name: &str) -> Arc<Mutex<()>> {
    let mut guard = REFRESHING.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get_or_insert_with(HashMap::new)
        .entry(name.to_string())
        .or_default()
        .clone()
}

pub fn set_status(name: &str, status: CacheStatus) {
    if let Ok(mut guard) = STATUSES.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(name.to_string(), status);
    }
}

pub fn statuses() -> HashMap<String, CacheStatus> {
    STATUSES
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default()
}

fn path_of(name: &str) -> PathBuf {
    paths::cache_dir().join(name)
}

pub fn now_unix() -> u64 {
    unix_seconds(SystemTime::now())
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1)
        .max(1)
}

/// A 304 confirms the copy without rewriting it, so the confirmation lives in
/// the file's mtime (see `confirm`). A freshly stored payload has an mtime at
/// or after its embedded timestamp, so taking the later of the two never ages
/// a copy and a replacement cannot inherit its predecessor's confirmation.
pub fn load<T: DeserializeOwned>(name: &str) -> Option<Cached<T>> {
    let file = std::fs::File::open(path_of(name)).ok()?;
    let confirmed = file.metadata().and_then(|m| m.modified()).ok();
    match serde_json::from_reader::<_, Cached<T>>(std::io::BufReader::new(file)) {
        Ok(mut cached) => {
            if let Some(confirmed) = confirmed {
                cached.retrieved_at_unix = cached.retrieved_at_unix.max(unix_seconds(confirmed));
            }
            Some(cached)
        }
        Err(e) => {
            warn!("discarding unreadable cache {name}: {e}");
            None
        }
    }
}

pub fn store<T: Serialize>(name: &str, etag: Option<String>, data: &T) -> std::io::Result<()> {
    store_at(name, etag, data, now_unix())
}

fn store_at<T: Serialize>(
    name: &str,
    etag: Option<String>,
    data: &T,
    retrieved_at_unix: u64,
) -> std::io::Result<()> {
    let cached = Cached {
        retrieved_at_unix,
        etag,
        data,
    };
    let body = serde_json::to_vec(&cached)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    atomic_write(&path_of(name), &body)
}

fn confirm(name: &str, unix: u64) -> std::io::Result<()> {
    // Write access: SetFileTime needs FILE_WRITE_ATTRIBUTES on Windows.
    std::fs::OpenOptions::new()
        .write(true)
        .open(path_of(name))?
        .set_modified(UNIX_EPOCH + Duration::from_secs(unix))
}

/// Serve `name`, refetching when it is older than `ttl`.
///
/// `fetch` receives the cached ETag so it can ask the server whether anything
/// changed. Returns the data, the rung that supplied it, and, when the answer
/// is not current, what went wrong, for the caller to surface.
pub fn get_or_refresh<T>(
    name: &str,
    ttl: Duration,
    fetch: impl FnOnce(Option<&str>) -> Result<Fetched<T>, String>,
) -> (Option<T>, Source, Option<String>)
where
    T: Serialize + DeserializeOwned,
{
    get_or_refresh_at(name, ttl, fetch, now_unix)
}

fn get_or_refresh_at<T>(
    name: &str,
    ttl: Duration,
    fetch: impl FnOnce(Option<&str>) -> Result<Fetched<T>, String>,
    now: impl FnOnce() -> u64,
) -> (Option<T>, Source, Option<String>)
where
    T: Serialize + DeserializeOwned,
{
    // The frontend and the background scheduler both ask for the same caches at
    // launch. Without this they download the catalogue twice and race each other
    // writing the same temporary file.
    let lock = refresh_lock(name);
    let _refreshing = lock.lock().unwrap_or_else(|e| e.into_inner());

    let cached = load::<T>(name);
    let now = now();
    // Strict: `Duration::ZERO` must always refetch, including when the clock
    // has stepped backwards past the stored timestamp.
    let still_fresh = cached
        .as_ref()
        .is_some_and(|c| now.saturating_sub(c.retrieved_at_unix) < ttl.as_secs());
    if still_fresh {
        let c = cached.expect("still_fresh is only true for a loaded cache");
        return report(
            name,
            Some(c.retrieved_at_unix),
            Source::Fresh,
            None,
            Some(c.data),
        );
    }

    set_status(
        name,
        CacheStatus {
            source: Source::Refreshing,
            last_updated: cached.as_ref().map(|c| c.retrieved_at_unix),
            warning: None,
        },
    );
    let result = fetch(cached.as_ref().and_then(|c| c.etag.as_deref()));
    match result {
        Ok(Fetched::New(data, etag)) => {
            if let Err(e) = store_at(name, etag, &data, now) {
                warn!("cannot write cache {name}: {e}");
            }
            report(name, Some(now), Source::Refreshed, None, Some(data))
        }
        // 304 only means anything against a cached copy; without one there is
        // nothing to confirm, and the fetcher had no ETag to send in the first
        // place.
        Ok(Fetched::NotModified) => match cached {
            Some(c) => {
                if let Err(e) = confirm(name, now) {
                    warn!("cannot confirm cache {name}: {e}");
                }
                report(name, Some(now), Source::Fresh, None, Some(c.data))
            }
            None => {
                let warning = format!("{name}: server reported not-modified with no cached copy");
                report(name, None, Source::Fallback, Some(warning), None)
            }
        },
        Err(e) => match cached {
            Some(c) => {
                let warning = format!("{name}: showing cached data, refresh failed: {e}");
                report(
                    name,
                    Some(c.retrieved_at_unix),
                    Source::Stale,
                    Some(warning),
                    Some(c.data),
                )
            }
            None => {
                let warning = format!("{name}: no cached data and refresh failed: {e}");
                report(name, None, Source::Fallback, Some(warning), None)
            }
        },
    }
}

fn report<T>(
    name: &str,
    last_updated: Option<u64>,
    source: Source,
    warning: Option<String>,
    data: Option<T>,
) -> (Option<T>, Source, Option<String>) {
    set_status(
        name,
        CacheStatus {
            source,
            last_updated,
            warning: warning.clone(),
        },
    );
    (data, source, warning)
}

/// Bodies past this are refused as runaway responses. The catalogue sources are
/// the largest bodies we pull, and All.json alone is ~30 MB.
const MAX_BODY_BYTES: u64 = 256 * 1024 * 1024;

/// For the multi-megabyte JSON listings (WFM `/v2/items`, the pricing mirror)
/// that ureq's default 10 MB `read_json` cap would otherwise refuse as they
/// grow.
pub const BULK_JSON_BYTES: u64 = 64 * 1024 * 1024;

/// GET `url`, asking the server to skip the body when `etag` still matches.
pub fn get_conditional(url: &str, etag: Option<&str>) -> Result<Fetched<String>, String> {
    let mut req = ureq::get(url)
        .header(
            "User-Agent",
            concat!("FrameForge/", env!("CARGO_PKG_VERSION")),
        )
        // Generous because of the body sizes, but bounded: ureq has no default
        // timeout, and a black-holed connection here would otherwise hang the
        // caller, and with it the refresh lock, forever.
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(300)))
        .build();
    if let Some(tag) = etag {
        req = req.header("If-None-Match", tag);
    }
    match req.call() {
        // Only 4xx and 5xx become errors, so a confirmed copy arrives here.
        Ok(resp) if resp.status() == 304 => Ok(Fetched::NotModified),
        Ok(resp) => {
            let etag = resp
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            // Not `read_to_string()`: ureq caps that at 10 MB.
            use std::io::Read;
            // Capped so a lying Content-Length cannot make us allocate the
            // whole cap up front.
            let hint = resp
                .body()
                .content_length()
                .unwrap_or(0)
                .min(64 * 1024 * 1024) as usize;
            let mut body = Vec::with_capacity(hint);
            // One byte past the cap so a body of exactly the cap's size is
            // distinguishable from a truncated one.
            resp.into_body()
                .into_reader()
                .take(MAX_BODY_BYTES + 1)
                .read_to_end(&mut body)
                .map_err(|e| e.to_string())?;
            if body.len() as u64 > MAX_BODY_BYTES {
                return Err(format!(
                    "{url}: response body exceeds {MAX_BODY_BYTES} bytes"
                ));
            }
            let body = String::from_utf8(body).map_err(|e| e.to_string())?;
            Ok(Fetched::New(body, etag))
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_modified_without_a_copy_is_a_fallback() {
        let name = scratch("not-modified-missing");
        let (data, source, warning) = get_or_refresh(&name, Duration::ZERO, |_| {
            Ok(Fetched::<String>::NotModified)
        });
        assert!(data.is_none());
        assert_eq!(source, Source::Fallback);
        assert!(warning
            .expect("missing copy is reported")
            .contains("no cached copy"));
    }

    #[test]
    fn replacement_does_not_inherit_previous_confirmation() {
        let name = scratch("replace-confirmed");
        store(&name, None, &"old").expect("test cache is writable");
        expire(&name);
        let confirmed = now_unix() + 1_000_000;
        get_or_refresh_at(
            &name,
            Duration::ZERO,
            |_| Ok(Fetched::<String>::NotModified),
            || confirmed,
        );
        assert_eq!(
            load::<String>(&name)
                .expect("cache exists")
                .retrieved_at_unix,
            confirmed
        );
        store(&name, Some("new-etag".into()), &"new").expect("replacement is writable");
        let cached = load::<String>(&name).expect("replacement loads");
        assert!(cached.retrieved_at_unix < confirmed);
        assert_eq!(cached.data, "new");
        assert_eq!(cached.etag.as_deref(), Some("new-etag"));
    }

    #[test]
    fn not_modified_updates_freshness_without_rewriting_payload() {
        let name = scratch("not-modified-bytes");
        store(&name, Some("catalogue-etag".into()), &"catalogue").expect("test cache is writable");
        expire(&name);
        let before = std::fs::read(path_of(&name)).expect("cache exists");
        let (data, source, warning) = get_or_refresh_at(
            &name,
            Duration::ZERO,
            |_| Ok(Fetched::<String>::NotModified),
            || 100,
        );
        assert_eq!(data.as_deref(), Some("catalogue"));
        assert_eq!(source, Source::Fresh);
        assert!(warning.is_none());
        assert_eq!(std::fs::read(path_of(&name)).expect("cache exists"), before);
        let cached = load::<String>(&name).expect("cache remains readable");
        assert_eq!(cached.retrieved_at_unix, 100);
        assert_eq!(cached.etag.as_deref(), Some("catalogue-etag"));
    }

    #[test]
    fn large_catalogue_round_trips() {
        let name = scratch("large-catalogue");
        let catalogue = vec!["/Lotus/Weapons/Tenno/Rifle/Braton".repeat(160); 1024];
        store(&name, Some("catalogue-etag".into()), &catalogue).expect("test cache is writable");
        let cached = load::<Vec<String>>(&name).expect("catalogue loads");
        assert!(cached.data == catalogue);
        assert_eq!(cached.etag.as_deref(), Some("catalogue-etag"));
    }

    #[test]
    fn missing_and_malformed_caches_are_misses() {
        let name = scratch("malformed-catalogue");
        assert!(load::<Vec<String>>(&name).is_none());
        for body in [b"{\"data\":".as_slice(), b"\xff", b"{} trailing"] {
            atomic_write(&path_of(&name), body).expect("test cache is writable");
            assert!(load::<Vec<String>>(&name).is_none());
        }
    }

    #[test]
    fn broken_clock_serves_existing_cache_without_refetching() {
        let name = scratch("broken-clock");
        store(&name, None, &"cached").expect("test cache is writable");
        let now = unix_seconds(UNIX_EPOCH - Duration::from_secs(1));
        assert_eq!(now, 1);
        assert_eq!(unix_seconds(UNIX_EPOCH), 1);
        let (data, source, _) = get_or_refresh_at::<String>(
            &name,
            Duration::from_secs(60),
            |_| panic!("broken clock must not refetch"),
            || now,
        );
        assert_eq!(source, Source::Fresh);
        assert_eq!(data.as_deref(), Some("cached"));
    }

    #[test]
    fn ttl_boundary_refetches() {
        let name = scratch("ttl-boundary");
        store(&name, None, &"cached").expect("test cache is writable");
        let retrieved = load::<String>(&name)
            .expect("cache exists")
            .retrieved_at_unix;
        let (data, source, _) = get_or_refresh_at::<String>(
            &name,
            Duration::from_secs(60),
            fetches("refetched"),
            || retrieved + 60,
        );
        assert_eq!(source, Source::Refreshed);
        assert_eq!(data.as_deref(), Some("refetched"));
    }

    #[test]
    fn zero_ttl_refetches_a_copy_stored_this_second() {
        let name = scratch("zero-ttl");
        store(&name, None, &"cached").expect("test cache is writable");
        let (_, source, _) = get_or_refresh(&name, Duration::ZERO, fetches("refetched"));
        assert_eq!(source, Source::Refreshed);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_destination_when_temp_disappears() {
        use std::io::Read;
        use std::os::unix::ffi::OsStrExt;

        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tmp/atomic-write-missing-temp");
        // A run that died between mkfifo and remove_file leaves the pipe behind.
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("test directory is writable");
        let path = root.join("catalogue.json");
        let tmp = root.join("catalogue.json.tmp");
        std::fs::write(&path, b"original").expect("test directory is writable");
        let fifo = std::ffi::CString::new(tmp.as_os_str().as_bytes()).expect("path has no NUL");
        // A pipe holds the writer open until its temporary name has been removed.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        std::thread::scope(|threads| {
            let writer = threads.spawn(|| atomic_write(&path, &vec![b'x'; 1024 * 1024]));
            let mut reader = std::fs::File::open(&tmp).expect("writer opens the pipe");
            std::fs::remove_file(&tmp).expect("pipe is removable");
            reader
                .read_to_end(&mut Vec::new())
                .expect("writer closes the pipe");
            let error = writer
                .join()
                .expect("writer does not panic")
                .expect_err("temporary name was removed");
            assert!(error.to_string().starts_with("sync:"));
        });
        assert_eq!(std::fs::read(&path).expect("original exists"), b"original");
        assert!(!tmp.exists());
        std::fs::remove_dir_all(root).expect("test directory is removable");
    }

    #[test]
    fn atomic_write_cleans_up_after_rename_failure() {
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tmp/atomic-write-rename-failure");
        let path = root.join("catalogue.json");
        std::fs::create_dir_all(&path).expect("test directory is writable");
        let error = atomic_write(&path, b"replacement").expect_err("cannot replace a directory");
        assert!(error.to_string().starts_with("rename:"));
        assert!(path.is_dir());
        assert!(!root.join("catalogue.json.tmp").exists());
        std::fs::remove_dir_all(root).expect("test directory is removable");
    }

    #[test]
    fn atomic_writes_with_different_extensions_do_not_clobber_each_other() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tmp/atomic-write-extensions");
        std::fs::create_dir_all(&root).expect("test directory is writable");
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|threads| {
            for (extension, byte) in [("json", b'j'), ("bin", b'b')] {
                let root = &root;
                let barrier = &barrier;
                threads.spawn(move || {
                    let path = root.join(format!("foo-v1.{extension}"));
                    let body = vec![byte; 1024 * 1024];
                    barrier.wait();
                    for _ in 0..10 {
                        atomic_write(&path, &body).expect("independent write succeeds");
                        assert!(std::fs::read(&path).expect("cache exists") == body);
                    }
                });
            }
        });
        std::fs::remove_dir_all(root).expect("test directory is removable");
    }

    #[test]
    fn blocked_temp_creation_falls_back_to_a_direct_write() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tmp/atomic-write-failure");
        std::fs::create_dir_all(&root).expect("test directory is writable");
        let path = root.join("catalogue.json");
        std::fs::write(&path, b"original").expect("test directory is writable");
        for name in ["catalogue.tmp", "catalogue.json.tmp"] {
            std::fs::create_dir_all(root.join(name)).expect("test directory is writable");
        }

        atomic_write(&path, b"replacement").expect("direct write succeeds");
        assert_eq!(
            std::fs::read(&path).expect("destination exists"),
            b"replacement"
        );
        std::fs::remove_dir_all(root).expect("test directory is removable");
    }

    /// One process, one root: every test names its own cache file instead.
    fn scratch(name: &str) -> String {
        let root = std::env::temp_dir().join("frameforge-cache-tests");
        let _ = paths::set_root_override(root);
        let file = format!("{name}.json");
        let _ = std::fs::remove_file(paths::cache_dir().join(&file));
        file
    }

    fn expire(name: &str) {
        let mut cached = load::<String>(name).expect("cache exists");
        cached.retrieved_at_unix = 1;
        atomic_write(
            &path_of(name),
            &serde_json::to_vec(&cached).expect("cache serializes"),
        )
        .expect("test cache is writable");
        confirm(name, 1).expect("test cache records mtimes");
    }

    fn fetches(body: &str) -> impl FnOnce(Option<&str>) -> Result<Fetched<String>, String> + '_ {
        move |_| Ok(Fetched::New(body.to_string(), None))
    }

    fn fails(_: Option<&str>) -> Result<Fetched<String>, String> {
        Err("offline".to_string())
    }

    #[test]
    fn fresh_cache_skips_the_fetch() {
        let name = scratch("fresh");
        store(&name, None, &"cached".to_string()).unwrap();

        let (data, source, warning) =
            get_or_refresh::<String>(&name, Duration::from_secs(3600), |_| {
                panic!("a fresh cache must not be refetched")
            });

        assert_eq!(data.as_deref(), Some("cached"));
        assert_eq!(source, Source::Fresh);
        assert!(warning.is_none());
    }

    #[test]
    fn status_reads_refreshing_while_the_fetch_runs() {
        let name = scratch("refreshing");
        store(&name, None, &"old".to_string()).unwrap();
        expire(&name);

        let (_, source, _) = get_or_refresh(&name, Duration::ZERO, |_| {
            assert_eq!(statuses()[&name].source, Source::Refreshing);
            Ok(Fetched::New("new".to_string(), None))
        });

        assert_eq!(source, Source::Refreshed);
    }

    #[test]
    fn expired_cache_is_replaced_by_the_fetch() {
        let name = scratch("refreshed");
        store(&name, None, &"old".to_string()).unwrap();
        expire(&name);

        let (data, source, _) = get_or_refresh(&name, Duration::ZERO, fetches("new"));

        assert_eq!(data.as_deref(), Some("new"));
        assert_eq!(source, Source::Refreshed);
        assert_eq!(load::<String>(&name).unwrap().data, "new");
    }

    #[test]
    fn a_failed_refresh_still_serves_the_stale_copy() {
        let name = scratch("stale");
        store(&name, None, &"old".to_string()).unwrap();
        expire(&name);

        let (data, source, warning) = get_or_refresh(&name, Duration::ZERO, fails);

        assert_eq!(data.as_deref(), Some("old"));
        assert_eq!(source, Source::Stale);
        assert!(warning.unwrap().contains("offline"));
    }

    #[test]
    fn nothing_cached_and_no_network_leaves_the_caller_empty() {
        let name = scratch("fallback");

        let (data, source, warning) = get_or_refresh::<String>(&name, Duration::ZERO, fails);

        assert!(data.is_none());
        assert_eq!(source, Source::Fallback);
        assert!(warning.is_some());
    }

    #[test]
    fn not_modified_keeps_the_payload_and_clears_the_staleness() {
        let name = scratch("not-modified");
        store(&name, Some("abc".to_string()), &"body".to_string()).unwrap();
        expire(&name);
        let before = load::<String>(&name).unwrap().retrieved_at_unix;

        let seen_etag = Mutex::new(None);
        let (data, source, _) = get_or_refresh(&name, Duration::ZERO, |etag| {
            *seen_etag.lock().unwrap() = etag.map(str::to_string);
            Ok(Fetched::<String>::NotModified)
        });

        assert_eq!(seen_etag.into_inner().unwrap().as_deref(), Some("abc"));
        assert_eq!(data.as_deref(), Some("body"));
        assert_eq!(source, Source::Fresh);
        let after = load::<String>(&name).unwrap();
        assert_eq!(after.etag.as_deref(), Some("abc"));
        assert!(after.retrieved_at_unix >= before);
    }

    #[test]
    fn a_new_schema_version_misses_the_old_file() {
        let v1 = scratch("schema-v1");
        let v2 = scratch("schema-v2");
        store(&v1, None, &"old shape".to_string()).unwrap();

        let (data, source, _) =
            get_or_refresh(&v2, Duration::from_secs(3600), fetches("new shape"));

        assert_eq!(data.as_deref(), Some("new shape"));
        assert_eq!(source, Source::Refreshed);
    }

    #[test]
    fn two_callers_after_a_cold_cache_fetch_once() {
        let name = scratch("concurrent");
        static FETCHES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

        std::thread::scope(|s| {
            for _ in 0..2 {
                s.spawn(|| {
                    get_or_refresh(&name, Duration::from_secs(3600), |_| {
                        FETCHES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(50));
                        Ok(Fetched::New("body".to_string(), None))
                    })
                });
            }
        });

        assert_eq!(FETCHES.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn status_records_the_last_rung_taken() {
        let name = scratch("status");

        let _ = get_or_refresh::<String>(&name, Duration::ZERO, fails);

        let status = statuses().remove(&name).expect("status recorded");
        assert_eq!(status.source, Source::Fallback);
        assert!(status.warning.is_some());
    }
}
