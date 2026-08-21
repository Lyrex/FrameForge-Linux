//! Memory-region sources for the blob-stitch engine.
//!
//! `WindowsRegionSource` reads regions from a running game.
//! `RecordedRegions` replays recorded regions, so tests can run without the game.

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

/// Walks a live Warframe process with `VirtualQueryEx`/`ReadProcessMemory`.
/// Closes the handle on drop.
///
/// `next_region` filters to committed, readable, non-code, non-image regions
/// of at least `min_region` bytes. The filter drops PE image sections because
/// they hold exe/DLL string constants that false-trigger the Lotus anchor
/// check and cost tens of seconds.
#[cfg(target_os = "windows")]
pub struct WindowsRegionSource {
    process: windows_sys::Win32::Foundation::HANDLE,
    addr: usize,
    min_region: usize,
    read_cap: usize,
    /// Read buffer reused across `next_region` calls.
    buffer: Vec<u8>,
    // Diagnostics for the capture-done log. `Cell`s because `query`/`read`
    // run from both `&self` (`read_at`) and `&mut self` (`next_region`).
    skipped: std::cell::Cell<usize>,
    query_time: std::cell::Cell<std::time::Duration>,
    read_time: std::cell::Cell<std::time::Duration>,
}

#[cfg(target_os = "windows")]
impl WindowsRegionSource {
    pub fn open(pid: u32, min_region: usize, read_cap: usize) -> Option<Self> {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
        };
        let process = unsafe {
            OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid)
        };
        if process == 0 {
            return None;
        }
        Some(Self {
            process,
            addr: 0,
            min_region,
            read_cap,
            buffer: Vec::new(),
            skipped: Default::default(),
            query_time: Default::default(),
            read_time: Default::default(),
        })
    }

    /// `(regions_skipped, vquery_ms, read_ms)` accumulated since `open`.
    pub fn stats(&self) -> (usize, f64, f64) {
        (
            self.skipped.get(),
            self.query_time.get().as_secs_f64() * 1000.0,
            self.read_time.get().as_secs_f64() * 1000.0,
        )
    }

    /// Queries the region containing `addr`. `None` when the query fails or
    /// the region would not advance a walk past `addr` (loop/overflow guard).
    fn query(
        &self,
        addr: usize,
    ) -> Option<windows_sys::Win32::System::Memory::MEMORY_BASIC_INFORMATION> {
        use std::ffi::c_void;
        use std::mem;
        use windows_sys::Win32::System::Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION};

        let t = std::time::Instant::now();
        let mut mbi = unsafe { mem::zeroed::<MEMORY_BASIC_INFORMATION>() };
        let ok = unsafe {
            VirtualQueryEx(
                self.process,
                addr as *const c_void,
                &mut mbi,
                mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } != 0;
        self.query_time.set(self.query_time.get() + t.elapsed());
        if !ok {
            return None;
        }
        let next_addr = (mbi.BaseAddress as usize).saturating_add(mbi.RegionSize);
        if next_addr <= addr {
            return None;
        }
        Some(mbi)
    }

    fn unreadable(
        mbi: &windows_sys::Win32::System::Memory::MEMORY_BASIC_INFORMATION,
    ) -> bool {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS};
        mbi.State != MEM_COMMIT
            || mbi.Protect & PAGE_GUARD != 0
            || mbi.Protect & PAGE_NOACCESS != 0
    }

    /// Reads up to `len.min(read_cap)` bytes at `addr` into `buf`, returning
    /// the byte count actually read. `None` when the read fails outright.
    /// Bytes in `buf` past that count are stale from an earlier read.
    ///
    /// The cap can leave a region's tail unread while the walk advances past
    /// it. That is harmless while `read_cap` (64 MB) exceeds the engine's
    /// scan limit (20 MB): a blob crossing the cap would be dropped anyway.
    fn read_into(&self, addr: usize, len: usize, buf: &mut Vec<u8>) -> Option<usize> {
        use std::ffi::c_void;
        use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;

        let t = std::time::Instant::now();
        let len = len.min(self.read_cap);
        // Grow-only, so a reused buffer is zeroed once per high-water mark
        // rather than once per region.
        if buf.len() < len {
            buf.resize(len, 0);
        }
        let mut n = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                self.process,
                addr as *const c_void,
                buf.as_mut_ptr() as *mut c_void,
                len,
                &mut n,
            )
        } != 0;
        self.read_time.set(self.read_time.get() + t.elapsed());
        ok.then_some(n)
    }
}

#[cfg(target_os = "windows")]
impl RegionSource for WindowsRegionSource {
    fn next_region(&mut self) -> Option<(usize, &[u8])> {
        use windows_sys::Win32::System::Memory::MEM_IMAGE;

        // Executable pages never contain heap data, so they are safe to skip.
        const PAGE_EXECUTE: u32 = 0x10;
        const PAGE_EXECUTE_READ: u32 = 0x20;
        const PAGE_EXECUTE_RW: u32 = 0x40;
        const PAGE_EXECUTE_WC: u32 = 0x80;
        const EXEC_MASK: u32 =
            PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_RW | PAGE_EXECUTE_WC;

        loop {
            let mbi = self.query(self.addr)?;
            let region_addr = mbi.BaseAddress as usize;
            let region_size = mbi.RegionSize;
            self.addr = region_addr.saturating_add(region_size);

            // ── Region filters ──────────────────────────────────────────────────
            // Skip pages that can never hold heap JSON:
            // • must be committed and readable
            // • skip execute-only pages (code sections, JIT stubs)
            // • skip PE image sections: their exe/DLL string constants
            //   false-trigger the Lotus anchor check and cost ~40 s scanning
            //   20 MB+ without ever finding the blob end
            // • skip anything smaller than MIN_REGION
            if Self::unreadable(&mbi)
                || mbi.Protect & EXEC_MASK != 0
                || mbi.Type == MEM_IMAGE
                || region_size < self.min_region
            {
                self.skipped.set(self.skipped.get() + 1);
                continue;
            }

            // Take the buffer out for the read: `read_into` borrows all of
            // `self` shared, so it cannot also receive `&mut self.buffer`.
            let mut buf = std::mem::take(&mut self.buffer);
            let read = self.read_into(region_addr, region_size, &mut buf);
            self.buffer = buf;
            match read {
                // Fewer than 8 bytes cannot hold any marker, so treat this as a failed read.
                Some(n) if n >= 8 => return Some((region_addr, &self.buffer[..n])),
                _ => continue,
            }
        }
    }

    fn read_at(&self, addr: usize, max_len: usize) -> Option<(usize, Vec<u8>)> {
        use windows_sys::Win32::System::Memory::MEM_IMAGE;

        let mbi = self.query(addr)?;
        let next_addr = (mbi.BaseAddress as usize).saturating_add(mbi.RegionSize);
        // Image sections get the empty-bytes treatment too: a stale cached
        // address can land in a remapped DLL, whose string constants can only
        // false-trigger the anchor checks.
        if Self::unreadable(&mbi) || mbi.Type == MEM_IMAGE {
            return Some((next_addr, Vec::new()));
        }
        // Read from the requested address, not the region base: the cached
        // blob-start address sits mid-region, and the caller checks that the
        // returned bytes begin with the blob's opening `{"`.
        let mut bytes = Vec::new();
        let n = self
            .read_into(addr, (next_addr - addr).min(max_len), &mut bytes)
            .unwrap_or(0);
        bytes.truncate(n);
        // A read cut short by `max_len` (or by a fault) resumes at its own
        // end, not at the region end. The stitch then never crosses bytes it
        // has not read.
        if !bytes.is_empty() && addr + bytes.len() < next_addr {
            return Some((addr + bytes.len(), bytes));
        }
        Some((next_addr, bytes))
    }
}

#[cfg(target_os = "windows")]
impl Drop for WindowsRegionSource {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        unsafe { CloseHandle(self.process) };
    }
}
