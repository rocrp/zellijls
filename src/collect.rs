use std::collections::{HashMap, HashSet};
use std::time::{Instant, SystemTime};

use netstat2::{AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, SocketInfo, TcpState};
use serde::Deserialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::age::{format_age, sort_sessions_for_display};
use crate::agent::{base_name, is_agent_base, is_agent_command, is_spinner, task_from_agent_pane};
use crate::model::{AgentState, Pane, Session};
use crate::session_info::{connected_clients, list_sessions};

#[derive(Debug)]
struct SessionMeta {
    name: String,
    age: String,
    age_seconds: u64,
    is_current: bool,
    is_exited: bool,
}

#[derive(Deserialize)]
struct ZellijPane {
    is_plugin: Option<bool>,
    pane_command: Option<String>,
    terminal_command: Option<String>,
    pane_cwd: Option<String>,
    title: Option<String>,
}

#[derive(Debug)]
struct PaneQuery {
    panes: Vec<Pane>,
    corrupt: bool,
}

type AgentPidKey = (String, String);
type AgentPidMap = HashMap<AgentPidKey, Vec<u32>>;

/// Run a subprocess with a hard wall-clock timeout. Returns None if the
/// process fails, exits non-zero, or is killed for exceeding `timeout` (e.g.
/// a wedged zellij server). Robustness matters here: without a timeout a
/// single stuck session server would hang the whole CLI indefinitely.
fn run_cmd(cmd: &str, args: &[&str], timeout: std::time::Duration) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });

    let result = match rx.recv_timeout(timeout) {
        Ok(buf) => Some(buf),
        Err(_) => {
            let _ = child.kill();
            None
        }
    };
    let _ = child.wait();
    let _ = reader.join();

    let buf = result?;
    Some(String::from_utf8_lossy(&buf).trim().to_string())
}

fn get_session_list() -> Vec<SessionMeta> {
    let current = std::env::var("ZELLIJ_SESSION_NAME").ok();
    list_sessions()
        .into_iter()
        .map(|entry| {
            let is_current = current.as_deref() == Some(entry.name.as_str());
            SessionMeta {
                name: entry.name,
                age: entry.age.label,
                age_seconds: entry.age.seconds,
                is_current,
                is_exited: entry.is_exited,
            }
        })
        .collect()
}

fn get_panes(session: &str) -> PaneQuery {
    // Normal queries finish in ~50-100ms; anything past 1s is a wedged
    // session server and we'd rather degrade to "empty" than hang the CLI.
    let Some(json_str) = run_cmd(
        "zellij",
        &["-s", session, "action", "list-panes", "--all", "--json"],
        std::time::Duration::from_secs(1),
    ) else {
        return PaneQuery {
            panes: Vec::new(),
            corrupt: false,
        };
    };
    let Ok(panes) = serde_json::from_str::<Vec<ZellijPane>>(&json_str) else {
        return PaneQuery {
            panes: Vec::new(),
            corrupt: false,
        };
    };

    let corrupt = panes.iter().any(|p| {
        !p.is_plugin.unwrap_or(false) && p.pane_command.is_none() && p.terminal_command.is_none()
    });
    let panes = panes
        .into_iter()
        .filter(|p| !p.is_plugin.unwrap_or(false))
        .map(|p| Pane {
            command: p.pane_command.or(p.terminal_command).unwrap_or_default(),
            cwd: p.pane_cwd.unwrap_or_default(),
            title: p.title.unwrap_or_default(),
            agent_state: None,
        })
        .collect();

    PaneQuery { panes, corrupt }
}

fn query_session(session: &SessionMeta) -> (u32, PaneQuery) {
    if session.is_exited {
        return (
            0,
            PaneQuery {
                panes: Vec::new(),
                corrupt: false,
            },
        );
    }

    (connected_clients(&session.name), get_panes(&session.name))
}

fn is_agent_process(process: &sysinfo::Process) -> bool {
    let name = process.name().to_string_lossy();
    is_agent_base(base_name(&name))
}

fn agent_pid_key(process: &sysinfo::Process) -> Option<AgentPidKey> {
    let name = process.name().to_string_lossy();
    let base = base_name(&name);
    if !is_agent_base(base) {
        return None;
    }
    let cwd = process.cwd()?;
    Some((cwd.to_string_lossy().into_owned(), base.to_string()))
}

fn build_agent_pid_map(sys: &System) -> AgentPidMap {
    let mut map: AgentPidMap = HashMap::new();
    for process in sys.processes().values() {
        let Some(key) = agent_pid_key(process) else {
            continue;
        };
        map.entry(key).or_default().push(process.pid().as_u32());
    }
    map
}

fn build_working_pids(
    sys: &System,
    agent_pids: &AgentPidMap,
    sockets: &[SocketInfo],
) -> HashSet<u32> {
    let all_pids: HashSet<u32> = agent_pids.values().flatten().copied().collect();
    if all_pids.is_empty() {
        return HashSet::new();
    }

    let mut working = HashSet::new();

    for &pid in &all_pids {
        if let Some(process) = sys.process(Pid::from_u32(pid))
            && process.cpu_usage() > 3.0
        {
            working.insert(pid);
        }
    }

    for si in sockets {
        let ProtocolSocketInfo::Tcp(ref tcp) = si.protocol_socket_info else {
            continue;
        };
        if tcp.state != TcpState::Established || tcp.remote_addr.is_loopback() {
            continue;
        }
        for &pid in &si.associated_pids {
            if all_pids.contains(&pid)
                && !working.contains(&pid)
                && let Some(process) = sys.process(Pid::from_u32(pid))
                && process.cpu_usage() > 0.5
            {
                working.insert(pid);
            }
        }
    }
    working
}

fn pane_agent_state(
    pane: &Pane,
    agent_pid_map: &AgentPidMap,
    working_pids: &HashSet<u32>,
) -> AgentState {
    if pane.title.starts_with(is_spinner) {
        return AgentState::Working;
    }

    let key = (pane.cwd.clone(), base_name(&pane.command).to_string());
    if let Some(pids) = agent_pid_map.get(&key) {
        if pids.iter().any(|p| working_pids.contains(p)) {
            AgentState::Working
        } else {
            AgentState::Waiting
        }
    } else {
        AgentState::Waiting
    }
}

fn update_agent_states(
    sessions: &mut [Session],
    agent_pid_map: &AgentPidMap,
    working_pids: &HashSet<u32>,
) {
    for session in sessions {
        for pane in &mut session.panes {
            if is_agent_command(&pane.command) {
                pane.agent_state = Some(pane_agent_state(pane, agent_pid_map, working_pids));
            }
        }

        session.agent_state = if session
            .panes
            .iter()
            .any(|p| p.agent_state == Some(AgentState::Working))
        {
            Some(AgentState::Working)
        } else if session.panes.iter().any(|p| p.agent_state.is_some()) {
            Some(AgentState::Waiting)
        } else {
            None
        };
    }
}

pub(crate) fn build_sessions() -> Vec<Session> {
    let meta = get_session_list();
    if meta.is_empty() {
        return vec![];
    }

    let cpu_sample_start = Instant::now();

    // Everything below runs concurrently:
    //  - Phase A: broad sysinfo refresh (CPU only; cwd is expensive on macOS
    //    and we only need it for agent PIDs, refreshed later in Phase B).
    //  - TCP socket table scan for detecting network-active agents.
    //  - Pane queries per session. zellij 0.44.3 can drop `pane_command` from
    //    non-plugin panes when list-panes calls overlap, so the fast path keeps
    //    2-wide batches and then strictly re-queries corrupt responses.
    const PANE_QUERY_CONCURRENCY: usize = 2;
    let (mut sys, sockets, mut pane_data) = std::thread::scope(|s| {
        let sys_h = s.spawn(|| {
            let mut sys = System::new();
            sys.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing().with_cpu(),
            );
            sys
        });
        let sock_h = s.spawn(|| {
            netstat2::get_sockets_info(
                AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6,
                ProtocolFlags::TCP,
            )
            .unwrap_or_default()
        });
        let pane_h = s.spawn(|| {
            let mut out: Vec<(u32, PaneQuery)> = Vec::with_capacity(meta.len());
            std::thread::scope(|ps| {
                for batch in meta.chunks(PANE_QUERY_CONCURRENCY) {
                    let handles: Vec<_> = batch
                        .iter()
                        .map(|m| ps.spawn(move || query_session(m)))
                        .collect();
                    for h in handles {
                        out.push(h.join().unwrap());
                    }
                }
            });
            out
        });
        (
            sys_h.join().unwrap(),
            sock_h.join().unwrap(),
            pane_h.join().unwrap(),
        )
    });

    for (session, (_, query)) in meta.iter().zip(&mut pane_data) {
        if !session.is_exited && query.corrupt {
            *query = get_panes(&session.name);
        }
    }

    // Agent PIDs identified by process name alone; cwd gets populated in Phase B.
    let agent_pids: Vec<Pid> = sys
        .processes()
        .values()
        .filter(|p| is_agent_process(p))
        .map(|p| p.pid())
        .collect();

    // Zellij PIDs (any role: server/client/etc.) — cmd line gets populated
    // in Phase B so we can pick out the `--server` ones and map them back
    // to session names for TTY-mtime activity probing.
    #[cfg(target_os = "macos")]
    let zellij_pids: Vec<Pid> = sys
        .processes()
        .values()
        .filter(|p| {
            let name = p.name().to_string_lossy();
            base_name(&name) == "zellij"
        })
        .map(|p| p.pid())
        .collect();

    let mut sessions: Vec<Session> = meta
        .iter()
        .zip(pane_data)
        .map(|(m, (clients, query))| {
            let task = query
                .panes
                .iter()
                .filter(|p| is_agent_command(&p.command))
                .find_map(|p| task_from_agent_pane(p, &m.name))
                .unwrap_or_default();
            Session {
                name: m.name.clone(),
                age: m.age.clone(),
                age_seconds: m.age_seconds,
                is_current: m.is_current,
                is_exited: m.is_exited,
                connected_clients: clients,
                panes: query.panes,
                agent_state: None,
                task,
            }
        })
        .collect();

    // Ensure >= 100ms between CPU samples for sysinfo delta accuracy. Pane
    // queries normally cover this on their own; we only sleep the shortfall.
    let min_delay = std::time::Duration::from_millis(100);
    let elapsed = cpu_sample_start.elapsed();
    if elapsed < min_delay {
        std::thread::sleep(min_delay - elapsed);
    }

    // Phase B: targeted refresh — only agent PIDs, this time with cwd.
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&agent_pids),
        true,
        ProcessRefreshKind::nothing()
            .with_cpu()
            .with_cwd(UpdateKind::Always),
    );

    // Targeted refresh for zellij PIDs to pull `cmd` (cheap on macOS:
    // single sysctl per PID, ~5-10 PIDs typical). Then derive per-session
    // last-activity timestamps from PTY slave mtime and overwrite the
    // metadata-mtime age, which is uselessly fresh for live sessions
    // because zellij rewrites the metadata on every tick.
    #[cfg(target_os = "macos")]
    {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&zellij_pids),
            true,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
        );
        let server_by_session = crate::tty_age::discover_servers(&sys, &zellij_pids);
        let activity = crate::tty_age::last_activity_per_session(&sys, &server_by_session);
        let now = SystemTime::now();
        for session in &mut sessions {
            if session.is_exited {
                continue;
            }
            let Some(mtime) = activity.get(&session.name) else {
                continue;
            };
            let secs = now.duration_since(*mtime).unwrap_or_default().as_secs();
            session.age_seconds = secs;
            session.age = format_age(secs);
        }
    }

    let agent_pid_map = build_agent_pid_map(&sys);
    let working_pids = build_working_pids(&sys, &agent_pid_map, &sockets);
    update_agent_states(&mut sessions, &agent_pid_map, &working_pids);

    sort_sessions_for_display(&mut sessions);
    sessions
}
