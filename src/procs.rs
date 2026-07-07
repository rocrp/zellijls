//! Process-tree data source: pane shells, foreground commands, tty ages.
//!
//! Each zellij pane is a direct child of its session's `zellij --server`
//! process (the Pane Shell), running on its own PTY. One cheap per-pid
//! probe (macOS: `proc_pidinfo`; Linux: `/proc/<pid>/stat`) yields every
//! per-pane fact we need:
//!  - controlling tty dev → slave mtime = last interaction time
//!  - tpgid (foreground process group on that tty) → the Pane Command,
//!    matching what zellij's `list-panes` reports as `pane_command`
//!  - start time → creation-order key for binding metadata-KDL titles
//!
//! See docs/adr/0001-panes-from-process-tree-and-metadata-kdl.md.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::SystemTime;

use sysinfo::{Pid, System};

/// Per-pid facts from the OS probe. Only processes with a controlling tty
/// qualify — pane shells always have one.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ShellProbe {
    pub pid: u32,
    /// The shell's own process group.
    pub pgid: u32,
    /// Controlling tty dev_t.
    pub tty_dev: u32,
    /// Foreground process group on the controlling tty.
    pub fg_pgid: u32,
    /// Creation-order key: µs since epoch on macOS, clock ticks since
    /// boot on Linux. Only ordering matters; tie-break by pid.
    pub start_key: u64,
}

/// One pane resolved from the process tree, creation-ordered.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PaneSource {
    /// The Pane Command: foreground process group leader on the pane's
    /// tty, or the shell itself when idle.
    pub fg_pid: u32,
    pub tty_dev: u32,
}

/// Find `zellij --server <runtime>/<session>` processes and map session
/// name → server pid. Requires `cmd` to be loaded on the relevant
/// processes — caller should run a targeted refresh with
/// `ProcessRefreshKind::with_cmd(...)` first.
pub(crate) fn discover_servers(sys: &System, zellij_pids: &[Pid]) -> HashMap<String, u32> {
    let mut out = HashMap::new();
    for pid in zellij_pids {
        let Some(proc) = sys.process(*pid) else {
            continue;
        };
        let cmd = proc.cmd();
        let mut iter = cmd.iter();
        while let Some(arg) = iter.next() {
            if arg == "--server" {
                if let Some(path) = iter.next()
                    && let Some(name) = Path::new(path).file_name().and_then(|n| n.to_str())
                {
                    out.insert(name.to_owned(), pid.as_u32());
                }
                break;
            }
        }
    }
    out
}

/// Group direct children by server pid in a single sysinfo pass.
pub(crate) fn children_by_server(
    sys: &System,
    server_pids: &HashSet<u32>,
) -> HashMap<u32, Vec<u32>> {
    let mut out: HashMap<u32, Vec<u32>> = HashMap::new();
    for proc in sys.processes().values() {
        let Some(parent) = proc.parent() else {
            continue;
        };
        let parent = parent.as_u32();
        if server_pids.contains(&parent) {
            out.entry(parent).or_default().push(proc.pid().as_u32());
        }
    }
    out
}

/// Resolve a server's children into creation-ordered pane sources.
///
/// Multiple children can share one tty (shell + a directly-forked helper);
/// the earliest-started child per tty is the Pane Shell. The Pane Command
/// is the tty's foreground group leader (pid == fg_pgid) when it exists
/// and differs from the shell's own group; otherwise the shell itself.
pub(crate) fn pane_sources(sys: &System, children: &[u32]) -> Vec<PaneSource> {
    let mut probes: Vec<ShellProbe> = children.iter().filter_map(|&pid| probe(pid)).collect();
    probes.sort_by_key(|p| (p.start_key, p.pid));

    let mut seen_ttys = HashSet::new();
    probes
        .into_iter()
        .filter(|p| seen_ttys.insert(p.tty_dev))
        .map(|p| {
            let fg_alive = p.fg_pgid != p.pgid && sys.process(Pid::from_u32(p.fg_pgid)).is_some();
            PaneSource {
                fg_pid: if fg_alive { p.fg_pgid } else { p.pid },
                tty_dev: p.tty_dev,
            }
        })
        .collect()
}

/// mtime of the slave PTY for a given dev_t — the kernel touches it on
/// every read/write through the pane, making it the per-pane "last
/// interaction" signal.
pub(crate) fn tty_mtime(dev: u32) -> Option<SystemTime> {
    fs::metadata(Path::new(&tty_path_from_dev(dev)))
        .ok()?
        .modified()
        .ok()
}

// ---------------------------------------------------------------- macOS --

/// macOS encodes char device dev_t as `(major << 24) | minor`, and PTY
/// slaves live at `/dev/ttysNNN` where NNN is the minor zero-padded to
/// at least 3 digits. Constructed directly instead of `readdir`-ing /dev
/// (saves ~1ms cold).
#[cfg(target_os = "macos")]
fn tty_path_from_dev(dev: u32) -> String {
    let minor = dev & 0x00ff_ffff;
    format!("/dev/ttys{minor:03}")
}

/// Probe one pid via `proc_pidinfo(PROC_PIDTBSDINFO)`. Returns None for
/// processes with no controlling tty (e.g. daemons) or that died.
#[cfg(target_os = "macos")]
pub(crate) fn probe(pid: u32) -> Option<ShellProbe> {
    use std::mem;

    const PROC_PIDTBSDINFO: i32 = 3;
    const NODEV: u32 = u32::MAX;

    // Mirrors `struct proc_bsdinfo` from `<sys/proc_info.h>`. Field
    // offsets must match the kernel layout.
    #[repr(C)]
    struct ProcBsdInfo {
        pbi_flags: u32,
        pbi_status: u32,
        pbi_xstatus: u32,
        pbi_pid: u32,
        pbi_ppid: u32,
        pbi_uid: u32,
        pbi_gid: u32,
        pbi_ruid: u32,
        pbi_rgid: u32,
        pbi_svuid: u32,
        pbi_svgid: u32,
        rfu_1: u32,
        pbi_comm: [libc::c_char; 16],
        pbi_name: [libc::c_char; 32],
        pbi_nfiles: u32,
        pbi_pgid: u32,
        pbi_pjobc: u32,
        e_tdev: u32,
        e_tpgid: u32,
        pbi_nice: i32,
        pbi_start_tvsec: u64,
        pbi_start_tvusec: u64,
    }

    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut libc::c_void,
            buffersize: i32,
        ) -> i32;
    }

    let size = mem::size_of::<ProcBsdInfo>() as i32;
    let mut info: ProcBsdInfo = unsafe { mem::zeroed() };
    let n = unsafe {
        proc_pidinfo(
            pid as i32,
            PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if n != size {
        return None;
    }
    if info.e_tdev == NODEV || info.e_tdev == 0 {
        return None;
    }
    Some(ShellProbe {
        pid,
        pgid: info.pbi_pgid,
        tty_dev: info.e_tdev,
        fg_pgid: info.e_tpgid,
        start_key: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
    })
}

// ---------------------------------------------------------------- Linux --

/// UNIX98 PTY slaves: majors 136–143, pts index = (major-136)*256 + minor
/// (modern kernels put everything on major 136 with extended minors,
/// which the same formula handles).
#[cfg(target_os = "linux")]
fn tty_path_from_dev(dev: u32) -> String {
    let major = (dev >> 8) & 0xfff;
    let minor = (dev & 0xff) | ((dev >> 12) & 0xfff00);
    if (136..=143).contains(&major) {
        format!("/dev/pts/{}", (major - 136) * 256 + minor)
    } else {
        // Not a pts (e.g. a real console); stat will simply fail.
        format!("/dev/tty{minor}")
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn probe(pid: u32) -> Option<ShellProbe> {
    let content = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_proc_stat(pid, &content)
}

/// Parse the fields of `/proc/<pid>/stat` we need. The comm field (2) is
/// parenthesized and may contain spaces — everything is split after the
/// last ')'. 0-indexed from there: 0=state 2=pgrp 4=tty_nr 5=tpgid
/// 19=starttime.
#[allow(dead_code)] // cross-compiled for tests on all platforms
fn parse_proc_stat(pid: u32, content: &str) -> Option<ShellProbe> {
    let rest = &content[content.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let pgid: u32 = fields.get(2)?.parse().ok()?;
    let tty_nr: i64 = fields.get(4)?.parse().ok()?;
    let tpgid: i64 = fields.get(5)?.parse().ok()?;
    let start_key: u64 = fields.get(19)?.parse().ok()?;
    if tty_nr <= 0 {
        return None;
    }
    Some(ShellProbe {
        pid,
        pgid,
        tty_dev: tty_nr as u32,
        fg_pgid: if tpgid > 0 { tpgid as u32 } else { pgid },
        start_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn formats_tty_path_from_dev() {
        // major=16, minor=9 → /dev/ttys009
        assert_eq!(tty_path_from_dev(0x10_00_00_09), "/dev/ttys009");
        // major=16, minor=10 → /dev/ttys010
        assert_eq!(tty_path_from_dev(0x10_00_00_0A), "/dev/ttys010");
        // major=16, minor=1234 → /dev/ttys1234 (no truncation)
        assert_eq!(tty_path_from_dev(0x10_00_04_D2), "/dev/ttys1234");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn formats_pts_path_from_dev() {
        // major=136, minor=3 → /dev/pts/3
        assert_eq!(tty_path_from_dev((136 << 8) | 3), "/dev/pts/3");
        // major=137, minor=4 → /dev/pts/260 (legacy secondary major)
        assert_eq!(tty_path_from_dev((137 << 8) | 4), "/dev/pts/260");
        // extended minor 300 on major 136 → /dev/pts/300
        let dev = (136 << 8) | (300 & 0xff) | ((300 & 0xfff00) << 12);
        assert_eq!(tty_path_from_dev(dev), "/dev/pts/300");
    }

    #[test]
    fn parses_proc_stat_line() {
        // pid 91480 "claude" running on pts/2 (dev 34818 = 136<<8|2),
        // fg group 91480, pgrp 91480, starttime 5021882.
        let line = "91480 (claude) S 91310 91480 91310 34818 91480 4194304 \
                    0 0 0 0 5 3 0 0 20 0 12 0 5021882 1000000 500 hidden";
        let p = parse_proc_stat(91480, line).unwrap();
        assert_eq!(p.pgid, 91480);
        assert_eq!(p.tty_dev, 34818);
        assert_eq!(p.fg_pgid, 91480);
        assert_eq!(p.start_key, 5021882);
    }

    #[test]
    fn proc_stat_comm_with_spaces_and_parens() {
        let line = "42 (tmux: client (v3)) S 1 42 42 34816 42 0 \
                    0 0 0 0 0 0 0 0 20 0 1 0 12345 0 0";
        let p = parse_proc_stat(42, line).unwrap();
        assert_eq!(p.tty_dev, 34816);
        assert_eq!(p.start_key, 12345);
    }

    #[test]
    fn proc_stat_without_tty_is_none() {
        let line = "99 (daemon) S 1 99 99 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 777 0 0";
        assert!(parse_proc_stat(99, line).is_none());
    }
}
