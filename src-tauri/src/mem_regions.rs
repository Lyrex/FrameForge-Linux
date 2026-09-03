//! Memory-region sources for the blob-stitch engine.
//!
//! `LinuxRegionSource` (declared in `memory_scanner_linux`) walks a live
//! process via `/proc`; `RecordedRegions` replays recorded regions, so tests
//! can run without the game.

/// Streams a target's readable memory regions in ascending-address order.
///
/// Each `next_region` yields `(base_address, bytes)` for one region, and
/// `None` ends the walk. The engine relies only on ascending addresses and
/// on `bytes` starting at `base_address`. Filtering (protection, size, image
/// sections), read caps, and skipping unreadable regions are the source's
/// own policy.
///
/// The bytes are lent, valid until the next call. The walk copies what it
/// keeps anyway, and lending lets a source reuse one read buffer instead of
/// allocating and zeroing up to a chunk-cap-sized `Vec` per region.
///
/// `read_at` serves the cached-blob fast path. It returns up to `max_len`
/// bytes starting at `addr` itself, not at the containing region's base. It
/// also returns the address the stitch should continue at. The size filter
/// of `next_region` does not apply: a caller probing a known address only
/// cares whether it is still readable. Either empty bytes or `None` ends the
/// stitch. A source can report an unreadable address as whichever of the two
/// suits how it enumerates memory.
///
/// The stitch splices the returned bytes into one contiguous blob, so a
/// source must never paper over a hole. Bytes it returns for `addr` must
/// actually live at `addr`. An address it cannot vouch for ends the stitch
/// instead of skipping forward to the next readable mapping. Whether the
/// answers come from live queries or from a snapshot taken at open is the
/// source's own policy. A snapshot only ages by the milliseconds a probe
/// runs.
pub trait RegionSource {
    fn next_region(&mut self) -> Option<(usize, &[u8])>;
    fn read_at(&self, addr: usize, max_len: usize) -> Option<(usize, Vec<u8>)>;
}

#[cfg(test)]
pub struct RecordedRegions {
    regions: Vec<(usize, Vec<u8>)>,
    pos: usize,
}

#[cfg(test)]
impl RecordedRegions {
    pub fn new(regions: Vec<(usize, Vec<u8>)>) -> Self {
        Self { regions, pos: 0 }
    }
}

#[cfg(test)]
impl RegionSource for RecordedRegions {
    fn next_region(&mut self) -> Option<(usize, &[u8])> {
        let pos = self.pos;
        self.pos += 1;
        let (base, bytes) = self.regions.get(pos)?;
        Some((*base, bytes.as_slice()))
    }

    fn read_at(&self, addr: usize, max_len: usize) -> Option<(usize, Vec<u8>)> {
        for (base, bytes) in &self.regions {
            let end = base + bytes.len();
            if (*base..end).contains(&addr) {
                let bytes = &bytes[addr - base..];
                let bytes = &bytes[..bytes.len().min(max_len)];
                return Some((addr + bytes.len(), bytes.to_vec()));
            }
        }
        None
    }
}
