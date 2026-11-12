//! Cheap resource numbers for `system.stats`: this process, the host, and the
//! process tree under each pane's shell.
//!
//! Every reader returns `None` for a number the OS cannot give; the caller
//! leaves that field out. Per-pane figures come from ONE process-table scan
//! shared by all panes (`tree_usages`), not one scan per pane.
#![allow(dead_code)]

use std::collections::HashMap;

static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Called first thing in `main` so `uptime_s` measures from process start.
pub(crate) fn mark_started() {
    STARTED.get_or_init(std::time::Instant::now);
}

/// Seconds since `mark_started` (or since the first call, if it never ran).
pub(crate) fn uptime_s() -> u64 {
    STARTED
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs()
}

/// Memory and cumulative CPU time of one process or a set of processes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Usage {
    pub rss_bytes: Option<u64>,
    /// User + system CPU time, nanoseconds, since the process started.
    pub cpu_ns: Option<u64>,
}

impl Usage {
    fn add(&mut self, other: Usage) {
        self.rss_bytes = sum_opt(self.rss_bytes, other.rss_bytes);
        self.cpu_ns = sum_opt(self.cpu_ns, other.cpu_ns);
    }
}

fn sum_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (x, None) | (None, x) => x,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SelfUsage {
    pub usage: Usage,
    pub peak_rss_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct HostUsage {
    pub mem_total_bytes: Option<u64>,
    pub mem_available_bytes: Option<u64>,
    pub load1: Option<f64>,
    pub cpus: Option<u32>,
}

/// One row of a process-table scan. `sid` is the POSIX session id, `None` on
/// Windows (the tree there is the parent/child chain only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcRow {
    pub pid: u32,
    pub ppid: u32,
    pub sid: Option<i64>,
}

/// CPU use in percent of one core over `wall_ns`, from two cumulative CPU
/// readings. Never negative: a tree that lost a child can have a smaller total.
pub(crate) fn cpu_percent(prev_cpu_ns: u64, now_cpu_ns: u64, wall_ns: u64) -> f64 {
    if wall_ns == 0 {
        return 0.0;
    }
    let delta = now_cpu_ns.saturating_sub(prev_cpu_ns) as f64;
    (delta / wall_ns as f64 * 1000.0).round() / 10.0
}

/// Pids belonging to the tree under `root`: its descendants by parent link,
/// plus (Unix) every process in the root's session when the root leads it,
/// which catches orphans that were re-parented to init.
pub(crate) fn tree_members(rows: &[ProcRow], root: u32) -> Vec<u32> {
    let Some(root_row) = rows.iter().find(|row| row.pid == root) else {
        return Vec::new();
    };
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for row in rows {
        if row.pid != row.ppid {
            children.entry(row.ppid).or_default().push(row.pid);
        }
    }
    let mut members = vec![root];
    let mut seen = std::collections::HashSet::from([root]);
    let mut next = 0;
    while next < members.len() {
        let pid = members[next];
        next += 1;
        for child in children.get(&pid).into_iter().flatten() {
            if seen.insert(*child) {
                members.push(*child);
            }
        }
    }
    if let Some(sid) = root_row.sid.filter(|sid| *sid == i64::from(root)) {
        for row in rows {
            if row.sid == Some(sid) && seen.insert(row.pid) {
                members.push(row.pid);
            }
        }
    }
    members
}

/// Resident memory and CPU time of the process tree under each root, from one
/// process-table scan. The result lines up with `roots`; a root that is gone
/// gets an empty `Usage`.
pub(crate) fn tree_usages(roots: &[u32]) -> Vec<Usage> {
    if roots.is_empty() {
        return Vec::new();
    }
    let rows = scan_processes();
    let mut cache: HashMap<u32, Usage> = HashMap::new();
    roots
        .iter()
        .map(|root| {
            let mut total = Usage::default();
            for pid in tree_members(&rows, *root) {
                let usage = *cache.entry(pid).or_insert_with(|| process_usage(pid));
                total.add(usage);
            }
            total
        })
        .collect()
}

// ---------------------------------------------------------------- Linux parse

/// Fields of `/proc/<pid>/stat` that matter here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LinuxStat {
    pub ppid: u32,
    pub sid: i64,
    /// utime + stime, clock ticks.
    pub cpu_ticks: u64,
    pub rss_pages: u64,
}

/// Parses `/proc/<pid>/stat`. The command name sits in parentheses and may
/// itself contain spaces and parentheses, so fields count from the LAST `)`.
pub(crate) fn parse_linux_stat(stat: &str) -> Option<LinuxStat> {
    let rest = stat.get(stat.rfind(')')? + 1..)?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    // f[0] is field 3 (state); field n is f[n - 3].
    Some(LinuxStat {
        ppid: f.get(1)?.parse().ok()?,
        sid: f.get(3)?.parse().ok()?,
        cpu_ticks: f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?,
        rss_pages: f.get(21)?.parse::<i64>().ok()?.max(0) as u64,
    })
}

/// `MemTotal` / `MemAvailable` from `/proc/meminfo` text, in bytes.
pub(crate) fn parse_meminfo(text: &str) -> (Option<u64>, Option<u64>) {
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
            .map(|kib| kib * 1024)
    };
    (field("MemTotal"), field("MemAvailable"))
}

// ---------------------------------------------------------------------- Linux

#[cfg(target_os = "linux")]
fn clock_ticks_ns() -> u64 {
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz > 0 {
        1_000_000_000 / hz as u64
    } else {
        10_000_000
    }
}

#[cfg(target_os = "linux")]
fn page_size() -> u64 {
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if size > 0 {
        size as u64
    } else {
        4096
    }
}

#[cfg(target_os = "linux")]
fn read_stat(pid: u32) -> Option<LinuxStat> {
    parse_linux_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

#[cfg(target_os = "linux")]
fn scan_processes() -> Vec<ProcRow> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if let Some(stat) = read_stat(pid) {
            rows.push(ProcRow {
                pid,
                ppid: stat.ppid,
                sid: Some(stat.sid),
            });
        }
    }
    rows
}

#[cfg(target_os = "linux")]
fn process_usage(pid: u32) -> Usage {
    match read_stat(pid) {
        Some(stat) => Usage {
            rss_bytes: Some(stat.rss_pages * page_size()),
            cpu_ns: Some(stat.cpu_ticks * clock_ticks_ns()),
        },
        None => Usage::default(),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn host_usage() -> HostUsage {
    let (mem_total_bytes, mem_available_bytes) = std::fs::read_to_string("/proc/meminfo")
        .map(|text| parse_meminfo(&text))
        .unwrap_or((None, None));
    HostUsage {
        mem_total_bytes,
        mem_available_bytes,
        load1: unix_load1(),
        cpus: cpu_count(),
    }
}

// ---------------------------------------------------------------------- macOS

#[cfg(target_os = "macos")]
fn scan_processes() -> Vec<ProcRow> {
    let mut pids: Vec<libc::pid_t> = vec![0; 4096];
    loop {
        let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        let got = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        if got <= 0 {
            return Vec::new();
        }
        if (got as usize) < pids.len() {
            pids.truncate(got as usize);
            break;
        }
        let doubled = pids.len() * 2;
        pids.resize(doubled, 0);
    }
    pids.into_iter()
        .filter(|pid| *pid > 0)
        .filter_map(|pid| {
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            let got = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    (&mut info as *mut libc::proc_bsdinfo).cast(),
                    size,
                )
            };
            if got != size {
                return None;
            }
            let sid = unsafe { libc::getsid(pid) };
            Some(ProcRow {
                pid: pid as u32,
                ppid: info.pbi_ppid,
                sid: (sid > 0).then_some(i64::from(sid)),
            })
        })
        .collect()
}

/// Mach time ticks to nanoseconds (1:1 on Intel, 125:3 on Apple Silicon).
#[cfg(target_os = "macos")]
fn mach_ticks_to_ns(ticks: u64) -> u64 {
    static RATIO: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();
    let (numer, denom) = *RATIO.get_or_init(|| {
        let mut info = libc::mach_timebase_info { numer: 0, denom: 0 };
        if unsafe { libc::mach_timebase_info(&mut info) } == 0 && info.denom != 0 {
            (u64::from(info.numer), u64::from(info.denom))
        } else {
            (1, 1)
        }
    });
    (u128::from(ticks) * u128::from(numer) / u128::from(denom)) as u64
}

#[cfg(target_os = "macos")]
fn process_usage(pid: u32) -> Usage {
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            (&mut info as *mut libc::proc_taskinfo).cast(),
            size,
        )
    };
    if got != size {
        return Usage::default();
    }
    Usage {
        rss_bytes: Some(info.pti_resident_size),
        cpu_ns: Some(mach_ticks_to_ns(
            info.pti_total_user.saturating_add(info.pti_total_system),
        )),
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn host_usage() -> HostUsage {
    HostUsage {
        mem_total_bytes: macos_mem_total(),
        mem_available_bytes: macos_mem_available(),
        load1: unix_load1(),
        cpus: cpu_count(),
    }
}

#[cfg(target_os = "macos")]
fn macos_mem_total() -> Option<u64> {
    let mut value: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let rc = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&mut value as *mut u64).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && value > 0).then_some(value)
}

/// Free + inactive + purgeable-style reclaimable pages: what the OS can hand
/// out without swapping, the closest macOS has to Linux's MemAvailable.
#[cfg(target_os = "macos")]
fn macos_mem_available() -> Option<u64> {
    let mut stats: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = (std::mem::size_of::<libc::vm_statistics64>()
        / std::mem::size_of::<libc::integer_t>())
        as libc::mach_msg_type_number_t;
    #[allow(deprecated)]
    let host = unsafe { libc::mach_host_self() };
    let rc = unsafe {
        libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            (&mut stats as *mut libc::vm_statistics64).cast(),
            &mut count,
        )
    };
    if rc != 0 {
        return None;
    }
    let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok()?;
    let pages = u64::from(stats.free_count) + u64::from(stats.inactive_count);
    Some(pages * page)
}

// ----------------------------------------------------------- Unix, both flavors

#[cfg(unix)]
fn unix_load1() -> Option<f64> {
    let mut load = [0f64; 1];
    let got = unsafe { libc::getloadavg(load.as_mut_ptr(), 1) };
    (got >= 1).then(|| (load[0] * 100.0).round() / 100.0)
}

#[cfg(unix)]
pub(crate) fn self_usage() -> SelfUsage {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    let rusage_ok = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } == 0;
    let timeval_ns = |tv: libc::timeval| {
        (tv.tv_sec.max(0) as u64) * 1_000_000_000 + (tv.tv_usec.max(0) as u64) * 1_000
    };
    let cpu_ns = rusage_ok.then(|| timeval_ns(ru.ru_utime) + timeval_ns(ru.ru_stime));
    // ru_maxrss is bytes on macOS and KiB on Linux.
    let peak = rusage_ok.then(|| {
        let max = ru.ru_maxrss.max(0) as u64;
        if cfg!(target_os = "macos") {
            max
        } else {
            max * 1024
        }
    });
    let rss = process_usage(std::process::id()).rss_bytes;
    SelfUsage {
        usage: Usage {
            rss_bytes: rss,
            cpu_ns,
        },
        peak_rss_bytes: peak,
    }
}

// -------------------------------------------------------------------- Windows

#[cfg(windows)]
mod win {
    use super::{HostUsage, ProcRow, SelfUsage, Usage};
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    pub(super) fn scan_processes() -> Vec<ProcRow> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Vec::new();
        }
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..unsafe { std::mem::zeroed() }
        };
        let mut rows = Vec::new();
        let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
        while ok {
            rows.push(ProcRow {
                pid: entry.th32ProcessID,
                ppid: entry.th32ParentProcessID,
                sid: None,
            });
            ok = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
        }
        unsafe { CloseHandle(snapshot) };
        rows
    }

    fn filetime_100ns(ft: FILETIME) -> u64 {
        (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
    }

    fn handle_usage(handle: HANDLE) -> (Usage, Option<u64>) {
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let mem_ok = unsafe { GetProcessMemoryInfo(handle, &mut counters, counters.cb) } != 0;

        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        let cpu_ok =
            unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }
                != 0;
        (
            Usage {
                rss_bytes: mem_ok.then_some(counters.WorkingSetSize as u64),
                cpu_ns: cpu_ok
                    .then(|| (filetime_100ns(kernel) + filetime_100ns(user)).saturating_mul(100)),
            },
            mem_ok.then_some(counters.PeakWorkingSetSize as u64),
        )
    }

    pub(super) fn process_usage(pid: u32) -> Usage {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return Usage::default();
        }
        let (usage, _) = handle_usage(handle);
        unsafe { CloseHandle(handle) };
        usage
    }

    pub(super) fn self_usage() -> SelfUsage {
        let (usage, peak) = handle_usage(unsafe { GetCurrentProcess() });
        SelfUsage {
            usage,
            peak_rss_bytes: peak,
        }
    }

    pub(super) fn host_usage() -> HostUsage {
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
        HostUsage {
            mem_total_bytes: ok.then_some(status.ullTotalPhys),
            mem_available_bytes: ok.then_some(status.ullAvailPhys),
            // Windows has no load average.
            load1: None,
            cpus: super::cpu_count(),
        }
    }
}

#[cfg(windows)]
fn scan_processes() -> Vec<ProcRow> {
    win::scan_processes()
}
#[cfg(windows)]
fn process_usage(pid: u32) -> Usage {
    win::process_usage(pid)
}
#[cfg(windows)]
pub(crate) fn self_usage() -> SelfUsage {
    win::self_usage()
}
#[cfg(windows)]
pub(crate) fn host_usage() -> HostUsage {
    win::host_usage()
}

// ---------------------------------------------------------------------- Others

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn scan_processes() -> Vec<ProcRow> {
    Vec::new()
}
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn process_usage(_pid: u32) -> Usage {
    Usage::default()
}
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(crate) fn self_usage() -> SelfUsage {
    SelfUsage::default()
}
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(crate) fn host_usage() -> HostUsage {
    HostUsage {
        cpus: cpu_count(),
        ..HostUsage::default()
    }
}

fn cpu_count() -> Option<u32> {
    std::thread::available_parallelism()
        .ok()
        .map(|n| n.get() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, ppid: u32, sid: Option<i64>) -> ProcRow {
        ProcRow { pid, ppid, sid }
    }

    #[test]
    fn cpu_percent_is_a_share_of_one_core_and_never_negative() {
        assert_eq!(cpu_percent(0, 500_000_000, 1_000_000_000), 50.0);
        assert_eq!(cpu_percent(0, 2_000_000_000, 1_000_000_000), 200.0);
        assert_eq!(cpu_percent(900, 100, 1_000_000_000), 0.0);
        assert_eq!(cpu_percent(0, 100, 0), 0.0);
    }

    #[test]
    fn tree_follows_parent_links_and_ignores_other_trees() {
        let rows = [
            row(10, 1, None),
            row(11, 10, None),
            row(12, 11, None),
            row(20, 1, None),
            row(21, 20, None),
        ];
        let mut members = tree_members(&rows, 10);
        members.sort_unstable();
        assert_eq!(members, vec![10, 11, 12]);
        assert!(tree_members(&rows, 999).is_empty());
    }

    #[test]
    fn session_leader_tree_includes_reparented_orphans() {
        let rows = [
            row(10, 1, Some(10)),
            row(11, 10, Some(10)),
            row(30, 1, Some(10)), // orphan, re-parented to init, same session
            row(40, 1, Some(40)),
        ];
        let mut members = tree_members(&rows, 10);
        members.sort_unstable();
        assert_eq!(members, vec![10, 11, 30]);
        // A root that does not lead its session only gets descendants.
        let rows = [row(11, 10, Some(10)), row(30, 1, Some(10))];
        assert_eq!(tree_members(&rows, 11), vec![11]);
    }

    #[test]
    fn tree_survives_a_parent_cycle() {
        let rows = [row(1, 1, None), row(2, 3, None), row(3, 2, None)];
        let mut members = tree_members(&rows, 2);
        members.sort_unstable();
        assert_eq!(members, vec![2, 3]);
    }

    #[test]
    fn linux_stat_parses_names_with_spaces_and_parens() {
        let line = "4242 (my (odd) name) S 100 4242 4242 34816 4242 4194560 \
                    1000 0 0 0 7 5 0 0 20 0 1 0 12345 100000000 2500 18446744073709551615 \
                    1 1 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0";
        let stat = parse_linux_stat(line).unwrap();
        assert_eq!(stat.ppid, 100);
        assert_eq!(stat.sid, 4242);
        assert_eq!(stat.cpu_ticks, 12);
        assert_eq!(stat.rss_pages, 2500);
        assert!(parse_linux_stat("garbage").is_none());
    }

    #[test]
    fn meminfo_reads_total_and_available_and_tolerates_gaps() {
        let text = "MemTotal:       16384000 kB\nMemFree:  1 kB\nMemAvailable:    8192000 kB\n";
        assert_eq!(
            parse_meminfo(text),
            (Some(16_384_000 * 1024), Some(8_192_000 * 1024))
        );
        assert_eq!(parse_meminfo("MemTotal: 5 kB\n"), (Some(5 * 1024), None));
        assert_eq!(parse_meminfo(""), (None, None));
    }

    #[test]
    fn usage_sum_skips_missing_numbers() {
        let mut total = Usage::default();
        total.add(Usage {
            rss_bytes: Some(10),
            cpu_ns: None,
        });
        total.add(Usage {
            rss_bytes: None,
            cpu_ns: Some(7),
        });
        total.add(Usage {
            rss_bytes: Some(5),
            cpu_ns: Some(1),
        });
        assert_eq!(total.rss_bytes, Some(15));
        assert_eq!(total.cpu_ns, Some(8));
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn this_process_reports_real_numbers() {
        let me = self_usage();
        assert!(me.usage.rss_bytes.unwrap_or(0) > 0, "{me:?}");
        let host = host_usage();
        assert!(host.mem_total_bytes.unwrap_or(0) > 0, "{host:?}");
        assert!(host.cpus.unwrap_or(0) >= 1);
        let trees = tree_usages(&[std::process::id()]);
        assert!(trees[0].rss_bytes.unwrap_or(0) > 0, "{trees:?}");
    }
}
