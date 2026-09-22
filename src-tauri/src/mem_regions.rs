//! Memory-region sources for the blob-stitch engine.
//!
//! `ProcessRegions` reads regions from a running game via the platform
//! abstraction. `RecordedRegions` replays recorded regions for tests.

use crate::platform::{MemoryRegionInfo, Platform, ProcessAccess, ProcessHandle};
use std::cell::Cell;

/// Statistics from a region walk, returned by `RegionSource::stats`.
#[derive(Debug, Default, Clone)]
pub struct RegionStats {
    pub regions_skipped: usize,
    pub enumerate_ms: f64,
    pub read_ms: f64,
}

/// Cell-based stats for interior mutability in `&self` methods.
#[derive(Debug, Default)]
struct Stats {
    regions_skipped: Cell<usize>,
    enumerate_ms: Cell<f64>,
    read_ms: Cell<f64>,
}

impl Stats {
    fn snapshot(&self) -> RegionStats {
        RegionStats {
            regions_skipped: self.regions_skipped.get(),
            enumerate_ms: self.enumerate_ms.get(),
            read_ms: self.read_ms.get(),
        }
    }
}

/// Streams a target's readable memory regions in ascending-address order.
///
/// Each `next_region` yields `(base_address, bytes)` for one chunk of a
/// region, and `None` ends the walk. Large regions are yielded in
/// `chunk`-sized pieces so the caller never misses data beyond `read_cap`.
///
/// ## Contract
///
/// The stitch splices what `read_at` returns into one contiguous blob, so a
/// source must never paper over a hole: the bytes it returns for `addr` have
/// to live at `addr`, and an address it cannot vouch for ends the stitch
/// rather than skipping forward to the next readable mapping.
///
/// On Linux, `process_vm_readv` returns a short read (not an error) when a
/// read crosses from a mapped page into an unmapped one. The source must
/// return only the bytes that were actually read and let the caller decide
/// whether to continue.
///
/// `read_at` serves the cached-blob fast path. It returns the bytes starting
/// at `addr` itself, plus the address just past the bytes read. `max_len` is
/// how many bytes the caller can still use.
pub trait RegionSource {
    /// Yields the next readable chunk. The bytes are valid until the next
    /// call to `next_region`. Returns `None` when the walk is done or the
    /// deadline has passed.
    fn next_region(&mut self) -> Option<(usize, &[u8])>;

    /// The region containing `addr`, or `None` when nothing is mapped there.
    /// On Windows, free address space is reported as a region with
    /// `is_committed: false`; callers must check `is_committed` rather than
    /// reading `Some` as "readable".
    fn region_at(&self, _addr: usize) -> Option<MemoryRegionInfo> {
        // Default: query one region from the handle.
        // Overridden by ProcessRegions which owns the handle.
        None
    }

    /// Read at most `max_len` bytes starting at `addr`. The returned bytes
    /// live at `addr`; a short read means the address is partially unmapped.
    fn read_at(&self, addr: usize, max_len: usize) -> Option<(usize, Vec<u8>)>;

    fn stats(&self) -> RegionStats { RegionStats::default() }
}

/// Open a region source for the given process. Returns `None` on unsupported
/// platforms or when the process cannot be opened.
pub fn open_region_source(
    pid: u32,
    min_region: usize,
    read_cap: usize,
    chunk: usize,
    deadline: Option<std::time::Instant>,
    filter: Option<Box<dyn Fn(&MemoryRegionInfo) -> bool + Send>>,
) -> Option<Box<dyn RegionSource>> {
    let handle = Platform::open_process(pid).ok()?;
    Some(Box::new(ProcessRegions::new(handle, min_region, read_cap, chunk, deadline, filter)))
}

// ─── ProcessRegions ───────────────────────────────────────────────────────────

/// Platform-independent region source that reads through a `ProcessHandle`.
/// The handle is owned so `Box<dyn RegionSource>` needs no lifetime parameter.
///
/// Large regions are yielded in `chunk`-sized pieces. The walk advances to
/// the next region only after all chunks of the current region have been
/// handed out, so no data is lost beyond `read_cap`.
pub struct ProcessRegions {
    handle: Box<dyn ProcessHandle>,
    addr: usize,
    min_region: usize,
    read_cap: usize,
    chunk: usize,
    deadline: Option<std::time::Instant>,
    filter: Option<Box<dyn Fn(&MemoryRegionInfo) -> bool + Send>>,
    /// Current region being yielded in chunks.
    current: Option<MemoryRegionInfo>,
    /// Byte offset within the current region for the next chunk.
    offset: usize,
    buf: Vec<u8>,
    stats: Stats,
}

impl ProcessRegions {
    pub fn new(
        handle: Box<dyn ProcessHandle>,
        min_region: usize,
        read_cap: usize,
        chunk: usize,
        deadline: Option<std::time::Instant>,
        filter: Option<Box<dyn Fn(&MemoryRegionInfo) -> bool + Send>>,
    ) -> Self {
        Self {
            handle,
            addr: 0,
            min_region,
            read_cap,
            chunk,
            deadline,
            filter,
            current: None,
            offset: 0,
            buf: Vec::new(),
            stats: Stats::default(),
        }
    }

    /// Check whether the deadline has been exceeded.
    fn expired(&self) -> bool {
        self.deadline.map_or(false, |d| std::time::Instant::now() >= d)
    }

    /// Query the next region from the handle, apply the caller-supplied
    /// filter, and return it. The borrow on `self.handle` ends when
    /// `regions_from().next()` returns, so `self.handle` is free for
    /// `read_into` afterwards.
    fn next_passing_region(&mut self) -> Option<MemoryRegionInfo> {
        let t = std::time::Instant::now();
        loop {
            if self.expired() { return None; }
            let region = self.handle.regions_from(self.addr).next()?;
            self.addr = region.base_address + region.region_size;
            self.stats.enumerate_ms.set(
                self.stats.enumerate_ms.get() + t.elapsed().as_secs_f64() * 1000.0,
            );

            if !region.is_committed || !region.is_readable {
                self.stats.regions_skipped.set(self.stats.regions_skipped.get() + 1);
                continue;
            }
            if let Some(ref f) = self.filter {
                if !f(&region) {
                    self.stats.regions_skipped.set(self.stats.regions_skipped.get() + 1);
                    continue;
                }
            }
            if region.region_size < self.min_region {
                self.stats.regions_skipped.set(self.stats.regions_skipped.get() + 1);
                continue;
            }
            return Some(region);
        }
    }
}

impl RegionSource for ProcessRegions {
    fn next_region(&mut self) -> Option<(usize, &[u8])> {
        loop {
            if self.expired() { return None; }

            // If we have a current region, yield the next chunk from it.
            if let Some(ref region) = self.current {
                if self.offset < region.region_size {
                    let base = region.base_address + self.offset;
                    let remaining = region.region_size - self.offset;
                    let len = self.chunk.min(self.read_cap).min(remaining);

                    let t = std::time::Instant::now();
                    self.buf.resize(len, 0);
                    let n = self.handle.read_into(base, &mut self.buf);
                    self.stats.read_ms.set(
                        self.stats.read_ms.get() + t.elapsed().as_secs_f64() * 1000.0,
                    );

                    self.offset += len;

                    if n == 0 {
                        // Unmapped hole — stop the stitch here.
                        self.current = None;
                        self.offset = 0;
                        return Some((base, &[]));
                    }
                    if n < len {
                        // Short read (faulted page) — return what we got
                        // and end this region.
                        self.buf.truncate(n);
                        let result = (base, &self.buf[..n]);
                        self.current = None;
                        self.offset = 0;
                        return Some(result);
                    }
                    self.buf.truncate(n);
                    return Some((base, &self.buf));
                }
                // Region fully consumed.
                self.current = None;
                self.offset = 0;
            }

            // Fetch the next passing region.
            let region = self.next_passing_region()?;
            self.current = Some(region);
            self.offset = 0;
        }
    }

    fn region_at(&self, addr: usize) -> Option<MemoryRegionInfo> {
        // Check if addr falls within the current region first.
        if let Some(ref region) = self.current {
            let region_end = region.base_address + region.region_size;
            if region.base_address <= addr && addr < region_end {
                return Some(region.clone());
            }
        }
        // Otherwise query the handle. The borrow is short-lived.
        self.handle.regions_from(addr)
            .next()
            .filter(|r| r.base_address <= addr)
    }

    fn read_at(&self, addr: usize, max_len: usize) -> Option<(usize, Vec<u8>)> {
        let read_len = self.read_cap.min(max_len);
        let mut buf = vec![0u8; read_len];
        let t = std::time::Instant::now();
        let n = self.handle.read_into(addr, &mut buf);
        self.stats.read_ms.set(
            self.stats.read_ms.get() + t.elapsed().as_secs_f64() * 1000.0,
        );

        if n == 0 {
            return Some((addr, Vec::new()));
        }
        buf.truncate(n);
        Some((addr + n, buf))
    }

    fn stats(&self) -> RegionStats {
        self.stats.snapshot()
    }
}

// ─── RecordedRegions (test only) ─────────────────────────────────────────────

#[cfg(test)]
pub struct RecordedRegions {
    regions: Vec<(usize, Vec<u8>)>,
    pos: usize,
    buf: Vec<u8>,
}

#[cfg(test)]
impl RecordedRegions {
    pub fn new(regions: Vec<(usize, Vec<u8>)>) -> Self {
        Self { regions, pos: 0, buf: Vec::new() }
    }
}

#[cfg(test)]
impl RegionSource for RecordedRegions {
    fn next_region(&mut self) -> Option<(usize, &[u8])> {
        let (base, bytes) = self.regions.get(self.pos)?;
        self.buf.clear();
        self.buf.extend_from_slice(bytes);
        self.pos += 1;
        Some((*base, &self.buf))
    }

    fn read_at(&self, addr: usize, max_len: usize) -> Option<(usize, Vec<u8>)> {
        for (base, bytes) in &self.regions {
            let end = base + bytes.len();
            if (*base..end).contains(&addr) {
                let avail = (end - addr).min(max_len);
                return Some((addr + avail, bytes[addr - base..][..avail].to_vec()));
            }
        }
        None
    }
}
