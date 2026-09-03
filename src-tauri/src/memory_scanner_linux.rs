//! Reading Warframe's memory under Proton.
//!
//! The blob, the markers and the stitch engine live in `memory_scanner`;
//! this module only reads another process: `/proc/pid/maps` for the
//! mappings, `process_vm_readv` for the bytes, and a [`RegionSource`] that
//! passes both to the engine.

use memchr::memmem;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use crate::mem_regions::RegionSource;
use crate::memory_scanner::{
    cold_log_search_due, looks_like_log_buffer, newest_sync_timestamp, probe_outcome,
    scan_auth_credentials, scan_cached_blob, scan_steam_id, stitch_blobs, sync_marker_is_new,
    BlobInventory, ScanOutcome, LAST_LOG_REGION, LOG_LINE_MARKER,
    LOG_SEARCH_BACKOFF, LOG_SEARCH_BACKOFF_PROBES, MAX_LOG_REGION, MAX_SCAN,
};

// ==============================================================================
// Process and mappings
// ==============================================================================

#[derive(Debug, PartialEq, Eq, Default)]
struct LinuxRegion {
    start: usize,
    len: usize,
    executable: bool,
    // `/proc/pid/maps`'s 6th field: absent for anonymous mappings, a real
    // path for file-backed ones, or a kernel pseudo-path like `[heap]`,
    // `[stack]`, `[vvar]`, `[vsyscall]`.
    path: Option<Box<str>>,
}

impl LinuxRegion {
    /// `[heap]` and `[stack]` are anonymous in spirit — kernel-labeled
    /// untagged memory, not a mapped file — and a 105 MB `[heap]` mapping is
    /// exactly the shape a multi-megabyte JSON blob lives in, so both stay
    /// first-pass candidates alongside true anonymous mappings. Other
    /// bracketed pseudo-paths (`[vvar]`, `[vsyscall]`, ...) are kernel data
    /// pages that can never hold heap JSON, so they are excluded from both
    /// passes rather than falling through to the file-backed tier.
    fn is_anonymous(&self) -> bool {
        matches!(self.path.as_deref(), None | Some("[heap]") | Some("[stack]"))
    }

    fn is_file_backed(&self) -> bool {
        matches!(self.path.as_deref(), Some(path) if !path.starts_with('['))
    }
}

struct LinuxProcess {
    pid: u32,
    // Fallback for the process_vm_readv EFAULT case in `read` below. Not used
    // on the fast path.
    memory: File,
}

impl LinuxProcess {
    fn open(pid: u32) -> Result<Self, String> {
        open_linux_process_memory(pid).map(|memory| Self { pid, memory })
    }

    fn read(&self, address: usize, buffer: &mut [u8]) -> std::io::Result<usize> {
        read_linux_process_memory(self.pid, &self.memory, address, buffer)
    }
}

fn parse_linux_maps(maps: &str) -> Vec<LinuxRegion> {
    maps.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (start, end) = fields.next()?.split_once('-')?;
            let permissions = fields.next()?;
            if !permissions.starts_with('r') {
                return None;
            }
            let start = usize::from_str_radix(start, 16).ok()?;
            let end = usize::from_str_radix(end, 16).ok()?;
            // nth(3) skips offset, dev, inode to land on the pathname, which
            // unlike every earlier field may contain spaces, so it is taken as
            // the rest of the line rather than as a single token. The kernel's
            // " (deleted)" suffix — how a Wine prefix updated under a running
            // game shows up — is not part of the path and would otherwise
            // defeat the `warframe.x64.exe` match in `linux_game_image_span`.
            let path = fields.nth(3).map(|name| {
                let offset = name.as_ptr() as usize - line.as_ptr() as usize;
                let path = line[offset..].trim_end();
                Box::from(path.strip_suffix(" (deleted)").unwrap_or(path))
            });
            Some(LinuxRegion {
                start,
                len: end.checked_sub(start)?,
                executable: permissions.as_bytes().get(2) == Some(&b'x'),
                path,
            })
        })
        .collect()
}

fn linux_process_regions(pid: u32) -> Result<Vec<LinuxRegion>, String> {
    let path = format!("/proc/{pid}/maps");
    std::fs::read_to_string(&path)
        .map(|maps| parse_linux_maps(&maps))
        .map_err(|error| {
            format!(
                "Failed to read {path}: {error}. Ensure kernel.yama.ptrace_scope permits same-user process access"
            )
        })
}

fn open_linux_process_memory(pid: u32) -> Result<File, String> {
    let path = format!("/proc/{pid}/mem");
    File::open(&path).map_err(|error| {
        format!(
            "Failed to open {path}: {error}. Ensure kernel.yama.ptrace_scope permits same-user process access"
        )
    })
}

/// `/proc/pid/mem` copies every byte through a kernel scratch buffer.
/// `process_vm_readv` pins the remote pages and copies straight into `buffer`
/// (measured 3.55s vs 0.44s reading the same 754 regions). Same privilege
/// check as the procfs path (`PTRACE_MODE_ATTACH_REALCREDS`), so the
/// ptrace_scope guidance above stays accurate.
fn read_linux_process_memory(
    pid: u32,
    memory: &File,
    address: usize,
    buffer: &mut [u8],
) -> std::io::Result<usize> {
    let local_iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    let remote_iov = libc::iovec {
        iov_base: address as *mut std::ffi::c_void,
        iov_len: buffer.len(),
    };

    // SAFETY: local_iov/iov_base points at `buffer`, which the caller keeps
    // alive and exclusively borrowed for `buffer.len()` bytes across this
    // call. remote_iov's base is only ever dereferenced by the kernel inside
    // the target process, never in this address space. The return value is
    // checked before `buffer` is trusted.
    let written = unsafe { libc::process_vm_readv(pid as libc::pid_t, &local_iov, 1, &remote_iov, 1, 0) };
    if written >= 0 {
        return Ok(written as usize);
    }

    let error = std::io::Error::last_os_error();
    // A hole partway through the range comes back as a short read (handled
    // above), same as `read_at`. EFAULT means not even the first page was
    // readable, which is the one case where the two readers walk the mapping
    // differently enough to be worth double-checking, so retry through procfs
    // and let its answer stand — it reports the same case as EIO, and callers
    // were written against that. Any other errno (ESRCH once the process has
    // exited) propagates, and every caller already skips the region on Err.
    if error.raw_os_error() == Some(libc::EFAULT) {
        return memory.read_at(buffer, address as u64);
    }
    Err(error)
}

fn is_warframe_command(command: &str) -> bool {
    let command = command.to_ascii_lowercase();
    command.contains("warframe.x64.exe")
        && !command.contains("launcher.exe")
        && !command.contains("warframe-companion")
}

/// The game's PID, or `None` while it is not running.
pub fn find_warframe_pid() -> Option<u32> {
    std::fs::read_dir("/proc")
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            let pid = entry.file_name().to_str()?.parse().ok()?;
            let command = std::fs::read(entry.path().join("cmdline")).ok()?;
            is_warframe_command(&String::from_utf8_lossy(&command)).then_some(pid)
        })
}

// ==============================================================================
// Region source for the stitch engine
// ==============================================================================

/// Mappings reach the engine in pieces this size, so a multi-gigabyte arena
/// cannot exhaust the heap.
const WALK_CHUNK: usize = 64 * 1024 * 1024;

/// Feeds the shared stitch engine from a list of mappings.
///
/// Which mappings to offer is the caller's decision: the cold walk passes one
/// tier at a time, the probe passes everything. What this adds is the read
/// policy, a per-read cap and, for the walk, a deadline.
///
/// The mapping list is a snapshot taken when the caller parsed /proc/maps,
/// not a live query per read. Callers build a fresh source per tick, so it can
/// only age by the milliseconds one probe or walk runs. A mapping freed inside
/// that window fails at the read, which ends the stitch the same as a mapping
/// missing from the list.
struct LinuxRegionSource<'a> {
    process: &'a LinuxProcess,
    regions: Vec<LinuxRegion>,
    /// Mapping `next_region` resumes at, and how far into it the walk has read.
    next: usize,
    offset: usize,
    read_cap: usize,
    /// Only the walk is bounded. The probe reads a handful of mappings and
    /// ends long before any deadline would matter.
    deadline: Option<Instant>,
    /// Reused across `next_region` calls. The walk copies what it keeps, so
    /// the alternative is allocating and zeroing up to `WALK_CHUNK` (64 MiB)
    /// per chunk — gigabytes of pure memset over a full walk.
    buffer: Vec<u8>,
    bytes_read: u64,
    read_time: Duration,
}

impl<'a> LinuxRegionSource<'a> {
    /// Fewer than eight bytes cannot hold any marker, so a read that short is
    /// treated as a failed one.
    const MIN_USEFUL: usize = 8;

    fn walking(process: &'a LinuxProcess, regions: Vec<LinuxRegion>, deadline: Instant) -> Self {
        Self::new(process, regions, WALK_CHUNK, Some(deadline))
    }

    /// The stitch the probe feeds is capped at `MAX_SCAN` in total, so no
    /// single read into it can usefully be larger.
    fn probing(process: &'a LinuxProcess, regions: Vec<LinuxRegion>) -> Self {
        Self::new(process, regions, MAX_SCAN, None)
    }

    fn new(
        process: &'a LinuxProcess,
        regions: Vec<LinuxRegion>,
        read_cap: usize,
        deadline: Option<Instant>,
    ) -> Self {
        Self {
            process,
            regions,
            next: 0,
            offset: 0,
            read_cap,
            deadline,
            buffer: Vec::new(),
            bytes_read: 0,
            read_time: Duration::ZERO,
        }
    }

    /// `(bytes_read, read_ms)` accumulated so far.
    fn stats(&self) -> (u64, f64) {
        (self.bytes_read, self.read_time.as_secs_f64() * 1000.0)
    }
}

impl RegionSource for LinuxRegionSource<'_> {
    fn next_region(&mut self) -> Option<(usize, &[u8])> {
        loop {
            let (start, len) = {
                let region = self.regions.get(self.next)?;
                (region.start, region.len)
            };
            if self.offset >= len {
                self.next += 1;
                self.offset = 0;
                continue;
            }
            if self.deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return None;
            }

            let size = self.read_cap.min(len - self.offset);
            let address = start + self.offset;
            self.offset += size;

            let started = Instant::now();
            // Grow-only, so zeroing happens once per high-water mark instead
            // of once per chunk. The read overwrites stale bytes up to
            // `read`, and only `..read` is handed out.
            if self.buffer.len() < size {
                self.buffer.resize(size, 0);
            }
            let read = self.process.read(address, &mut self.buffer[..size]);
            self.read_time += started.elapsed();
            match read {
                Ok(read) if read >= Self::MIN_USEFUL => {
                    self.bytes_read += read as u64;
                    return Some((address, &self.buffer[..read]));
                }
                Ok(_) | Err(_) => continue,
            }
        }
    }

    fn read_at(&self, addr: usize, max_len: usize) -> Option<(usize, Vec<u8>)> {
        // The blob is contiguous, so `addr` must itself be mapped. A seed in
        // a hole is a stale seed, and a stitch that reaches a hole has
        // reached the blob's end. Skipping forward would return a later
        // mapping's bytes as if they lived at `addr` and splice unrelated
        // memory into the blob. That includes a seed flush against a
        // mapping's end — an exclusive bound, so not mapped either.
        let region = self
            .regions
            .iter()
            .find(|region| (region.start..region.start + region.len).contains(&addr))?;
        let end = region.start + region.len;
        // Executable mappings hold code. File-backed ones hold mapped
        // PE/data files whose string constants false-trigger the anchor
        // checks. Empty bytes end the stitch. A blob the tier-2 walk found in
        // a file-backed mapping loses the fast path this way and re-walks per
        // sync.
        if region.executable || region.is_file_backed() {
            return Some((end, Vec::new()));
        }
        let mut buffer = vec![0u8; (end - addr).min(self.read_cap).min(max_len)];
        let read = self.process.read(addr, &mut buffer).ok()?;
        if read == 0 {
            return Some((end, Vec::new()));
        }
        buffer.truncate(read);
        // A read cut short — by `max_len`, the cap, or a faulted page inside
        // the mapping — resumes at its own end rather than the region end.
        // The next call re-enters this mapping there, so a fault ends the
        // stitch on its next read instead of silently skipping the hole.
        Some((addr + read, buffer))
    }
}

// ==============================================================================
// Inventory blob capture
// ==============================================================================

/// Walk `regions` and stitch, parse and send every FULL_ACCOUNT blob found.
///
/// File-backed mappings (PE data sections, fonts, shader caches, the whole
/// Wine prefix's mapped files) hold no heap JSON in practice, so they are read
/// only as a fallback. Pass 1 walks the anonymous mappings. Pass 2 walks the
/// file-backed remainder only if pass 1 found nothing. Wine's heap being
/// anonymous is one Wine version's implementation detail, not a guarantee, so
/// the second tier stays rather than rejecting file-backed mappings outright.
/// Worst case, both passes run and read everything.
fn scan_inventory_regions(
    process: &LinuxProcess,
    regions: Vec<LinuxRegion>,
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

    let (anonymous, file_backed): (Vec<LinuxRegion>, Vec<LinuxRegion>) = regions
        .into_iter()
        .filter(|region| {
            !region.executable
                && region.len >= MIN_REGION
                && (region.is_anonymous() || region.is_file_backed())
        })
        .partition(LinuxRegion::is_anonymous);

    let started = Instant::now();
    let deadline = started + Duration::from_secs(TIMEOUT);

    let mut source = LinuxRegionSource::walking(process, anonymous, deadline);
    let mut saved = stitch_blobs(&mut source, blob_dir, ts, blob_tx.clone(), save);
    let (mut bytes_read, mut read_ms) = source.stats();

    if saved.is_none() {
        let mut source = LinuxRegionSource::walking(process, file_backed, deadline);
        saved = stitch_blobs(&mut source, blob_dir, ts, blob_tx, save);
        let (tier_bytes, tier_ms) = source.stats();
        bytes_read += tier_bytes;
        read_ms += tier_ms;
    }

    debug!(
        target: "frameforge::blob_capture",
        bytes_mb = bytes_read / 1_000_000,
        read_ms,
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
    let Some(pid) = find_warframe_pid() else {
        warn!(target: "frameforge::blob_capture", "Warframe is not running");
        return 0;
    };
    let process = match LinuxProcess::open(pid) {
        Ok(process) => process,
        Err(error) => {
            error!(target: "frameforge::blob_capture", %error, "failed to open Warframe process");
            return 0;
        }
    };
    let regions = match linux_process_regions(pid) {
        Ok(regions) => regions,
        Err(error) => {
            error!(target: "frameforge::blob_capture", %error, "failed to enumerate Warframe process regions");
            return 0;
        }
    };

    // No fast path here on purpose. The only caller is the monitor, which
    // reaches this after `probe_tick` already ran that scan and decided the
    // answer was worth a walk. Re-running it returns the same verdict and skips
    // the walk just asked for, so the `Unchanged`-plus-sync escalation never
    // walks at all.
    let saved = scan_inventory_regions(&process, regions, blob_dir, ts, blob_tx, save);
    if saved.is_none() {
        warn!(target: "frameforge::blob_capture", "no FULL_ACCOUNT blob found (open Arsenal or Inventory and try again)");
    }
    saved.unwrap_or(0)
}

/// One monitor tick: re-read the blob from its remembered address, and check
/// whether the game has logged an inventory sync since the last tick.
///
/// Both answers come from one process handle and one region list because the
/// caller always wants both, and acquiring them means re-parsing a maps file
/// with thousands of entries.
///
/// The marker is read first and every tick, because it is what tells the blob
/// scan it has something to look at. The scan itself runs only when `force` or
/// that marker says so. Between syncs it can only ever conclude that nothing
/// moved. `None` means it was not scanned this tick, which is not the same as
/// a miss.
#[tracing::instrument(level = "debug", skip_all, fields(force = force))]
pub fn probe_tick(
    pid: u32,
    blob_tx: Sender<BlobInventory>,
    force: bool,
) -> (Option<ScanOutcome>, bool) {
    let Ok(process) = LinuxProcess::open(pid) else { return (None, false) };
    let Ok(regions) = linux_process_regions(pid) else { return (None, false) };
    let sync = sync_marker_is_new(linux_newest_sync_timestamp(&process, &regions));
    if !(force || sync) {
        return (None, sync);
    }
    let source = LinuxRegionSource::probing(&process, regions);
    (Some(probe_outcome(scan_cached_blob(&source), &blob_tx)), sync)
}

/// Newest sync-marker timestamp currently in the game's log buffers, probing
/// the remembered mapping first and searching for it again when that fails.
fn linux_newest_sync_timestamp(process: &LinuxProcess, regions: &[LinuxRegion]) -> Option<f64> {
    let mut buffer = Vec::new();
    let read_region = |region: &LinuxRegion, buffer: &mut Vec<u8>| -> Option<usize> {
        buffer.resize(region.len.min(MAX_LOG_REGION), 0);
        match process.read(region.start, buffer) {
            Ok(read) if read > LOG_LINE_MARKER.len() => Some(read),
            Ok(_) | Err(_) => None,
        }
    };

    let cached = LAST_LOG_REGION.load(Ordering::Relaxed) as usize;
    if cached != 0 {
        if let Some(region) = regions.iter().find(|region| region.start == cached) {
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
        if region.executable || region.len > MAX_LOG_REGION {
            continue;
        }
        let Some(read) = read_region(region, &mut buffer) else { continue };
        let chunk = &buffer[..read];
        if !looks_like_log_buffer(chunk) {
            continue;
        }
        if found == 0 {
            debug!(addr = format_args!("0x{:012x}", region.start), kb = read / 1000, "sync-marker buffer");
            LAST_LOG_REGION.store(region.start as u64, Ordering::Relaxed);
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
// Credentials
// ==============================================================================

#[tracing::instrument(level = "info", skip_all)]
pub fn scan_warframe_credentials_process() -> Result<(String, String, String), String> {
    let pid = find_warframe_pid().ok_or("Warframe is not running")?;
    let process = LinuxProcess::open(pid)?;
    let regions = linux_process_regions(pid)?;

    scan_linux_credential_regions(&process, regions).ok_or_else(|| {
        "Credentials not found in memory. Make sure you are in the orbiter (not loading screen) \
         and Warframe has been running for a few minutes."
            .into()
    })
}

/// Walk readable data mappings looking for the login response the game keeps in
/// memory. Split from the command above so it can be exercised against the test
/// process, where the mappings and the expected bytes are both known.
fn scan_linux_credential_regions(
    process: &LinuxProcess,
    regions: impl IntoIterator<Item = LinuxRegion>,
) -> Option<(String, String, String)> {
    // Per-region ceiling: anything larger is a texture or heap arena, not the
    // small JSON blob we are after, and reading it would cost hundreds of
    // megabytes of copies per scan.
    const MAX_REGION: usize = 128 * 1024 * 1024;

    // Deliberately not routed through `walk_regions`: that chunks at 64 MiB,
    // which would split the 64–128 MiB mappings this accepts into two reads and
    // hand `scan_auth_credentials` and `scan_steam_id` a boundary they could
    // lose a match across. Reading each accepted region whole is the point here.
    let mut buffer = Vec::new();
    for region in regions {
        if region.executable || region.len > MAX_REGION {
            continue;
        }
        buffer.resize(region.len, 0);
        let read = match process.read(region.start, &mut buffer) {
            Ok(read) if read > 0 => read,
            Ok(_) | Err(_) => continue,
        };
        let data = &buffer[..read];
        if let Some((id, nonce)) = scan_auth_credentials(data) {
            return Some((id, nonce, scan_steam_id(data).unwrap_or_default()));
        }
    }
    None
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
/// Takes an already-open process and an already-discovered region list so a
/// caller that already holds both is not forced to reopen `/proc/pid/mem` and
/// re-read `/proc/pid/maps` just to get at the read loop.
fn walk_regions(
    process: &LinuxProcess,
    regions: impl IntoIterator<Item = LinuxRegion>,
    accept: impl Fn(&LinuxRegion) -> bool,
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
        while offset < region.len {
            if Instant::now() >= deadline {
                return Ok(());
            }
            let size = WALK_CHUNK.min(region.len - offset);
            let address = region.start + offset;
            offset += size;

            buffer.resize(size, 0);
            let read = match process.read(address, &mut buffer[..size]) {
                Ok(read) if read >= MIN_USEFUL => read,
                Ok(_) | Err(_) => continue,
            };
            if !visit(address, &buffer[..read]) {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Open the process and discover its regions, then delegate to
/// [`walk_regions`]. The one-shot diagnostic tools below only ever walk once,
/// so they keep this single-call shape rather than plumbing a `LinuxProcess`
/// and `Vec<LinuxRegion>` through themselves.
fn walk_linux_regions(
    pid: u32,
    accept: impl Fn(&LinuxRegion) -> bool,
    deadline: Instant,
    visit: impl FnMut(usize, &[u8]) -> bool,
) -> Result<(), String> {
    let process = LinuxProcess::open(pid)?;
    let regions = linux_process_regions(pid)?;
    walk_regions(&process, regions, accept, deadline, visit)
}

/// Read a single byte from the game process, used for the riven validity flag.
/// `None` means the process or the address is not readable.
pub fn read_process_byte(pid: u32, address: usize) -> Option<u8> {
    let process = LinuxProcess::open(pid).ok()?;
    let mut byte = [0u8; 1];
    match process.read(address, &mut byte) {
        Ok(1) => Some(byte[0]),
        _ => None,
    }
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

    let Some(pid) = find_warframe_pid() else {
        return vec!["Warframe is not running".to_string()];
    };

    let mut results: Vec<String> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    let walk = walk_linux_regions(pid, |region| !region.executable, deadline, |address, data| {
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

/// Collect context around the request strings the game builds its API calls
/// from. Shares the region walk with the other probes.
pub fn scan_api_url_strings() -> Result<Vec<String>, String> {
    const NEEDLES: &[&[u8]] = &[
        b"/API/PHP/",
        b"inventory.php",
        b"login.php",
        b"warframe.com/A",
        b"Nonce",
        b"accountId",
    ];
    const MAX_RESULTS: usize = 40;

    let pid = find_warframe_pid().ok_or("Warframe not running")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut found: Vec<String> = Vec::new();

    walk_linux_regions(pid, |region| !region.executable, deadline, |_, data| {
        for needle in NEEDLES {
            for position in memmem::find_iter(data, needle) {
                let start = position.saturating_sub(30);
                let end = (position + 100).min(data.len());
                let context: String = data[start..end]
                    .iter()
                    .map(|&byte| if (0x20..0x7f).contains(&byte) { byte as char } else { ' ' })
                    .collect();
                let trimmed = context.split_whitespace().collect::<Vec<_>>().join(" ");
                // Near-identical strings appear in thousands of copies, so the
                // first 30 characters act as the deduplication key.
                let key = &trimmed[..trimmed.len().min(30)];
                if !found.iter().any(|seen| seen.contains(key)) {
                    found.push(format!("[{}] {}", String::from_utf8_lossy(needle), trimmed));
                }
                if found.len() >= MAX_RESULTS {
                    return false;
                }
            }
        }
        true
    })?;

    Ok(found)
}

#[tracing::instrument(level = "info", skip_all)]
pub fn raw_scan_pass(out: &mut impl std::io::Write) -> Result<usize, String> {
    const MIN_LEN: usize = 8;
    const TIMEOUT: u64 = 600; // 10 minutes — full coverage over a full scan

    let pid = find_warframe_pid().ok_or("Warframe not running")?;
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

/// Address span of the game's own module.
///
/// Wine does not leave the executable's code file-backed — only the PE headers
/// and one data section keep the pathname, while `.text` becomes a large
/// anonymous executable mapping wedged between them. So the module is
/// identified by the span its named mappings bracket, not by file backing.
fn linux_game_image_span(regions: &[LinuxRegion]) -> Option<std::ops::Range<usize>> {
    regions
        .iter()
        .filter(|region| {
            region
                .path
                .as_deref()
                .is_some_and(|path| path.to_ascii_lowercase().ends_with("warframe.x64.exe"))
        })
        .map(|region| region.start..region.start + region.len)
        .reduce(|span, next| span.start.min(next.start)..span.end.max(next.end))
}

/// Locate the byte the game sets while a riven reroll's A/B selection screen is
/// up. Non-zero = selection pending, which is the same event the EE.log
/// `omegarerollselection.swf` line reports.
///
/// This is not the Pattern D-2 scan. That pattern matches exactly one
/// site under Proton, and the byte it resolves to reads 1 in the orbiter, in
/// the mod segment and on the reroll screen alike, so it cannot drive the
/// watcher. The signature below was found by diffing the game's writable
/// statics across those states against the live game, and the surviving byte
/// was confirmed to be 0 everywhere except while the A/B screen is up.
///
/// It matches the store the game publishes the state with:
///
/// ```text
/// 44 87 35 <disp32>    xchg dword ptr [rip+disp], r14d
/// 41 83 fe 01          cmp  r14d, 1
/// ```
///
/// Only the displacement varies, and it appeared exactly once in the whole
/// process, so the first match is taken.
///
/// ponytail: a byte signature tracks one game build. If riven detection stops
/// working after an update, re-run `examples/riven_flag_hunt.rs` to find the
/// flag again rather than guessing at the pattern.
pub fn find_riven_validity_va(pid: u32) -> Option<usize> {
    const STORE: [u8; 3] = [0x44, 0x87, 0x35];
    const COMPARE: [u8; 4] = [0x41, 0x83, 0xfe, 0x01];
    // Bytes from the start of the store up to the end of its displacement,
    // which is where the RIP-relative address is measured from.
    const STORE_LEN: usize = 7;

    let regions = linux_process_regions(pid).ok()?;
    let span = linux_game_image_span(&regions)?;
    let process = LinuxProcess::open(pid).ok()?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut result = None;

    // The game's code is one mapping well under the walk's chunk size, so a
    // signature cannot be split across two chunks here.
    let walk = walk_regions(
        &process,
        regions,
        |region| region.executable && span.contains(&region.start),
        deadline,
        |address, data| {
            // STORE has no self-border (no proper suffix of it is also a
            // prefix), so a match can never start inside a previous match's
            // span. find_iter's non-overlapping search cannot skip a real hit.
            let Some(limit) = data.len().checked_sub(STORE_LEN + COMPARE.len()) else {
                return true;
            };
            // `limit` is the last index where the full signature still fits,
            // so it is itself in bounds.
            for index in memmem::find_iter(data, &STORE) {
                if index > limit {
                    break;
                }
                if data[index + STORE_LEN..index + STORE_LEN + COMPARE.len()] != COMPARE {
                    continue;
                }
                let displacement = i32::from_le_bytes(
                    data[index + STORE.len()..index + STORE_LEN]
                        .try_into()
                        .expect("displacement is four bytes"),
                ) as i64;
                let flag = (address + index + STORE_LEN) as i64 + displacement;
                if flag > 0x10000 && flag < 0x7fff_ffff_ffff {
                    result = Some(flag as usize);
                    return false;
                }
            }
            true
        },
    );
    walk.ok()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_scanner::{
        blob_digest_test_guard, reset_last_blob_region, CachedBlobScan, LAST_BLOB_REGION,
    };
    use std::sync::mpsc::Receiver;

    const BLOB_TAIL: &[u8] = br#""DeathSquadable":false}"#;

    fn this_process() -> LinuxProcess {
        LinuxProcess::open(std::process::id()).expect("current process is readable")
    }

    /// A readable data mapping over the test process's own bytes.
    fn region(data: &[u8]) -> LinuxRegion {
        LinuxRegion {
            start: data.as_ptr() as usize,
            len: data.len(),
            executable: false,
            ..Default::default()
        }
    }

    /// Opening of a FULL_ACCOUNT blob, padded out to 64 000 bytes. The parser
    /// rejects a blob under 50 KB, and one with no owned Warframe in it, so a
    /// fixture has to carry both before the scan is reached at all.
    fn blob_head(credits: u32) -> Vec<u8> {
        let mut head = format!(
            r#"{{"SubscribedToEmails":true,"RegularCredits":{credits},"MiscItems":[],"XPInfo":[],"FusionPoints":0,"Suits":[{{"ItemType":"/Lotus/Powersuits/Mag/Mag"}}],"#
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
    fn walk(
        process: &LinuxProcess,
        regions: Vec<LinuxRegion>,
    ) -> (Option<usize>, Receiver<BlobInventory>) {
        let (blob_tx, blob_rx) = std::sync::mpsc::channel();
        let saved =
            scan_inventory_regions(process, regions, &std::env::temp_dir(), "test", blob_tx, false);
        (saved, blob_rx)
    }

    fn probe(process: &LinuxProcess, regions: Vec<LinuxRegion>) -> Option<CachedBlobScan> {
        scan_cached_blob(&LinuxRegionSource::probing(process, regions))
    }

    #[test]
    fn linux_reader_reads_its_own_mapping() {
        let marker = b"frameforge-linux-reader";
        let process = this_process();
        let mut actual = vec![0; marker.len()];
        let read = process
            .read(marker.as_ptr() as usize, &mut actual)
            .expect("marker address is mapped");
        assert_eq!(read, marker.len());
        assert_eq!(actual, marker);
    }

    #[test]
    fn linux_reader_skips_rather_than_panics_when_the_first_page_is_unmapped() {
        let process = this_process();
        let mut buffer = vec![0u8; 4096];
        // Below mmap_min_addr on every normal Linux config, so nothing is ever
        // mapped here. process_vm_readv reports this as EFAULT, which read()
        // retries through /proc/pid/mem. Either shape must reach the caller
        // as a skip rather than a panic.
        match process.read(0x1000, &mut buffer) {
            Ok(read) => assert_eq!(read, 0, "no bytes can come from an unmapped page"),
            Err(_) => {}
        }
    }

    #[test]
    fn linux_reader_returns_leading_bytes_when_the_read_crosses_into_a_hole() {
        use std::ffi::c_void;

        let page = 4096;
        // Two adjacent anonymous pages, then revoke access to the second: the
        // read below spans a readable page followed by an unreadable one,
        // exactly the shape process_vm_readv reports as a short read rather
        // than an error. PROT_NONE rather than munmap because tests run in
        // parallel and a genuine hole is an address another test's allocation
        // could land in, which would turn this into a flake.
        let mapped = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                page * 2,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(mapped, libc::MAP_FAILED, "test needs two throwaway pages");
        unsafe {
            std::ptr::write_bytes(mapped as *mut u8, 0xAB, page);
            assert_eq!(
                libc::mprotect(mapped.add(page) as *mut c_void, page, libc::PROT_NONE),
                0,
                "second page must become unreadable"
            );
        }

        let process = this_process();
        let mut buffer = vec![0u8; page * 2];
        let read = process
            .read(mapped as usize, &mut buffer)
            .expect("the first page is mapped, so this must not error");
        assert_eq!(read, page, "read must stop exactly at the hole");
        assert!(buffer[..page].iter().all(|&byte| byte == 0xAB));

        unsafe {
            libc::munmap(mapped, page * 2);
        }
    }

    /// The probe answers without walking memory, so what matters is that each
    /// cached-region result maps onto the outcome the caller's escalation
    /// policy keys off, and that only `Updated` puts an inventory on the
    /// channel.
    #[test]
    fn probe_outcomes_distinguish_fresh_unchanged_and_miss() {
        let _digest_guard = blob_digest_test_guard();
        let data = blob(42);
        let process = this_process();
        let (blob_tx, blob_rx) = std::sync::mpsc::channel();

        reset_last_blob_region();
        LAST_BLOB_REGION.store(data.as_ptr() as u64, Ordering::Relaxed);

        let outcome = probe_outcome(probe(&process, vec![region(&data)]), &blob_tx);
        assert_eq!(outcome, ScanOutcome::Updated);
        assert_eq!(blob_rx.try_recv().expect("a fresh blob is sent on").credits, 42);

        let outcome = probe_outcome(probe(&process, vec![region(&data)]), &blob_tx);
        assert_eq!(outcome, ScanOutcome::Unchanged);
        assert!(blob_rx.try_recv().is_err(), "unchanged bytes must not re-send the inventory");

        // A mission reward delta shares the blob's field names but describes a
        // single mission, so it must read as a miss rather than as inventory.
        let mut delta = br#"{"InventoryChanges":{"MiscItems":[],"SubscribedToEmails":true,"#.to_vec();
        delta.resize(64_000, b' ');
        delta.extend_from_slice(BLOB_TAIL);
        LAST_BLOB_REGION.store(delta.as_ptr() as u64, Ordering::Relaxed);
        let outcome = probe_outcome(probe(&process, vec![region(&delta)]), &blob_tx);
        assert_eq!(outcome, ScanOutcome::CacheMiss);

        LAST_BLOB_REGION.store(data.as_ptr() as u64 + 8, Ordering::Relaxed);
        let outcome = probe_outcome(probe(&process, vec![region(&data)]), &blob_tx);
        assert_eq!(outcome, ScanOutcome::CacheMiss);
        assert!(blob_rx.try_recv().is_err(), "a miss must not send anything");

        reset_last_blob_region();
    }

    #[test]
    fn linux_cached_blob_is_reread_and_rejected_when_stale() {
        let _digest_guard = blob_digest_test_guard();
        let data = blob(42);
        let process = this_process();

        LAST_BLOB_REGION.store(data.as_ptr() as u64, Ordering::Relaxed);
        match probe(&process, vec![region(&data)]).expect("cached blob is re-read") {
            CachedBlobScan::Fresh(_, inventory) => assert_eq!(inventory.credits, 42),
            CachedBlobScan::Unchanged => panic!("first sighting of this blob must parse"),
        }

        // An address that no longer starts a blob must fall back to the walk
        // rather than reporting whatever happens to live there now.
        LAST_BLOB_REGION.store(data.as_ptr() as u64 + 8, Ordering::Relaxed);
        assert!(probe(&process, vec![region(&data)]).is_none());

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
            LinuxRegion { start: base, len: first.len(), ..Default::default() },
            LinuxRegion { start: base + first.len(), len: second.len(), ..Default::default() },
        ];
        let process = this_process();
        LAST_BLOB_REGION.store(base as u64, Ordering::Relaxed);
        match probe(&process, regions).expect("split marker is still found") {
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
            LinuxRegion { start: base, len: first.len(), ..Default::default() },
            LinuxRegion { start: base + first.len(), len: second.len(), ..Default::default() },
        ];
        let process = this_process();
        LAST_BLOB_REGION.store(base as u64, Ordering::Relaxed);
        match probe(&process, regions).expect("blob completed by the next mapping") {
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
            LinuxRegion { start: base, len: head.len(), ..Default::default() },
            LinuxRegion {
                start: base + head.len(),
                len: code.len(),
                executable: true,
                ..Default::default()
            },
            LinuxRegion {
                start: base + head.len() + code.len(),
                len: tail.len(),
                ..Default::default()
            },
        ];
        let process = this_process();
        LAST_BLOB_REGION.store(base as u64, Ordering::Relaxed);
        assert!(
            probe(&process, regions).is_none(),
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
        let regions = vec![LinuxRegion { start: base, len: filler.len(), ..Default::default() }];

        let process = this_process();
        LAST_BLOB_REGION.store((base + filler.len()) as u64, Ordering::Relaxed);
        assert!(
            probe(&process, regions).is_none(),
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
        let regions = vec![LinuxRegion {
            start: base + hole.len(),
            len: arena.len() - hole.len(),
            ..Default::default()
        }];

        let process = this_process();
        LAST_BLOB_REGION.store(base as u64, Ordering::Relaxed);
        assert!(
            probe(&process, regions).is_none(),
            "an unmapped seed must miss, not adopt the next mapping's blob"
        );

        reset_last_blob_region();
    }

    #[test]
    fn linux_cached_blob_skips_reparse_when_bytes_are_unchanged() {
        let _digest_guard = blob_digest_test_guard();
        let data = blob(99);
        let process = this_process();

        reset_last_blob_region();
        LAST_BLOB_REGION.store(data.as_ptr() as u64, Ordering::Relaxed);
        match probe(&process, vec![region(&data)]).expect("first scan parses the blob") {
            CachedBlobScan::Fresh(_, inventory) => assert_eq!(inventory.credits, 99),
            CachedBlobScan::Unchanged => panic!("first sighting of this blob must parse"),
        }

        match probe(&process, vec![region(&data)]).expect("second scan still finds the region") {
            CachedBlobScan::Unchanged => {}
            CachedBlobScan::Fresh(..) => panic!("identical bytes must not be reparsed"),
        }

        reset_last_blob_region();
        LAST_BLOB_REGION.store(data.as_ptr() as u64, Ordering::Relaxed);
        match probe(&process, vec![region(&data)]).expect("scan after reset parses again") {
            CachedBlobScan::Fresh(_, inventory) => assert_eq!(inventory.credits, 99),
            CachedBlobScan::Unchanged => panic!("reset must force a reparse"),
        }

        reset_last_blob_region();
    }

    #[test]
    fn linux_credential_scan_skips_executable_and_reads_data_regions() {
        let login = br#"{"id":"594144e63ade7f2f2091c48e","Nonce":123456789,"steamId=76561198000000000"}"#;
        let code = login.to_vec();
        let data = login.to_vec();
        let regions = vec![
            LinuxRegion { executable: true, ..region(&code) },
            region(&data),
        ];
        let process = this_process();

        let found = scan_linux_credential_regions(&process, regions);

        assert_eq!(
            found,
            Some((
                "594144e63ade7f2f2091c48e".to_string(),
                "123456789".to_string(),
                "76561198000000000".to_string(),
            ))
        );
    }

    #[test]
    fn linux_inventory_scan_reports_unchanged_instead_of_reparsing() {
        let _digest_guard = blob_digest_test_guard();
        let mapping = blob(7);
        let process = this_process();

        let (found, blobs) = walk(&process, vec![region(&mapping)]);
        assert!(found.is_some(), "the first walk has no baseline to match");
        assert_eq!(blobs.try_recv().expect("inventory blob is found").credits, 7);

        let (found, blobs) = walk(&process, vec![region(&mapping)]);
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
        let mapping = blob(42);
        let regions = vec![LinuxRegion {
            path: Some("/usr/lib/warframe/data.pak".into()),
            ..region(&mapping)
        }];
        let process = this_process();

        let (_, blobs) = walk(&process, regions);

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
        let anonymous = blob(1);
        let file_backed = blob(2);
        let regions = vec![
            region(&anonymous),
            LinuxRegion {
                path: Some("/usr/lib/warframe/data.pak".into()),
                ..region(&file_backed)
            },
        ];
        let process = this_process();

        let (_, blobs) = walk(&process, regions);

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
        let first = blob_head(42);
        let mut second = BLOB_TAIL.to_vec();
        second.resize(64_000, 0);
        let process = this_process();

        let (_, blobs) = walk(&process, vec![region(&first), region(&second)]);

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
            br#"{"SubscribedToEmails":true,"RegularCredits":42,"MiscItems":[{"ItemType":"/Lotus/Types/Items/x"}],"XPInfo":[],"FusionPoints":0,"Suits":[{"ItemType":"/Lotus/Powersuits/Mag/Mag"}],"#
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
            LinuxRegion { start: base, len: prefix.len(), ..Default::default() },
            LinuxRegion { start: base + prefix.len(), len: blob.len(), ..Default::default() },
        ];
        let process = this_process();

        let (_, blobs) = walk(&process, regions);

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
        let process = this_process();

        let (found, blobs) = walk(&process, vec![region(&mission)]);

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
            LinuxRegion { start: base, len: opening.len(), ..Default::default() },
            LinuxRegion { start: base + opening.len(), len: marker_flush.len(), ..Default::default() },
            LinuxRegion {
                start: base + opening.len() + marker_flush.len(),
                len: filler.len(),
                ..Default::default()
            },
            LinuxRegion {
                start: base + opening.len() + marker_flush.len() + filler.len(),
                len: closing.len(),
                ..Default::default()
            },
        ];
        let process = this_process();

        let (_, blobs) = walk(&process, regions);

        assert_eq!(blobs.try_recv().expect("blob completed once brace lands").credits, 42);
        reset_last_blob_region();
    }

    /// The scan budget is spent on what a mapping can still contribute rather
    /// than on the mapping whole. A blob that closes inside an oversized
    /// mapping still parses instead of being dropped with it.
    #[test]
    fn linux_inventory_scan_finishes_before_rejecting_large_mapping() {
        let _digest_guard = blob_digest_test_guard();
        let first = blob_head(42);
        let mut second = vec![0; 64 * 1024 * 1024];
        second[..BLOB_TAIL.len()].copy_from_slice(BLOB_TAIL);
        let process = this_process();

        let (_, blobs) = walk(&process, vec![region(&first), region(&second)]);

        assert_eq!(blobs.try_recv().expect("inventory blob is found").credits, 42);
        reset_last_blob_region();
    }

    #[test]
    fn linux_inventory_scan_finds_blob_past_first_chunk_boundary() {
        let _digest_guard = blob_digest_test_guard();

        let mapping = blob(42);
        let blob_offset = WALK_CHUNK + 1024;
        let mut arena = vec![0u8; blob_offset + mapping.len()];
        arena[blob_offset..blob_offset + mapping.len()].copy_from_slice(&mapping);
        let process = this_process();

        let (_, blobs) = walk(&process, vec![region(&arena)]);

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

        let mapping = blob(42);
        // Start marker a few KiB before the seam, end marker (~offset 64 000)
        // well past it, so the blob body crosses the A/B boundary.
        let blob_offset = WALK_CHUNK - 8192;
        let mut arena = vec![0u8; blob_offset + mapping.len()];
        arena[blob_offset..blob_offset + mapping.len()].copy_from_slice(&mapping);
        let process = this_process();

        let (_, blobs) = walk(&process, vec![region(&arena)]);

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
            br#"{"RegularCredits":42,"MiscItems":[{"ItemType":"/Lotus/Test","ItemCount":1}],"XPInfo":[],"FusionPoints":0,"Suits":[{"ItemType":"/Lotus/Powersuits/Mag/Mag"}],"#;
        let suffix = br#""SubscribedToEmails":true,"DeathSquadable":false}"#;
        let mut mapping = vec![b' '; 128_000];
        mapping[..prefix.len()].copy_from_slice(prefix);
        mapping[64_000..64_000 + suffix.len()].copy_from_slice(suffix);
        let base = mapping.as_ptr() as usize;
        let regions = vec![
            LinuxRegion { start: base, len: 64_000, ..Default::default() },
            LinuxRegion { start: base + 64_000, len: 64_000, ..Default::default() },
        ];
        let process = this_process();

        let (_, blobs) = walk(&process, regions);

        let inventory = blobs.try_recv().expect("inventory blob is found");
        assert_eq!(inventory.credits, 42);
        assert_eq!(inventory.stackable_items.len(), 1);
        reset_last_blob_region();
    }

    #[test]
    fn parses_only_readable_linux_mappings() {
        let maps = "1000-2000 r--p 0 00:00 0\n2000-2800 --xp 0 00:00 0\n3000-5000 rw-p 0 00:00 0\n";
        assert_eq!(
            parse_linux_maps(maps),
            vec![
                LinuxRegion {
                    start: 0x1000,
                    len: 0x1000,
                    executable: false,
                    ..Default::default()
                },
                LinuxRegion {
                    start: 0x3000,
                    len: 0x2000,
                    executable: false,
                    ..Default::default()
                },
            ]
        );
    }

    #[test]
    fn classifies_pathnames_and_pseudo_paths() {
        let maps = "\
1000-2000 rw-p 0 00:00 0 \n\
2000-3000 rw-p 0 00:00 0 [heap]\n\
3000-4000 rw-p 0 00:00 0 [stack]\n\
4000-5000 r--p 0 00:00 0 [vvar]\n\
5000-6000 r--p 0 00:00 0 [vsyscall]\n\
6000-7000 r--p 0 08:01 123 /usr/lib/warframe/Warframe.x64.exe\n";
        let regions = parse_linux_maps(maps);

        assert_eq!(regions[0].path, None);
        assert!(regions[0].is_anonymous() && !regions[0].is_file_backed());

        assert_eq!(regions[1].path.as_deref(), Some("[heap]"));
        assert!(regions[1].is_anonymous() && !regions[1].is_file_backed());

        assert_eq!(regions[2].path.as_deref(), Some("[stack]"));
        assert!(regions[2].is_anonymous() && !regions[2].is_file_backed());

        assert_eq!(regions[3].path.as_deref(), Some("[vvar]"));
        assert!(!regions[3].is_anonymous() && !regions[3].is_file_backed());

        assert_eq!(regions[4].path.as_deref(), Some("[vsyscall]"));
        assert!(!regions[4].is_anonymous() && !regions[4].is_file_backed());

        assert_eq!(
            regions[5].path.as_deref(),
            Some("/usr/lib/warframe/Warframe.x64.exe")
        );
        assert!(!regions[5].is_anonymous() && regions[5].is_file_backed());
    }

    /// A Steam library folder with a space in its name, plus the " (deleted)"
    /// suffix an in-place game update leaves behind, are both shapes the game
    /// image really appears in, and either one silently breaks the span match
    /// if the pathname is read as a single whitespace token.
    #[test]
    fn game_image_span_survives_spaced_and_deleted_pathnames() {
        let maps = "\
1000-2000 r--p 0 08:01 1 /mnt/Games Drive/steamapps/common/Warframe/Warframe.x64.exe\n\
2000-5000 r-xp 1000 08:01 1 /mnt/Games Drive/steamapps/common/Warframe/Warframe.x64.exe (deleted)\n\
9000-a000 rw-p 0 00:00 0 [heap]\n";
        let regions = parse_linux_maps(maps);

        assert_eq!(
            regions[0].path.as_deref(),
            Some("/mnt/Games Drive/steamapps/common/Warframe/Warframe.x64.exe")
        );
        assert_eq!(regions[1].path.as_deref(), regions[0].path.as_deref());
        assert_eq!(linux_game_image_span(&regions), Some(0x1000..0x5000));
    }

    #[test]
    fn recognizes_warframe_but_not_launcher_processes() {
        assert!(is_warframe_command(
            "Z:\\Warframe\\Warframe.x64.exe -cluster:public"
        ));
        assert!(!is_warframe_command(
            "Z:\\Warframe\\Tools\\Launcher.exe"
        ));
        assert!(!is_warframe_command("warframe-companion"));
    }
}
