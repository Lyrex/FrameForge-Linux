//! Memory-region sources for the blob-stitch engine.
//!
//! `ProcessRegions` reads regions from a running game via the platform
//! abstraction. `RecordedRegions` replays recorded regions for tests.
//!
//! The Linux walk needs a per-tier region order and a deadline, so it builds
//! its own source and leaves `ProcessRegions` with no caller there.
#![allow(dead_code)]

use crate::platform::{MemoryRegionInfo, Platform, ProcessAccess, ProcessHandle, RegionBacking};
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
/// Each `next_region` yields `(base_address, bytes)` for one region, and
/// `None` ends the walk. The bytes are lent until the next call, so the
/// source can reuse one read buffer instead of allocating per region.
/// The walk copies what it keeps.
///
/// `read_at` serves the cached-blob fast path. It returns the bytes starting
/// at `addr` itself, not at the containing region's base, plus the address
/// just past the bytes read, so the caller can stitch forward. `max_len` is
/// how many bytes the caller can still use; the source cannot know the budget.
///
/// The stitch splices what `read_at` returns into one contiguous blob, so a
/// source must never paper over a hole: the bytes it returns for `addr` have
/// to live at `addr`, and an address it cannot vouch for ends the stitch
/// rather than skipping forward to the next readable mapping.
pub trait RegionSource {
    /// Yields the next committed, readable region. The bytes are valid until
    /// the next call to `next_region`.
    fn next_region(&mut self) -> Option<(usize, &[u8])>;

    /// Read at most `max_len` bytes starting at `addr`.
    fn read_at(&self, addr: usize, max_len: usize) -> Option<(usize, Vec<u8>)>;

    fn stats(&self) -> RegionStats { RegionStats::default() }
}

/// Open a region source for the given process. Returns `None` on unsupported
/// platforms or when the process cannot be opened.
pub fn open_region_source(pid: u32, min_region: usize, read_cap: usize) -> Option<Box<dyn RegionSource>> {
    let handle = Platform::open_process(pid).ok()?;
    Some(Box::new(ProcessRegions::new(handle, min_region, read_cap)))
}

// ─── ProcessRegions ───────────────────────────────────────────────────────────

/// Platform-independent region source that reads through a `ProcessHandle`.
/// The handle is owned so `Box<dyn RegionSource>` needs no lifetime parameter.
///
/// Regions are queried lazily via `handle.regions_from()` on each
/// `next_region()` call, so the handle borrow is always short-lived.
pub struct ProcessRegions {
    handle: Box<dyn ProcessHandle>,
    addr: usize,
    min_region: usize,
    read_cap: usize,
    buf: Vec<u8>,
    stats: Stats,
}

impl ProcessRegions {
    pub fn new(handle: Box<dyn ProcessHandle>, min_region: usize, read_cap: usize) -> Self {
        Self {
            handle,
            addr: 0,
            min_region,
            read_cap,
            buf: Vec::new(),
            stats: Stats::default(),
        }
    }

    /// Query the next region from the handle, filter, and return it.
    /// The borrow on `self.handle` ends when `regions_from().next()` returns,
    /// so `self.handle` is free for `read_into` afterwards.
    fn next_passing_region(&mut self) -> Option<MemoryRegionInfo> {
        let t = std::time::Instant::now();
        // Executable pages never contain heap data, so they are safe to skip.
        loop {
            let region = self.handle.regions_from(self.addr).next()?;
            self.addr = region.base_address + region.region_size;
            self.stats.enumerate_ms.set(
                self.stats.enumerate_ms.get() + t.elapsed().as_secs_f64() * 1000.0,
            );

            if !region.is_committed || !region.is_readable {
                self.stats.regions_skipped.set(self.stats.regions_skipped.get() + 1);
                continue;
            }
            if region.is_executable {
                self.stats.regions_skipped.set(self.stats.regions_skipped.get() + 1);
                continue;
            }
            if region.backing == RegionBacking::File {
                self.stats.regions_skipped.set(self.stats.regions_skipped.get() + 1);
                continue;
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
        let region = self.next_passing_region()?;

        let t = std::time::Instant::now();
        let len = region.region_size.min(self.read_cap);
        self.buf.resize(len, 0);
        let n = self.handle.read_into(region.base_address, &mut self.buf);
        self.stats.read_ms.set(
            self.stats.read_ms.get() + t.elapsed().as_secs_f64() * 1000.0,
        );

        if n < 8 {
            return self.next_region();
        }
        self.buf.truncate(n);
        Some((region.base_address, &self.buf))
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
