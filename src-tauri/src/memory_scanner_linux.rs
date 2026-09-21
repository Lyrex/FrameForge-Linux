//! Reading Warframe's memory under Proton.
//!
//! The blob, the markers and the stitch engine live in `memory_scanner`, and
//! [`ProcessHandle`] does the reading. This module decides which mappings to
//! offer and in what order, and wraps them in a [`RegionSource`] the engine
//! can walk.

use memchr::memmem;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use crate::mem_regions::{ProcessRegions, RegionSource, RegionStats};
use crate::memory_scanner::{
    cold_log_search_due, looks_like_log_buffer, newest_sync_timestamp, probe_outcome,
    scan_cached_blob, stitch_blobs, sync_marker_is_new,
    BlobInventory, ScanOutcome, LAST_LOG_REGION, LOG_LINE_MARKER,
    LOG_SEARCH_BACKOFF, LOG_SEARCH_BACKOFF_PROBES, MAX_LOG_REGION, MAX_SCAN,
};
use crate::platform::{MemoryRegionInfo, Platform, ProcessAccess, ProcessHandle, RegionBacking};

// ==============================================================================
// Region source for the stitch engine
// ==============================================================================

/// Mappings reach the engine in pieces this size, so a multi-gigabyte arena
/// cannot exhaust the heap.
const WALK_CHUNK: usize = 64 * 1024 * 1024;

/// Stops the cached-blob stitch at a mapping it must not read through.
///
/// The seed always lands in a mapping the walk accepted, but the stitch runs
/// forward from there and the next mapping up may be code or a mapped file.
/// Both hold byte patterns that satisfy the anchor checks, so splicing them in
/// yields a blob that parses with the wrong contents. The guard returns empty
/// bytes at such a mapping so the stitch ends on the boundary.
///
/// TODO: a blob the file-backed tier found has no fast path at all this way,
/// and re-walks on every sync. Matching the tier that found it would need the
/// walk to remember which one that was.
struct StitchGuard(Box<dyn RegionSource>);

impl RegionSource for StitchGuard {
    fn next_region(&mut self) -> Option<(usize, &[u8])> {
        self.0.next_region()
    }

    fn region_at(&self, addr: usize) -> Option<MemoryRegionInfo> {
        self.0.region_at(addr)
    }

    fn read_at(&self, addr: usize, max_len: usize) -> Option<(usize, Vec<u8>)> {
        let region = self.region_at(addr)?;
        let end = region.base_address + region.region_size;
        if region.is_executable || region.backing != RegionBacking::Anonymous {
            return Some((end, Vec::new()));
        }
        self.0.read_at(addr, max_len.min(end - addr))
    }

    fn stats(&self) -> RegionStats {
        self.0.stats()
    }
}

// ==============================================================================
// Inventory blob capture
// ==============================================================================

/// Walk the game's mappings and stitch, parse and send every FULL_ACCOUNT
/// blob found.
///
/// File-backed mappings (PE data sections, fonts, shader caches, the whole
/// Wine prefix's mapped files) hold no heap JSON in practice, so they are read
/// only as a fallback. Pass 1 walks the anonymous mappings. Pass 2 walks the
/// file-backed remainder only if pass 1 found nothing. Wine's heap being
/// anonymous is one Wine version's implementation detail, not a guarantee, so
/// the second tier stays rather than rejecting file-backed mappings outright.
/// Worst case, both passes run and read everything.
fn scan_inventory_regions(
    open: impl Fn() -> Option<Box<dyn ProcessHandle>>,
    blob_dir: &Path,
    ts: &str,
    blob_tx: Sender<BlobInventory>,
    save: bool,
) -> Option<usize> {
    const MIN_REGION: usize = 64_000;
    // A monitor tick, not a one-shot command, but the walk still needs a
    // bound. A full walk finishes in low single-digit seconds, so this is never
    // the reason a scan ends.
    const TIMEOUT: u64 = 600;

    let started = Instant::now();
    let deadline = started + Duration::from_secs(TIMEOUT);
    let tier = |backing: RegionBacking| -> Option<Box<dyn RegionSource>> {
        let keep = move |region: &MemoryRegionInfo| {
            !region.is_executable && region.backing == backing
        };
        Some(Box::new(ProcessRegions::new(
            open()?,
            MIN_REGION,
            WALK_CHUNK,
            WALK_CHUNK,
            Some(deadline),
            Some(Box::new(keep)),
        )))
    };

    let mut source = tier(RegionBacking::Anonymous)?;
    let mut saved = stitch_blobs(&mut *source, blob_dir, ts, blob_tx.clone(), save);
    let mut stats = source.stats();

    if saved.is_none() {
        let mut source = tier(RegionBacking::File)?;
        saved = stitch_blobs(&mut *source, blob_dir, ts, blob_tx, save);
        let tier_stats = source.stats();
        stats.regions_skipped += tier_stats.regions_skipped;
        stats.enumerate_ms += tier_stats.enumerate_ms;
        stats.read_ms += tier_stats.read_ms;
    }

    debug!(
        target: "frameforge::blob_capture",
        regions_skipped = stats.regions_skipped,
        enumerate_ms = stats.enumerate_ms,
        read_ms = stats.read_ms,
        total_ms = started.elapsed().as_secs_f64() * 1000.0,
        "scan done"
    );
    saved
}

/// Scans Warframe process memory for the FULL_ACCOUNT inventory blob and sends
/// it through `blob_tx` for the monitor loop to apply.
///
/// When `save=true` also writes the raw text to `blob_dir` for debugging.
/// Returns the number of files written (always 0 when `save=false`).
#[tracing::instrument(level = "debug", skip_all, fields(save = save))]
pub fn capture_all_blobs(
    blob_dir: &Path,
    ts: &str,
    blob_tx: Sender<BlobInventory>,
    save: bool,
) -> usize {
    let Some(pid) = Platform::find_warframe_pid() else {
        warn!(target: "frameforge::blob_capture", "Warframe is not running");
        return 0;
    };
    // No fast path here on purpose. The only caller is the monitor, which
    // reaches this after `probe_tick` already ran that scan and decided the
    // answer was worth a walk. Re-running it returns the same verdict and skips
    // the walk just asked for, so the `Unchanged`-plus-sync escalation never
    // walks at all.
    let open = || match Platform::open_process(pid) {
        Ok(process) => Some(process),
        Err(error) => {
            error!(target: "frameforge::blob_capture", %error, "failed to open Warframe process");
            None
        }
    };
    let saved = scan_inventory_regions(open, blob_dir, ts, blob_tx, save);
    if saved.is_none() {
        warn!(target: "frameforge::blob_capture", "no FULL_ACCOUNT blob found (game in mission, on login screen, or Arsenal not open?)");
    }
    saved.unwrap_or(0)
}

/// One monitor tick: re-read the blob from its remembered address, and check
/// whether the game has logged an inventory sync since the last tick.
///
/// Both answers come from one process handle because the caller always wants
/// both, and opening one means checking ptrace access again.
///
/// The marker is read first and every tick, because it is what tells the blob
/// scan it has something to look at. The scan itself runs only when `force` or
/// that marker says so. Between syncs it can only ever conclude that nothing
/// moved. `None` means it was not scanned this tick, which is not the same as
/// a miss.
///
/// TODO: the blob probe re-reads the maps file for every mapping it stitches
/// through, instead of reusing the snapshot the marker scan already took.
#[tracing::instrument(level = "debug", skip_all, fields(force = force))]
pub fn probe_tick(
    pid: u32,
    blob_tx: Sender<BlobInventory>,
    force: bool,
) -> (Option<ScanOutcome>, bool) {
    let Ok(process) = Platform::open_process(pid) else { return (None, false) };
    let regions: Vec<MemoryRegionInfo> = process.regions_from(0).collect();
    let sync = sync_marker_is_new(linux_newest_sync_timestamp(process.as_ref(), &regions));
    if !(force || sync) {
        return (None, sync);
    }
    // The stitch the probe feeds is capped at `MAX_SCAN` in total, so no
    // single read into it can usefully be larger.
    let source = ProcessRegions::new(process, 0, MAX_SCAN, MAX_SCAN, None, None);
    let source = StitchGuard(Box::new(source));
    (Some(probe_outcome(scan_cached_blob(&source), &blob_tx)), sync)
}

/// Newest sync-marker timestamp currently in the game's log buffers, probing
/// the remembered mapping first and searching for it again when that fails.
fn linux_newest_sync_timestamp(
    process: &dyn ProcessHandle,
    regions: &[MemoryRegionInfo],
) -> Option<f64> {
    let mut buffer = Vec::new();
    let read_region = |region: &MemoryRegionInfo, buffer: &mut Vec<u8>| -> Option<usize> {
        buffer.resize(region.region_size.min(MAX_LOG_REGION), 0);
        let read = process.read_into(region.base_address, buffer);
        (read > LOG_LINE_MARKER.len()).then_some(read)
    };

    let cached = LAST_LOG_REGION.load(Ordering::Relaxed) as usize;
    if cached != 0 {
        if let Some(region) = regions.iter().find(|region| region.base_address == cached) {
            if let Some(read) = read_region(region, &mut buffer) {
                if looks_like_log_buffer(&buffer[..read]) {
                    return newest_sync_timestamp(&buffer[..read]);
                }
            }
        }
        // The mapping is gone or holds something else now. Search again
        // rather than reporting a silent nothing from here on.
        LAST_LOG_REGION.store(0, Ordering::Relaxed);
    }

    if !cold_log_search_due() {
        return None;
    }

    // Cold search. There are two copies of the log text: the pending
    // file-write buffer and a heap ring of recent lines. Which one is
    // further ahead depends on where the game is in its flush cycle, so both
    // are read and the newer marker wins.
    let mut newest: Option<f64> = None;
    let mut found = 0;
    for region in regions {
        if region.is_executable || region.region_size > MAX_LOG_REGION {
            continue;
        }
        let Some(read) = read_region(region, &mut buffer) else { continue };
        let chunk = &buffer[..read];
        if !looks_like_log_buffer(chunk) {
            continue;
        }
        if found == 0 {
            debug!(addr = format_args!("0x{:012x}", region.base_address), kb = read / 1000, "sync-marker buffer");
            LAST_LOG_REGION.store(region.base_address as u64, Ordering::Relaxed);
        }
        if let Some(stamp) = newest_sync_timestamp(chunk) {
            newest = Some(newest.map_or(stamp, |best: f64| best.max(stamp)));
        }
        found += 1;
        if found == 2 {
            break;
        }
    }
    if found == 0 {
        info!("no in-memory log buffer found; sync markers come from the EE.log tail only");
        LOG_SEARCH_BACKOFF.store(LOG_SEARCH_BACKOFF_PROBES, Ordering::Relaxed);
    }
    newest
}

// ==============================================================================
// Diagnostic probes
// ==============================================================================
//
// These each want the same thing: every readable mapping, in bounded pieces,
// with the address the bytes came from. They share one walk so the read caps,
// the deadline, and the procfs error text stay in one place. The inventory scan
// does not join them. It needs a `RegionSource`, which is a different shape.
//
// `visit` returning false stops the walk early — used by the callers that cap
// their output.

/// Hand every mapping `accept` selects to `visit` in chunks, lowest address
/// first. Callers differ in what they want — inventory data, code, or both —
/// so the filter is theirs to supply rather than a flag this has to interpret.
///
/// Takes an already-checked process and an already-discovered region list so
/// a caller that holds both is not forced to repeat the access check and
/// re-read `/proc/pid/maps` just to get at the read loop.
fn walk_regions(
    process: &dyn ProcessHandle,
    regions: impl IntoIterator<Item = MemoryRegionInfo>,
    accept: impl Fn(&MemoryRegionInfo) -> bool,
    deadline: Instant,
    mut visit: impl FnMut(usize, &[u8]) -> bool,
) -> Result<(), String> {
    const MIN_USEFUL: usize = 8;

    let mut buffer = Vec::new();
    for region in regions {
        if !accept(&region) {
            continue;
        }
        let mut offset = 0;
        while offset < region.region_size {
            if Instant::now() >= deadline {
                return Ok(());
            }
            let size = WALK_CHUNK.min(region.region_size - offset);
            let address = region.base_address + offset;
            offset += size;

            buffer.resize(size, 0);
            let read = process.read_into(address, &mut buffer[..size]);
            if read < MIN_USEFUL {
                continue;
            }
            if !visit(address, &buffer[..read]) {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Open the process and discover its regions, then delegate to
/// [`walk_regions`]. The one-shot diagnostic tools below only ever walk once,
/// so they keep this single-call shape rather than plumbing a process handle
/// and a region list through themselves.
fn walk_linux_regions(
    pid: u32,
    accept: impl Fn(&MemoryRegionInfo) -> bool,
    deadline: Instant,
    visit: impl FnMut(usize, &[u8]) -> bool,
) -> Result<(), String> {
    let process = Platform::open_process(pid)?;
    let regions: Vec<MemoryRegionInfo> = process.regions_from(0).collect();
    walk_regions(process.as_ref(), regions, accept, deadline, visit)
}

/// Raw text context around every occurrence of a set of known strings, capped
/// at `max_hits`. Used to reverse-engineer the actual JSON format for inventory
/// items without any parsing assumptions.
#[tracing::instrument(level = "info", skip_all, fields(max_hits = max_hits))]
pub fn dump_inventory_regions(max_hits: usize) -> Vec<String> {
    const NEEDLES: &[&[u8]] = &[
        b"\"MiscItems\":[{",
        b"\"ItemCount\":",
        b"MiscItems",
        b"AlloyPlate",
        b"Circuits\"",
        b"/Lotus/Types/Items/MiscItems/",
    ];
    const HITS_PER_NEEDLE: usize = 3;

    let Some(pid) = Platform::find_warframe_pid() else {
        return vec!["Warframe is not running".to_string()];
    };

    let mut results: Vec<String> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    let walk = walk_linux_regions(pid, |region| !region.is_executable, deadline, |address, data| {
        // Stop the walk as soon as the cap is reached rather than searching
        // every remaining region for a hit we would only discard.
        if results.len() >= max_hits {
            return false;
        }
        for needle in NEEDLES {
            for position in memmem::find_iter(data, needle).take(HITS_PER_NEEDLE) {
                if results.len() >= max_hits {
                    return false;
                }
                let context_start = position.saturating_sub(80);
                let context_end = data.len().min(position + 200);
                let snippet: String = data[context_start..context_end]
                    .iter()
                    .map(|&byte| if (0x20..0x7f).contains(&byte) { byte as char } else { '·' })
                    .collect();
                results.push(format!(
                    "0x{:012x}  needle=\"{}\"  ctx: {}",
                    address + context_start,
                    String::from_utf8_lossy(needle),
                    snippet
                ));
            }
        }
        true
    });

    if let Err(error) = walk {
        return vec![error];
    }
    if results.is_empty() {
        results.push("No matches found".to_string());
    }
    results
}

#[tracing::instrument(level = "info", skip_all)]
pub fn raw_scan_pass(out: &mut impl std::io::Write) -> Result<usize, String> {
    const MIN_LEN: usize = 8;
    const TIMEOUT: u64 = 600; // 10 minutes — full coverage over a full scan

    let pid = Platform::find_warframe_pid().ok_or("Warframe not running")?;
    let deadline = Instant::now() + Duration::from_secs(TIMEOUT);
    let mut count = 0usize;

    // Executable mappings are included: the game's constant string tables live
    // in read-execute sections, and they are half the point of a raw dump.
    walk_linux_regions(pid, |_| true, deadline, |address, data| {
        let mut run_start = None;
        for (index, &byte) in data.iter().enumerate() {
            if (0x20..0x7f).contains(&byte) {
                run_start.get_or_insert(index);
                continue;
            }
            if let Some(start) = run_start.take() {
                if index - start >= MIN_LEN {
                    let text = std::str::from_utf8(&data[start..index]).unwrap_or("?");
                    let _ = writeln!(out, "0x{:012x}  {}", address + start, text);
                    count += 1;
                }
            }
        }
        // A run that reaches the end of the chunk is still worth reporting.
        if let Some(start) = run_start {
            if data.len() - start >= MIN_LEN {
                let text = std::str::from_utf8(&data[start..]).unwrap_or("?");
                let _ = writeln!(out, "0x{:012x}  {}", address + start, text);
                count += 1;
            }
        }
        true
    })?;

    Ok(count)
}

// ==============================================================================
// Riven validity flag
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_scanner::{
        blob_digest_test_guard, reset_last_blob_region, CachedBlobScan, LAST_BLOB_REGION,
    };
    use std::sync::mpsc::Receiver;

    const BLOB_TAIL: &[u8] = br#""DeathSquadable":false}"#;

    /// Serves a fixture mapping list while the reads go to this process's own
    /// memory, so every mapping describes bytes that really are at that
    /// address and an address left out of the list really is unmapped.
    struct FakeProcess {
        real: Box<dyn ProcessHandle>,
        regions: Vec<MemoryRegionInfo>,
    }

    impl FakeProcess {
        fn handle(regions: Vec<MemoryRegionInfo>) -> Box<dyn ProcessHandle> {
            // A real maps file is ascending, and the walk resumes from the end
            // of the last mapping it took, so an out-of-order fixture silently
            // loses mappings instead of failing.
            debug_assert!(
                regions.is_sorted_by_key(|region| region.base_address),
                "fixture mappings must be in ascending address order"
            );
            Box::new(Self {
                real: Platform::open_process(std::process::id())
                    .expect("current process is readable"),
                regions,
            })
        }
    }

    impl ProcessHandle for FakeProcess {
        fn read_into(&self, addr: usize, buf: &mut [u8]) -> usize {
            self.real.read_into(addr, buf)
        }

        fn regions_from(&self, from: usize) -> Box<dyn Iterator<Item = MemoryRegionInfo> + '_> {
            Box::new(
                self.regions
                    .iter()
                    .filter(move |region| region.base_address + region.region_size > from)
                    .cloned(),
            )
        }
    }

    fn mapping(start: usize, len: usize) -> MemoryRegionInfo {
        MemoryRegionInfo {
            base_address: start,
            region_size: len,
            is_committed: true,
            is_readable: true,
            is_writable: true,
            is_executable: false,
            backing: RegionBacking::Anonymous,
        }
    }

    /// A mapping over the test process's own bytes, so the reads are real.
    fn region(data: &[u8]) -> MemoryRegionInfo {
        mapping(data.as_ptr() as usize, data.len())
    }

    /// Opening of a FULL_ACCOUNT blob, padded out to 64 000 bytes. The parser
    /// rejects a blob under 50 KB, and one with no owned Warframe in it, so a
    /// fixture has to carry both before the scan is reached at all.
    fn blob_head(credits: u32) -> Vec<u8> {
        let mut head = format!(
            r#"{{"SubscribedToEmails":true,"RegularCredits":{credits},"MiscItems":[],"XPInfo":[],"FusionPoints":0,"PlayerLevel":0,"RawUpgrades":[],"Suits":[{{"ItemType":"/Lotus/Powersuits/Mag/Mag"}}],"#
        )
        .into_bytes();
        head.resize(64_000, b' ');
        head
    }

    /// A complete blob, trailing zeros standing in for the rest of the mapping.
    fn blob(credits: u32) -> Vec<u8> {
        let mut data = blob_head(credits);
        data.extend_from_slice(BLOB_TAIL);
        data.resize(128_000, 0);
        data
    }

    /// The two-tier walk the monitor runs, minus the process discovery.
    fn walk(regions: Vec<MemoryRegionInfo>) -> (Option<usize>, Receiver<BlobInventory>) {
        let (blob_tx, blob_rx) = std::sync::mpsc::channel();
        let open = || Some(FakeProcess::handle(regions.clone()));
        let saved = scan_inventory_regions(open, &std::env::temp_dir(), "test", blob_tx, false);
        (saved, blob_rx)
    }

    fn probe(regions: Vec<MemoryRegionInfo>) -> Option<CachedBlobScan> {
        let source = ProcessRegions::new(FakeProcess::handle(regions), 0, MAX_SCAN, MAX_SCAN, None, None);
        scan_cached_blob(&StitchGuard(Box::new(source)))
    }




    /// The probe answers without walking memory, so what matters is that each
    /// cached-region result maps onto the outcome the caller's escalation
    /// policy keys off, and that only `Updated` puts an inventory on the
    /// channel.
    #[test]
    fn probe_outcomes_distinguish_fresh_unchanged_and_miss() {
        let _digest_guard = blob_digest_test_guard();
        let data = blob(42);
        let (blob_tx, blob_rx) = std::sync::mpsc::channel();

        reset_last_blob_region();
        LAST_BLOB_REGION.store(data.as_ptr() as u64, Ordering::Relaxed);

        let outcome = probe_outcome(probe(vec![region(&data)]), &blob_tx);
        assert_eq!(outcome, ScanOutcome::Updated);
        assert_eq!(blob_rx.try_recv().expect("a fresh blob is sent on").credits, 42);

        let outcome = probe_outcome(probe(vec![region(&data)]), &blob_tx);
        assert_eq!(outcome, ScanOutcome::Unchanged);
        assert!(blob_rx.try_recv().is_err(), "unchanged bytes must not re-send the inventory");

        // A mission reward delta shares the blob's field names but describes a
        // single mission, so it must read as a miss rather than as inventory.
        let mut delta = br#"{"InventoryChanges":{"MiscItems":[],"SubscribedToEmails":true,"#.to_vec();
        delta.resize(64_000, b' ');
        delta.extend_from_slice(BLOB_TAIL);
        LAST_BLOB_REGION.store(delta.as_ptr() as u64, Ordering::Relaxed);
        let outcome = probe_outcome(probe(vec![region(&delta)]), &blob_tx);
        assert_eq!(outcome, ScanOutcome::CacheMiss);

        LAST_BLOB_REGION.store(data.as_ptr() as u64 + 8, Ordering::Relaxed);
        let outcome = probe_outcome(probe(vec![region(&data)]), &blob_tx);
        assert_eq!(outcome, ScanOutcome::CacheMiss);
        assert!(blob_rx.try_recv().is_err(), "a miss must not send anything");

        reset_last_blob_region();
    }

    #[test]
    fn linux_cached_blob_is_reread_and_rejected_when_stale() {
        let _digest_guard = blob_digest_test_guard();
        let data = blob(42);

        LAST_BLOB_REGION.store(data.as_ptr() as u64, Ordering::Relaxed);
        match probe(vec![region(&data)]).expect("cached blob is re-read") {
            CachedBlobScan::Fresh(_, inventory) => assert_eq!(inventory.credits, 42),
            CachedBlobScan::Unchanged => panic!("first sighting of this blob must parse"),
        }

        // An address that no longer starts a blob must fall back to the walk
        // rather than reporting whatever happens to live there now.
        LAST_BLOB_REGION.store(data.as_ptr() as u64 + 8, Ordering::Relaxed);
        assert!(probe(vec![region(&data)]).is_none());

        reset_last_blob_region();
    }

    /// The warm-path stitch cursor backs off by one marker length on each
    /// iteration. A marker cut in half by a mapping boundary is still seen once
    /// the rest of it arrives in the next read.
    #[test]
    fn linux_cached_blob_finds_end_marker_split_across_mapping_boundary() {
        let _digest_guard = blob_digest_test_guard();
        let (marker_head, marker_tail) = BLOB_TAIL[..b"\"DeathSquadable\":".len()].split_at(7);

        let mut first = blob_head(42);
        first.truncate(64_000 - marker_head.len());
        first.extend_from_slice(marker_head);

        let mut second = marker_tail.to_vec();
        second.extend_from_slice(b"false}");
        second.resize(1024, 0);

        let mut arena = first.clone();
        arena.extend_from_slice(&second);
        let base = arena.as_ptr() as usize;
        let regions = vec![
            mapping(base, first.len()),
            mapping(base + first.len(), second.len()),
        ];
        LAST_BLOB_REGION.store(base as u64, Ordering::Relaxed);
        match probe(regions).expect("split marker is still found") {
            CachedBlobScan::Fresh(_, inventory) => assert_eq!(inventory.credits, 42),
            CachedBlobScan::Unchanged => panic!("first sighting of this blob must parse"),
        }

        reset_last_blob_region();
    }

    #[test]
    fn linux_cached_blob_keeps_stitching_when_end_brace_lands_in_next_mapping() {
        let _digest_guard = blob_digest_test_guard();
        let marker = br#""DeathSquadable":"#;

        let mut first = blob_head(42);
        first.truncate(64_000 - marker.len() - 4);
        first.extend_from_slice(marker);
        first.extend_from_slice(b"fals");

        let mut second = b"e}".to_vec();
        second.resize(1024, 0);

        let mut arena = first.clone();
        arena.extend_from_slice(&second);
        let base = arena.as_ptr() as usize;
        let regions = vec![
            mapping(base, first.len()),
            mapping(base + first.len(), second.len()),
        ];
        LAST_BLOB_REGION.store(base as u64, Ordering::Relaxed);
        match probe(regions).expect("blob completed by the next mapping") {
            CachedBlobScan::Fresh(_, inventory) => assert_eq!(inventory.credits, 42),
            CachedBlobScan::Unchanged => panic!("first sighting of this blob must parse"),
        }

        reset_last_blob_region();
    }

    /// The blob is one contiguous heap allocation. An executable mapping
    /// where its continuation should be means the cached address no longer
    /// holds the blob. Reading past it would splice whatever data mapping
    /// follows onto the truncated head — best case a parse failure, worst
    /// case a stale tail shipped as live inventory.
    #[test]
    fn linux_cached_blob_misses_when_an_executable_mapping_cuts_the_blob() {
        let _digest_guard = blob_digest_test_guard();

        let head = blob_head(42);
        let code = vec![0xCCu8; 4096];
        let mut tail = BLOB_TAIL.to_vec();
        tail.resize(1024, 0);

        let mut arena = head.clone();
        arena.extend_from_slice(&code);
        arena.extend_from_slice(&tail);
        let base = arena.as_ptr() as usize;
        let regions = vec![
            mapping(base, head.len()),
            MemoryRegionInfo {
                is_executable: true,
                ..mapping(base + head.len(), code.len())
            },
            mapping(base + head.len() + code.len(), tail.len()),
        ];
        LAST_BLOB_REGION.store(base as u64, Ordering::Relaxed);
        assert!(
            probe(regions).is_none(),
            "a code mapping cutting the blob must miss, not splice around it"
        );

        reset_last_blob_region();
    }

    /// A freed seed flush against the end of the mapping below it. The end
    /// address is an exclusive bound — nothing is mapped at the seed itself —
    /// so the probe must miss rather than adopt the bytes of whatever mapping
    /// comes next as the seed's.
    #[test]
    fn linux_cached_blob_misses_when_the_seed_sits_at_a_mapping_end() {
        let _digest_guard = blob_digest_test_guard();

        let filler = vec![0u8; 4096];
        let mut arena = filler.clone();
        arena.extend_from_slice(&blob(7));
        let base = arena.as_ptr() as usize;
        // Only the filler is mapped. The blob above it lives in a hole, with
        // the stale seed exactly on the boundary between the two.
        let regions = vec![mapping(base, filler.len())];

        LAST_BLOB_REGION.store((base + filler.len()) as u64, Ordering::Relaxed);
        assert!(
            probe(regions).is_none(),
            "a seed on a mapping's end bound is unmapped and must miss"
        );

        reset_last_blob_region();
    }

    /// The game freed the mapping the seed was recorded in, so the seed now
    /// sits in a hole with a live blob mapped just above it. Reading that blob
    /// and reporting it as the cached one would leave the stale address cached
    /// forever, because a hit never triggers a full walk to correct it.
    #[test]
    fn linux_cached_blob_misses_when_the_seed_address_is_no_longer_mapped() {
        let _digest_guard = blob_digest_test_guard();

        let hole = vec![0u8; 4096];
        let mut arena = hole.clone();
        arena.extend_from_slice(&blob(7));
        let base = arena.as_ptr() as usize;
        let regions = vec![mapping(base + hole.len(), arena.len() - hole.len())];

        LAST_BLOB_REGION.store(base as u64, Ordering::Relaxed);
        assert!(
            probe(regions).is_none(),
            "an unmapped seed must miss, not adopt the next mapping's blob"
        );

        reset_last_blob_region();
    }

    #[test]
    fn linux_cached_blob_skips_reparse_when_bytes_are_unchanged() {
        let _digest_guard = blob_digest_test_guard();
        let data = blob(99);

        reset_last_blob_region();
        LAST_BLOB_REGION.store(data.as_ptr() as u64, Ordering::Relaxed);
        match probe(vec![region(&data)]).expect("first scan parses the blob") {
            CachedBlobScan::Fresh(_, inventory) => assert_eq!(inventory.credits, 99),
            CachedBlobScan::Unchanged => panic!("first sighting of this blob must parse"),
        }

        match probe(vec![region(&data)]).expect("second scan still finds the region") {
            CachedBlobScan::Unchanged => {}
            CachedBlobScan::Fresh(..) => panic!("identical bytes must not be reparsed"),
        }

        reset_last_blob_region();
        LAST_BLOB_REGION.store(data.as_ptr() as u64, Ordering::Relaxed);
        match probe(vec![region(&data)]).expect("scan after reset parses again") {
            CachedBlobScan::Fresh(_, inventory) => assert_eq!(inventory.credits, 99),
            CachedBlobScan::Unchanged => panic!("reset must force a reparse"),
        }

        reset_last_blob_region();
    }

    #[test]
    fn linux_inventory_scan_reports_unchanged_instead_of_reparsing() {
        let _digest_guard = blob_digest_test_guard();
        let arena = blob(7);

        let (found, blobs) = walk(vec![region(&arena)]);
        assert!(found.is_some(), "the first walk has no baseline to match");
        assert_eq!(blobs.try_recv().expect("inventory blob is found").credits, 7);

        let (found, blobs) = walk(vec![region(&arena)]);
        assert!(found.is_some(), "the second walk must still find the blob");
        assert!(blobs.try_recv().is_err(), "identical bytes must not be reparsed");

        reset_last_blob_region();
    }

    /// The safety valve the two-tier walk depends on. A blob living entirely
    /// in a file-backed mapping, the only kind of mapping in this fixture, must
    /// still be found by the tier-2 fallback that runs when pass 1 finds
    /// nothing.
    #[test]
    fn linux_inventory_scan_finds_blob_via_file_backed_fallback() {
        let _digest_guard = blob_digest_test_guard();
        let file_mapping = blob(42);
        let regions = vec![MemoryRegionInfo {
            backing: RegionBacking::File,
            ..region(&file_mapping)
        }];

        let (_, blobs) = walk(regions);

        assert_eq!(
            blobs
                .try_recv()
                .expect("blob in a file-backed mapping is still found via the tier-2 fallback")
                .credits,
            42
        );
        reset_last_blob_region();
    }

    /// The other half of the safety valve: when the anonymous pass already
    /// finds a blob, the file-backed mapping must never be read at all — not
    /// just "not returned", genuinely untouched.
    #[test]
    fn linux_inventory_scan_skips_file_backed_tier_when_anonymous_pass_finds_a_blob() {
        let _digest_guard = blob_digest_test_guard();
        let mut arena = blob(1);
        let anonymous_len = arena.len();
        arena.extend_from_slice(&blob(2));
        let base = arena.as_ptr() as usize;
        let regions = vec![
            mapping(base, anonymous_len),
            MemoryRegionInfo {
                backing: RegionBacking::File,
                ..mapping(base + anonymous_len, arena.len() - anonymous_len)
            },
        ];

        let (_, blobs) = walk(regions);

        assert_eq!(
            blobs.try_iter().map(|inventory| inventory.credits).collect::<Vec<_>>(),
            vec![1],
            "the file-backed blob must not be read once the anonymous pass already found one"
        );

        reset_last_blob_region();
    }

    #[test]
    fn linux_inventory_scan_stitches_and_parses_regions() {
        let _digest_guard = blob_digest_test_guard();
        let mut arena = blob_head(42);
        let head_len = arena.len();
        arena.extend_from_slice(BLOB_TAIL);
        arena.resize(head_len + 64_000, 0);
        let base = arena.as_ptr() as usize;

        let (_, blobs) = walk(vec![mapping(base, head_len), mapping(base + head_len, 64_000)]);

        assert_eq!(blobs.try_recv().expect("inventory blob is found").credits, 42);
        reset_last_blob_region();
    }

    /// The blob is rarely the first thing in the stitched buffer. When a mapping
    /// immediately ahead of it carries a Lotus path it joins the prefix chain,
    /// and seeding at the earliest `{"` in the chain — rather than at the brace
    /// enclosing the blob — produced a document that died on its first value
    /// ("expected value at line 1 column 9") and lost the inventory entirely.
    #[test]
    fn linux_inventory_scan_seeds_at_the_blob_not_at_earlier_json() {
        let _digest_guard = blob_digest_test_guard();
        // Qualifies for the prefix buffer: Lotus path, no start marker, no
        // mission delta — and opens a JSON object of its own.
        let mut prefix = br#"{"Mods":garbage/Lotus/Weapons/Tenno/Rifle "#.to_vec();
        prefix.resize(64_000, b' ');
        let mut blob =
            br#"{"SubscribedToEmails":true,"RegularCredits":42,"MiscItems":[{"ItemType":"/Lotus/Types/Items/x"}],"XPInfo":[],"FusionPoints":0,"PlayerLevel":0,"RawUpgrades":[],"Suits":[{"ItemType":"/Lotus/Powersuits/Mag/Mag"}],"#
                .to_vec();
        blob.resize(64_000, b' ');
        blob.extend_from_slice(BLOB_TAIL);
        blob.resize(128_000, 0);

        // One arena so the two mappings are genuinely contiguous — adjacency is
        // what makes the walk chain them.
        let mut arena = prefix.clone();
        arena.extend_from_slice(&blob);
        let base = arena.as_ptr() as usize;
        let regions = vec![
            mapping(base, prefix.len()),
            mapping(base + prefix.len(), blob.len()),
        ];

        let (_, blobs) = walk(regions);

        assert_eq!(blobs.try_recv().expect("inventory blob is found").credits, 42);
        reset_last_blob_region();
    }

    /// A mission-reward delta carries `/Lotus/` paths and inventory-shaped
    /// keys but no start marker, so the early-exiting qualification must
    /// still reject it rather than mistaking the prefix hit for a real seed.
    #[test]
    fn linux_inventory_scan_rejects_mission_delta_without_start_marker() {
        let _digest_guard = blob_digest_test_guard();

        let mut mission = br#"{"InventoryChanges":{"MiscItems":[{"ItemType":"/Lotus/Types/Items/x"}]}"#.to_vec();
        mission.resize(128_000, b' ');

        let (found, blobs) = walk(vec![region(&mission)]);

        assert!(found.is_none(), "a mission delta with no start marker must never parse as an inventory blob");
        assert!(blobs.try_recv().is_err());
        reset_last_blob_region();
    }

    /// Needs four mappings, not two: an `ActiveScan` cursor that does not latch
    /// on the marker lags one round behind and happens to paper over a
    /// two-mapping gap, so a filler mapping is required to expose the loss.
    #[test]
    fn linux_inventory_scan_completes_when_marker_flush_at_mapping_edge() {
        let _digest_guard = blob_digest_test_guard();

        let marker = b"\"DeathSquadable\":";

        let opening = blob_head(42);

        let mut marker_flush = vec![b' '; 64_000 - marker.len()];
        marker_flush.extend_from_slice(marker);

        let filler = vec![b' '; 64_000];

        let mut closing = b"false}".to_vec();
        closing.resize(64_000, 0);

        let mut arena = opening.clone();
        arena.extend_from_slice(&marker_flush);
        arena.extend_from_slice(&filler);
        arena.extend_from_slice(&closing);
        let base = arena.as_ptr() as usize;
        let regions = vec![
            mapping(base, opening.len()),
            mapping(base + opening.len(), marker_flush.len()),
            mapping(base + opening.len() + marker_flush.len(), filler.len()),
            mapping(base + opening.len() + marker_flush.len() + filler.len(), closing.len()),
        ];

        let (_, blobs) = walk(regions);

        assert_eq!(blobs.try_recv().expect("blob completed once brace lands").credits, 42);
        reset_last_blob_region();
    }

    /// The scan budget is spent on what a mapping can still contribute rather
    /// than on the mapping whole. A blob that closes inside an oversized
    /// mapping still parses instead of being dropped with it.
    #[test]
    fn linux_inventory_scan_finishes_before_rejecting_large_mapping() {
        let _digest_guard = blob_digest_test_guard();
        let mut arena = blob_head(42);
        let head_len = arena.len();
        arena.extend_from_slice(BLOB_TAIL);
        arena.resize(head_len + 64 * 1024 * 1024, 0);
        let base = arena.as_ptr() as usize;

        let (_, blobs) =
            walk(vec![mapping(base, head_len), mapping(base + head_len, 64 * 1024 * 1024)]);

        assert_eq!(blobs.try_recv().expect("inventory blob is found").credits, 42);
        reset_last_blob_region();
    }

    #[test]
    fn linux_inventory_scan_finds_blob_past_first_chunk_boundary() {
        let _digest_guard = blob_digest_test_guard();

        let blob_bytes = blob(42);
        let blob_offset = WALK_CHUNK + 1024;
        let mut arena = vec![0u8; blob_offset + blob_bytes.len()];
        arena[blob_offset..blob_offset + blob_bytes.len()].copy_from_slice(&blob_bytes);

        let (_, blobs) = walk(vec![region(&arena)]);

        assert_eq!(
            blobs.try_recv().expect("blob past the first 64 MiB chunk must still be found").credits,
            42
        );
        reset_last_blob_region();
    }

    /// The harder cousin of the test above: the blob physically spans the
    /// 64 MiB chunk seam. The start marker sits in the first chunk (opening an
    /// `ActiveScan`) while the end marker and closing brace land in the second,
    /// so the seed opened in chunk A must be stitched to chunk B to parse.
    #[test]
    fn linux_inventory_scan_finds_blob_straddling_chunk_boundary() {
        let _digest_guard = blob_digest_test_guard();

        let blob_bytes = blob(42);
        // Start marker a few KiB before the seam, end marker (~offset 64 000)
        // well past it, so the blob body crosses the A/B boundary.
        let blob_offset = WALK_CHUNK - 8192;
        let mut arena = vec![0u8; blob_offset + blob_bytes.len()];
        arena[blob_offset..blob_offset + blob_bytes.len()].copy_from_slice(&blob_bytes);

        let (_, blobs) = walk(vec![region(&arena)]);

        assert_eq!(
            blobs.try_recv().expect("blob straddling the 64 MiB seam must be stitched and found").credits,
            42
        );
        reset_last_blob_region();
    }

    #[test]
    fn linux_inventory_scan_recovers_fields_before_start_marker() {
        let _digest_guard = blob_digest_test_guard();
        let prefix =
            br#"{"RegularCredits":42,"MiscItems":[{"ItemType":"/Lotus/Test","ItemCount":1}],"XPInfo":[],"FusionPoints":0,"PlayerLevel":0,"RawUpgrades":[],"Suits":[{"ItemType":"/Lotus/Powersuits/Mag/Mag"}],"#;
        let suffix = br#""SubscribedToEmails":true,"DeathSquadable":false}"#;
        let mut arena = vec![b' '; 128_000];
        arena[..prefix.len()].copy_from_slice(prefix);
        arena[64_000..64_000 + suffix.len()].copy_from_slice(suffix);
        let base = arena.as_ptr() as usize;
        let regions = vec![
            mapping(base, 64_000),
            mapping(base + 64_000, 64_000),
        ];

        let (_, blobs) = walk(regions);

        let inventory = blobs.try_recv().expect("inventory blob is found");
        assert_eq!(inventory.credits, 42);
        assert_eq!(inventory.stackable_items.len(), 1);
        reset_last_blob_region();
    }
}
