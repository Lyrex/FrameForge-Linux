//! Differential hunt for the riven reroll screen's validity flag under Proton.
//!
//! The Windows scanner locates that flag with an instruction pattern (Pattern
//! D-2). Ported to Linux the pattern matches exactly one site in the game's
//! code, but the byte it points at reads 1 both with a reroll screen open and
//! with the player in the orbiter, so it cannot drive the watcher.
//!
//! Rather than trusting the pattern, this finds the flag from its behaviour:
//! dump the game module's writable memory in each state and keep the bytes that
//! flip. The flag is a static in the executable, so the search is confined to
//! the address span the game's own named mappings bracket — a few tens of
//! megabytes rather than the multi-gigabyte address space.
//!
//! Usage, toggling the riven screen in game between snapshots:
//!
//!     cargo run --example riven_flag_hunt -- snapshot closed
//!     cargo run --example riven_flag_hunt -- snapshot open
//!     cargo run --example riven_flag_hunt -- diff closed open
//!
//! `diff` reports the bytes that were 0 in the first state and non-zero in the
//! second. Repeating with more snapshots narrows the survivors further.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::os::unix::fs::FileExt;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["snapshot", label] => snapshot(label),
        ["diff", before, after] => diff(before, after),
        ["narrow", off, on, off_again] => narrow(off, on, off_again),
        ["refs", target] => refs(target),
        ["sig"] => sig(),
        ["peek", addresses @ ..] if !addresses.is_empty() => peek(addresses),
        _ => Err("usage: snapshot <label> | diff <before> <after> | peek <hex-address>…".into()),
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

/// The game's writable mappings, as (address, bytes) pairs.
fn read_game_data(pid: u32) -> Result<Vec<(usize, Vec<u8>)>, String> {
    let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).map_err(|e| e.to_string())?;
    let memory = File::open(format!("/proc/{pid}/mem")).map_err(|e| {
        format!("{e}. Ensure kernel.yama.ptrace_scope permits same-user process access")
    })?;

    // Wine leaves only the PE headers and one data section file-backed, so the
    // module is the span its named mappings bracket, not the mappings alone.
    let mut span: Option<(usize, usize)> = None;
    for line in maps.lines() {
        if line.to_ascii_lowercase().ends_with("warframe.x64.exe") {
            let (start, end) = parse_range(line).ok_or("bad maps line")?;
            span = Some(match span {
                Some((low, high)) => (low.min(start), high.max(end)),
                None => (start, end),
            });
        }
    }
    let (span_start, span_end) = span.ok_or("Warframe.x64.exe is not mapped")?;
    println!("module span 0x{span_start:x}-0x{span_end:x}");

    let mut regions = Vec::new();
    for line in maps.lines() {
        let permissions = line.split_whitespace().nth(1).unwrap_or("");
        if !permissions.starts_with("rw") {
            continue;
        }
        let Some((start, end)) = parse_range(line) else { continue };
        if start < span_start || start >= span_end {
            continue;
        }
        let mut buffer = vec![0; end - start];
        match memory.read_at(&mut buffer, start as u64) {
            Ok(read) => {
                buffer.truncate(read);
                regions.push((start, buffer));
            }
            // Mappings can vanish mid-walk; a partial picture is still usable.
            Err(_) => continue,
        }
    }
    Ok(regions)
}

fn parse_range(line: &str) -> Option<(usize, usize)> {
    let (start, end) = line.split_whitespace().next()?.split_once('-')?;
    Some((
        usize::from_str_radix(start, 16).ok()?,
        usize::from_str_radix(end, 16).ok()?,
    ))
}

fn snapshot(label: &str) -> Result<(), String> {
    let pid = find_warframe_pid().ok_or("Warframe is not running")?;
    let regions = read_game_data(pid)?;
    let total: usize = regions.iter().map(|(_, bytes)| bytes.len()).sum();

    let path = format!("../tmp/riven-{label}.snapshot");
    let mut out = BufWriter::new(File::create(&path).map_err(|e| e.to_string())?);
    for (start, bytes) in &regions {
        out.write_all(&(*start as u64).to_le_bytes()).map_err(|e| e.to_string())?;
        out.write_all(&(bytes.len() as u64).to_le_bytes()).map_err(|e| e.to_string())?;
        out.write_all(bytes).map_err(|e| e.to_string())?;
    }
    println!(
        "wrote {path}: {} regions, {:.1} MiB",
        regions.len(),
        total as f64 / 1048576.0
    );
    Ok(())
}

fn load(label: &str) -> Result<Vec<(usize, Vec<u8>)>, String> {
    let mut file = File::open(format!("../tmp/riven-{label}.snapshot")).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).map_err(|e| e.to_string())?;

    let mut regions = Vec::new();
    let mut offset = 0;
    while offset + 16 <= raw.len() {
        let start = u64::from_le_bytes(raw[offset..offset + 8].try_into().expect("8 bytes")) as usize;
        let len = u64::from_le_bytes(raw[offset + 8..offset + 16].try_into().expect("8 bytes")) as usize;
        offset += 16;
        if offset + len > raw.len() {
            break;
        }
        regions.push((start, raw[offset..offset + len].to_vec()));
        offset += len;
    }
    Ok(regions)
}

fn diff(before: &str, after: &str) -> Result<(), String> {
    let before_regions = load(before)?;
    let after_regions = load(after)?;

    let mut flipped = Vec::new();
    let mut compared = 0usize;
    for (start, old) in &before_regions {
        // Mappings are matched by base address; anything remapped between runs
        // is skipped rather than compared against unrelated bytes.
        let Some((_, new)) = after_regions.iter().find(|(other, _)| other == start) else {
            continue;
        };
        let shared = old.len().min(new.len());
        compared += shared;
        for index in 0..shared {
            if old[index] == 0 && new[index] != 0 {
                flipped.push((start + index, new[index]));
            }
        }
    }

    println!(
        "compared {:.1} MiB, {} bytes went 0 -> non-zero",
        compared as f64 / 1048576.0,
        flipped.len()
    );
    for (address, value) in flipped.iter().take(40) {
        println!("  0x{address:012x} = {value}");
    }
    if flipped.len() > 40 {
        println!("  … {} more", flipped.len() - 40);
    }
    Ok(())
}

/// Scan the game's code for the flag's writer and report where it points.
///
/// The sequence is `xchg dword ptr [rip+disp], r14d` followed by
/// `cmp r14d, 1` — the atomic store the game uses to publish the riven
/// selection state. Everything except the displacement is fixed, so the
/// displacement is what yields the flag address on this launch.
fn sig() -> Result<(), String> {
    const PREFIX: [u8; 3] = [0x44, 0x87, 0x35];
    const SUFFIX: [u8; 4] = [0x41, 0x83, 0xfe, 0x01];

    let pid = find_warframe_pid().ok_or("Warframe is not running")?;
    let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).map_err(|e| e.to_string())?;
    let memory = File::open(format!("/proc/{pid}/mem")).map_err(|e| e.to_string())?;

    let mut found = 0;
    for line in maps.lines() {
        let permissions = line.split_whitespace().nth(1).unwrap_or("");
        if !permissions.starts_with("r-x") {
            continue;
        }
        let Some((start, end)) = parse_range(line) else { continue };
        let mut data = vec![0; end - start];
        let Ok(read) = memory.read_at(&mut data, start as u64) else { continue };
        let data = &data[..read];

        for index in 0..data.len().saturating_sub(11) {
            if data[index..index + 3] != PREFIX {
                continue;
            }
            if data[index + 7..index + 11] != SUFFIX {
                continue;
            }
            let displacement =
                i32::from_le_bytes(data[index + 3..index + 7].try_into().expect("4 bytes")) as i64;
            let flag = (start + index + 7) as i64 + displacement;
            found += 1;
            println!("site=0x{:012x} flag=0x{flag:x}", start + index);
        }
    }
    println!("{found} signature matches");
    Ok(())
}

/// Find the instructions that reference a known address RIP-relative, and print
/// the bytes around each one.
///
/// Once the flag is identified by behaviour, its address is only good for this
/// launch — the game is relocated on every start. A byte signature of the code
/// that touches it is what survives, so this dumps the candidate call sites to
/// build that signature from.
fn refs(target: &str) -> Result<(), String> {
    let target = usize::from_str_radix(target.trim_start_matches("0x"), 16)
        .map_err(|e| format!("{target}: {e}"))?;
    let pid = find_warframe_pid().ok_or("Warframe is not running")?;
    let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).map_err(|e| e.to_string())?;
    let memory = File::open(format!("/proc/{pid}/mem")).map_err(|e| e.to_string())?;

    let mut hits = 0;
    for line in maps.lines() {
        let permissions = line.split_whitespace().nth(1).unwrap_or("");
        if !permissions.starts_with("r-x") {
            continue;
        }
        let Some((start, end)) = parse_range(line) else { continue };
        let mut data = vec![0; end - start];
        let Ok(read) = memory.read_at(&mut data, start as u64) else { continue };
        let data = &data[..read];

        for index in 0..data.len().saturating_sub(8) {
            let displacement =
                i32::from_le_bytes(data[index..index + 4].try_into().expect("4 bytes")) as i64;
            // The displacement is relative to the end of the instruction, and
            // instructions carry up to a few bytes of immediate after it, so
            // every plausible tail length is tried.
            let matches = (0..=8).any(|tail| {
                (start + index + 4 + tail) as i64 + displacement == target as i64
            });
            if !matches {
                continue;
            }
            hits += 1;
            let context_start = index.saturating_sub(8);
            let context: Vec<String> = data[context_start..(index + 8).min(data.len())]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            println!(
                "0x{:012x}  disp_at=+{}  bytes: {}",
                start + context_start,
                index - context_start,
                context.join(" ")
            );
        }
    }
    println!("{hits} references found");
    Ok(())
}

/// Keep only the bytes that track the screen across a full toggle: zero while
/// it is closed, non-zero while it is open, zero again afterwards.
///
/// A single open/closed pair leaves thousands of survivors, because the game
/// writes constantly. Requiring the byte to return to zero is what separates a
/// flag from ordinary churn.
fn narrow(off: &str, on: &str, off_again: &str) -> Result<(), String> {
    let first = load(off)?;
    let open = load(on)?;
    let second = load(off_again)?;

    let mut survivors = Vec::new();
    for (start, closed) in &first {
        let Some((_, opened)) = open.iter().find(|(other, _)| other == start) else { continue };
        let Some((_, reclosed)) = second.iter().find(|(other, _)| other == start) else { continue };
        let shared = closed.len().min(opened.len()).min(reclosed.len());
        for index in 0..shared {
            if closed[index] == 0 && opened[index] != 0 && reclosed[index] == 0 {
                survivors.push((start + index, opened[index]));
            }
        }
    }

    println!("{} bytes follow the screen (0 / non-zero / 0)", survivors.len());
    for (address, value) in survivors.iter().take(40) {
        println!("  0x{address:012x} open-value={value}");
    }
    if survivors.len() > 40 {
        println!("  … {} more", survivors.len() - 40);
    }
    Ok(())
}

/// Read individual bytes by address — for watching a candidate flag across game
/// states without dumping anything.
fn peek(addresses: &[&str]) -> Result<(), String> {
    let pid = find_warframe_pid().ok_or("Warframe is not running")?;
    let memory = File::open(format!("/proc/{pid}/mem")).map_err(|e| e.to_string())?;
    println!("pid={pid}");
    for address in addresses {
        let parsed = usize::from_str_radix(address.trim_start_matches("0x"), 16)
            .map_err(|e| format!("{address}: {e}"))?;
        let mut byte = [0u8; 1];
        match memory.read_at(&mut byte, parsed as u64) {
            Ok(1) => println!("0x{parsed:012x} = {}", byte[0]),
            _ => println!("0x{parsed:012x} = unreadable"),
        }
    }
    Ok(())
}

fn find_warframe_pid() -> Option<u32> {
    std::fs::read_dir("/proc").ok()?.filter_map(Result::ok).find_map(|entry| {
        let pid = entry.file_name().to_str()?.parse().ok()?;
        let command = std::fs::read(entry.path().join("cmdline")).ok()?;
        let command = String::from_utf8_lossy(&command).to_ascii_lowercase();
        (command.contains("warframe.x64.exe") && !command.contains("launcher.exe")).then_some(pid)
    })
}
