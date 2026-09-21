//! Linux platform implementation.
//!
//! Reading another process goes through `/proc/pid/maps` for the mappings
//! and `process_vm_readv` for the bytes.

use tracing::warn;

use super::{MemoryRegionInfo, ProcessHandle, RegionBacking};

/// The keyring calls block for as long as the user takes to answer an unlock
/// prompt, and Tauri runs a sync command on the main thread, so the commands in
/// [`crate::credentials`] stay async and hand these to `spawn_blocking`. Call
/// them from a blocking context only.
pub fn save_credentials(target: &str, email: &str, token: &str) -> Result<(), String> {
    crate::credentials::secret_save(target, email, token)
}

pub fn load_credentials(target: &str) -> Result<Option<(String, String)>, String> {
    crate::credentials::secret_load(target)
}

pub fn delete_credentials(target: &str) -> Result<(), String> {
    crate::credentials::secret_delete(target)
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

/// Fails with the ptrace_scope guidance when `process_vm_readv` would return
/// EPERM for every mapping, which a walk would otherwise report as a missing
/// blob.
pub fn open_process(pid: u32) -> Result<Box<dyn ProcessHandle>, String> {
    check_process_memory_access(pid)?;
    Ok(Box::new(LinuxProcess { pid }))
}

struct LinuxProcess {
    pid: u32,
}

impl ProcessHandle for LinuxProcess {
    fn read_into(&self, addr: usize, buf: &mut [u8]) -> usize {
        read_process_memory(self.pid, addr, buf).unwrap_or(0)
    }

    /// Each call re-reads and re-parses `/proc/pid/maps`. A caller that walks
    /// many regions should collect this once instead of restarting the
    /// iterator per region.
    fn regions_from(&self, from: usize) -> Box<dyn Iterator<Item = MemoryRegionInfo> + '_> {
        let path = format!("/proc/{}/maps", self.pid);
        let maps = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            warn!(%path, %error, "failed to read process mappings; ensure kernel.yama.ptrace_scope permits same-user process access");
            String::new()
        });
        Box::new(
            parse_linux_maps(&maps)
                .into_iter()
                .filter(move |region| region.start + region.len > from)
                .map(MemoryRegionInfo::from),
        )
    }
}

/// Opening `/proc/pid/mem` runs the same `PTRACE_MODE_ATTACH` check as
/// `process_vm_readv`.
fn check_process_memory_access(pid: u32) -> Result<(), String> {
    let path = format!("/proc/{pid}/mem");
    std::fs::File::open(&path).map(drop).map_err(|error| {
        format!(
            "Failed to open {path}: {error}. Ensure kernel.yama.ptrace_scope permits same-user process access"
        )
    })
}

/// Do not retry an EFAULT through `/proc/pid/mem`. It reads the game's GPU
/// buffers through the driver's aperture while holding locks the render
/// thread needs, and every mapping that can hold the blob is readable here.
fn read_process_memory(pid: u32, address: usize, buffer: &mut [u8]) -> std::io::Result<usize> {
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
    Err(std::io::Error::last_os_error())
}

#[derive(Debug, PartialEq, Eq)]
struct LinuxRegion {
    start: usize,
    len: usize,
    writable: bool,
    executable: bool,
    // `/proc/pid/maps`'s 6th field: absent for anonymous mappings, a real
    // path for file-backed ones, or a kernel pseudo-path like `[heap]`,
    // `[stack]`, `[vvar]`, `[vsyscall]`.
    path: Option<Box<str>>,
}

impl From<LinuxRegion> for MemoryRegionInfo {
    fn from(region: LinuxRegion) -> Self {
        Self {
            base_address: region.start,
            region_size: region.len,
            is_committed: true,
            is_readable: true,
            is_writable: region.writable,
            is_executable: region.executable,
            backing: region.backing(),
        }
    }
}

impl LinuxRegion {
    /// `[heap]` and `[stack]` count as anonymous. The kernel labels them but
    /// they hold untagged process memory, and a 105 MB `[heap]` mapping is
    /// where a multi-megabyte JSON blob turns up. The other bracketed
    /// pseudo-paths (`[vvar]`, `[vsyscall]`, ...) are kernel data pages that
    /// can never hold heap JSON.
    fn backing(&self) -> RegionBacking {
        match self.path.as_deref() {
            None | Some("[heap]") | Some("[stack]") => RegionBacking::Anonymous,
            Some(path) if path.starts_with('[') => RegionBacking::Kernel,
            Some(_) => RegionBacking::File,
        }
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
            // game shows up — is not part of the path.
            let path = fields.nth(3).map(|name| {
                let offset = name.as_ptr() as usize - line.as_ptr() as usize;
                let path = line[offset..].trim_end();
                Box::from(path.strip_suffix(" (deleted)").unwrap_or(path))
            });
            Some(LinuxRegion {
                start,
                len: end.checked_sub(start)?,
                writable: permissions.as_bytes().get(1) == Some(&b'w'),
                executable: permissions.as_bytes().get(2) == Some(&b'x'),
                path,
            })
        })
        .collect()
}

pub fn get_system_locale() -> String {
    // POSIX locales look like "de_DE.UTF-8" or "de_DE@euro"; the frontend
    // feeds this to Intl, which wants a BCP-47 tag like "de-DE". LC_TIME
    // outranks LANG because the locale only ever picks the clock format.
    let posix = ["LC_ALL", "LC_TIME", "LANG"].iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|s| !s.is_empty());
    if let Some(lang) = posix {
        let tag = lang.split(['.', '@']).next().unwrap_or("").replace('_', "-");
        if !tag.is_empty() && tag != "C" && tag != "POSIX" {
            return tag;
        }
    }
    "en-US".to_string()
}

pub fn get_warframe_window_rect() -> Result<[i32; 4], String> {
    crate::ocr::warframe_window_rect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn this_process() -> Box<dyn ProcessHandle> {
        open_process(std::process::id()).expect("current process is readable")
    }

    #[test]
    fn reads_its_own_mapping() {
        let marker = b"frameforge-linux-reader";
        let process = this_process();
        let mut actual = vec![0; marker.len()];
        let read = process.read_into(marker.as_ptr() as usize, &mut actual);
        assert_eq!(read, marker.len());
        assert_eq!(actual, marker);
    }

    #[test]
    fn reports_efault_for_an_unmapped_first_page() {
        let mut buffer = vec![0u8; 4096];
        // Below mmap_min_addr on every normal Linux config, so nothing is ever
        // mapped here.
        let error = read_process_memory(std::process::id(), 0x1000, &mut buffer)
            .expect_err("nothing is mapped at 0x1000");
        assert_eq!(error.raw_os_error(), Some(libc::EFAULT));
    }

    #[test]
    fn returns_leading_bytes_when_the_read_crosses_into_a_hole() {
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
                libc::mprotect(mapped.add(page), page, libc::PROT_NONE),
                0,
                "second page must become unreadable"
            );
        }

        let process = this_process();
        let mut buffer = vec![0u8; page * 2];
        let read = process.read_into(mapped as usize, &mut buffer);
        assert_eq!(read, page, "read must stop exactly at the hole");
        assert!(buffer[..page].iter().all(|&byte| byte == 0xAB));

        unsafe {
            libc::munmap(mapped, page * 2);
        }
    }

    #[test]
    fn parses_only_readable_mappings() {
        let maps = "1000-2000 r--p 0 00:00 0\n2000-2800 --xp 0 00:00 0\n3000-5000 rw-p 0 00:00 0\n";
        let regions = parse_linux_maps(maps);
        assert_eq!(
            regions.iter().map(|region| (region.start, region.len)).collect::<Vec<_>>(),
            vec![(0x1000, 0x1000), (0x3000, 0x2000)]
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

        assert_eq!(
            regions.iter().map(|region| region.path.as_deref()).collect::<Vec<_>>(),
            vec![
                None,
                Some("[heap]"),
                Some("[stack]"),
                Some("[vvar]"),
                Some("[vsyscall]"),
                Some("/usr/lib/warframe/Warframe.x64.exe"),
            ]
        );
        assert_eq!(
            regions.iter().map(LinuxRegion::backing).collect::<Vec<_>>(),
            vec![
                RegionBacking::Anonymous,
                RegionBacking::Anonymous,
                RegionBacking::Anonymous,
                RegionBacking::Kernel,
                RegionBacking::Kernel,
                RegionBacking::File,
            ]
        );
    }

    /// A Steam library folder with a space in its name, plus the " (deleted)"
    /// suffix an in-place game update leaves behind, are both shapes the game
    /// image really appears in, and either one mangles the path if the
    /// pathname is read as a single whitespace token.
    #[test]
    fn maps_pathnames_keep_spaces_and_drop_deleted_suffix() {
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
    }

    #[test]
    fn recognizes_warframe_but_not_launcher_processes() {
        assert!(is_warframe_command("Z:\\Warframe\\Warframe.x64.exe -cluster:public"));
        assert!(!is_warframe_command("Z:\\Warframe\\Tools\\Launcher.exe"));
        assert!(!is_warframe_command("warframe-companion"));
    }
}
