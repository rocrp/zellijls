//! Per-session "last interaction" timestamps via PTY slave mtime.
//!
//! `session-metadata.kdl` is rewritten on every zellij tick, so its mtime
//! is essentially "now" for every live session — useless for ranking
//! activity. The actual interaction signal lives on the slave PTY: the
//! kernel updates its mtime whenever the pane process reads input or
//! writes output. We resolve each pane's `/dev/ttys00X` path from the
//! controlling tty dev_t, stat it, and take the max per session.
//!
//! Cost: one `proc_pidinfo` syscall per pane PID (~10µs each) plus one
//! `stat` per unique tty. Typical total is well under 5ms — small enough
//! to fold into the existing concurrent scope without moving the wall
//! clock. macOS-only; on other platforms callers fall back to metadata
//! mtime.

#![cfg(target_os = "macos")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

use sysinfo::{Pid, System};

/// Compute last-activity time per session from PTY slave mtime.
///
/// `server_by_session`: session name → PID of the `zellij --server …`
/// process owning that session. Only direct children of the server are
/// inspected — those are the pane shells/agents, each on its own pane
/// PTY, and any deeper descendants share the same controlling tty.
pub(crate) fn last_activity_per_session(
    sys: &System,
    server_by_session: &HashMap<String, u32>,
) -> HashMap<String, SystemTime> {
    if server_by_session.is_empty() {
        return HashMap::new();
    }

    // Group children by their server PID in a single sysinfo pass instead
    // of N filtered scans.
    let server_pids: std::collections::HashSet<u32> =
        server_by_session.values().copied().collect();
    let mut children_by_server: HashMap<u32, Vec<u32>> = HashMap::new();
    for proc in sys.processes().values() {
        let Some(parent) = proc.parent() else {
            continue;
        };
        let parent = parent.as_u32();
        if server_pids.contains(&parent) {
            children_by_server
                .entry(parent)
                .or_default()
                .push(proc.pid().as_u32());
        }
    }

    // Cache stat results — multiple panes inside one server run on
    // distinct ttys, but the same tty path may be probed twice if the
    // pane has multiple direct children (e.g. shell + foreground prog).
    let mut tty_mtime_cache: HashMap<u32, Option<SystemTime>> = HashMap::new();

    let mut out = HashMap::new();
    for (session, server_pid) in server_by_session {
        let Some(child_pids) = children_by_server.get(server_pid) else {
            continue;
        };
        let mut latest: Option<SystemTime> = None;
        for &pid in child_pids {
            let Some(dev) = pid_controlling_tdev(pid as i32) else {
                continue;
            };
            let mtime = *tty_mtime_cache
                .entry(dev)
                .or_insert_with(|| stat_tty_mtime(dev));
            if let Some(mt) = mtime {
                latest = Some(latest.map_or(mt, |cur| cur.max(mt)));
            }
        }
        if let Some(mt) = latest {
            out.insert(session.clone(), mt);
        }
    }
    out
}

/// Path of the slave PTY for a given dev_t.
///
/// macOS encodes char device dev_t as `(major << 24) | minor`, and PTY
/// slaves live at `/dev/ttysNNN` where NNN is the minor zero-padded to
/// at least 3 digits. We construct the path directly instead of
/// `readdir`-ing /dev (saves ~1ms cold).
fn tty_path_from_dev(dev: u32) -> String {
    let minor = dev & 0x00ff_ffff;
    format!("/dev/ttys{minor:03}")
}

fn stat_tty_mtime(dev: u32) -> Option<SystemTime> {
    let path = tty_path_from_dev(dev);
    fs::metadata(Path::new(&path)).ok()?.modified().ok()
}

/// Look up the controlling tty's dev_t for a given pid via macOS's
/// `proc_pidinfo(PROC_PIDTBSDINFO)`. Returns None for processes with no
/// controlling tty (e.g. daemons).
fn pid_controlling_tdev(pid: i32) -> Option<u32> {
    use std::mem;

    const PROC_PIDTBSDINFO: i32 = 3;
    const NODEV: u32 = u32::MAX;

    // Mirrors `struct proc_bsdinfo` from `<sys/proc_info.h>`. Only
    // `e_tdev` is read; field offsets must match the kernel layout.
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
            pid,
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
    Some(info.e_tdev)
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

#[cfg(test)]
mod tests {
    use super::tty_path_from_dev;

    #[test]
    fn formats_tty_path_from_dev() {
        // major=16, minor=9 → /dev/ttys009
        assert_eq!(tty_path_from_dev(0x10_00_00_09), "/dev/ttys009");
        // major=16, minor=10 → /dev/ttys010
        assert_eq!(tty_path_from_dev(0x10_00_00_0A), "/dev/ttys010");
        // major=16, minor=1234 → /dev/ttys1234 (no truncation)
        assert_eq!(tty_path_from_dev(0x10_00_04_D2), "/dev/ttys1234");
    }
}
