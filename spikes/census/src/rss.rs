//! Reading how much memory a process is using.
//!
//! Two ways to ask, one per platform, no dependencies and no unsafe:
//! - Linux: `/proc/<pid>/statm` holds the resident page count;
//! - Windows: `tasklist` knows the memory of any process, so we ask it.
//!
//! When a platform cannot answer, the census says "not measurable here"
//! rather than guessing — a missing number is honest, an invented one is
//! not (Law 3).

/// The resident memory of a process, in bytes.
pub fn resident_bytes_of(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        linux_resident_bytes_of(pid)
    }
    #[cfg(target_os = "windows")]
    {
        windows_resident_bytes_of(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = pid;
        None
    }
}

/// This process's resident memory, in bytes.
pub fn own_resident_bytes() -> Option<u64> {
    resident_bytes_of(std::process::id())
}

/// Linux: the resident pages field of `/proc/<pid>/statm`, times the page size.
#[cfg(target_os = "linux")]
fn linux_resident_bytes_of(pid: u32) -> Option<u64> {
    const PAGE_SIZE_BYTES: u64 = 4096;
    let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(resident_pages * PAGE_SIZE_BYTES)
}

/// Windows: ask `tasklist` for a process's memory usage.
#[cfg(target_os = "windows")]
fn windows_resident_bytes_of(pid: u32) -> Option<u64> {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| line.contains(&format!("\"{pid}\"")))?;
    // "name.exe","1234","Console","1","12,345 K"
    let last = line.rsplit("\",\"").next()?.trim_matches('"');
    let kilobytes: u64 = last
        .replace(',', "")
        .trim()
        .trim_end_matches('K')
        .trim()
        .parse()
        .ok()?;
    Some(kilobytes * 1024)
}

/// The machine's total physical memory, in bytes, when the platform can say.
pub fn total_machine_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        for line in meminfo.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                let kilobytes: u64 = rest.trim().trim_end_matches(" kB").trim().parse().ok()?;
                return Some(kilobytes * 1024);
            }
        }
        None
    }
    #[cfg(target_os = "windows")]
    {
        // The operating system knows; ask it with the tool that is always
        // there. This runs once per census, so the cost does not matter.
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
            ])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        text.trim().parse::<u64>().ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// A one-line description of the machine the census ran on, so every number
/// it prints has provenance (Law 6).
pub fn machine_description() -> String {
    let cores = std::thread::available_parallelism()
        .map(|cores| cores.get())
        .unwrap_or(0);
    let memory = match total_machine_bytes() {
        Some(bytes) => format!("{:.1} GB RAM", bytes as f64 / (1024.0 * 1024.0 * 1024.0)),
        None => "RAM unknown".to_string(),
    };
    format!("{} · {cores} cores · {memory}", std::env::consts::OS)
}
