use std::collections::{HashMap, HashSet};
use std::time::{Instant, SystemTime};

use netstat2::{AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, SocketInfo, TcpState};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::age::{format_age, sort_sessions_for_display};
use crate::agent::{base_name, is_agent_base, is_agent_command, is_spinner, task_from_agent_pane};
use crate::model::{AgentState, Pane, Session};
use crate::procs::{self, PaneSource};
use crate::session_info::{
    MetaPane, list_sessions, parse_connected_clients, parse_panes, read_metadata,
};

#[derive(Debug)]
struct SessionMeta {
    name: String,
    age: String,
    age_seconds: u64,
    is_current: bool,
    is_exited: bool,
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

/// Bind metadata-KDL titles to pane sources (Creation-Order Binding).
///
/// `kdl` must be terminal (non-plugin), non-exited panes sorted by id;
/// `source_is_agent` flags each pane source in creation order. When the
/// counts match, pane ids and process start times share creation order, so
/// a positional zip is exact. On mismatch (pane closing mid-read) degrade
/// to agent-only association: spinner/✳-prefixed titles are agent-set, so
/// hand them out to agent panes in order and leave the rest untitled —
/// non-agent titles are never rendered anyway.
fn bind_titles(kdl: &[MetaPane], source_is_agent: &[bool]) -> Vec<String> {
    if kdl.len() == source_is_agent.len() {
        return kdl.iter().map(|p| p.title.clone()).collect();
    }

    let mut agent_titles = kdl
        .iter()
        .filter(|p| p.title.starts_with(|c: char| is_spinner(c) || c == '✳'));
    source_is_agent
        .iter()
        .map(|&is_agent| {
            if is_agent {
                agent_titles
                    .next()
                    .map(|p| p.title.clone())
                    .unwrap_or_default()
            } else {
                String::new()
            }
        })
        .collect()
}

/// Terminal, live panes from the metadata KDL in creation (id) order.
fn terminal_panes(metadata: Option<&String>) -> Vec<MetaPane> {
    let mut panes: Vec<MetaPane> = metadata
        .map(|text| parse_panes(text))
        .unwrap_or_default()
        .into_iter()
        .filter(|p| !p.is_plugin && !p.exited)
        .collect();
    panes.sort_by_key(|p| p.id);
    panes
}

fn command_string(process: &sysinfo::Process) -> String {
    let cmd = process.cmd();
    if cmd.is_empty() {
        process.name().to_string_lossy().into_owned()
    } else {
        cmd.iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn build_working_pids(
    sys: &System,
    agent_pids: &HashSet<u32>,
    sockets: &[SocketInfo],
) -> HashSet<u32> {
    if agent_pids.is_empty() {
        return HashSet::new();
    }

    let mut working = HashSet::new();

    for &pid in agent_pids {
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
            if agent_pids.contains(&pid)
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

fn update_agent_states(
    sessions: &mut [Session],
    fg_pids: &[Vec<u32>],
    working_pids: &HashSet<u32>,
) {
    for (session, pids) in sessions.iter_mut().zip(fg_pids) {
        for (pane, &fg_pid) in session.panes.iter_mut().zip(pids) {
            if is_agent_command(&pane.command) {
                pane.agent_state = Some(
                    if pane.title.starts_with(is_spinner) || working_pids.contains(&fg_pid) {
                        AgentState::Working
                    } else {
                        AgentState::Waiting
                    },
                );
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

    // Phase A: broad sysinfo refresh (CPU sample start; names come along
    // for free) concurrent with the TCP socket table scan used to detect
    // network-active agents. No subprocesses anywhere — see ADR-0001.
    let (mut sys, sockets) = std::thread::scope(|s| {
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
        (sys_h.join().unwrap(), sock_h.join().unwrap())
    });

    // Metadata KDL per live session: connected clients + pane titles from
    // one read each.
    let metadata: Vec<Option<String>> = meta
        .iter()
        .map(|m| {
            if m.is_exited {
                None
            } else {
                read_metadata(&m.name)
            }
        })
        .collect();

    // Zellij PIDs → targeted cmd refresh (cheap: single sysctl per PID,
    // ~5-10 PIDs typical) → session name → server pid → pane sources.
    let zellij_pids: Vec<Pid> = sys
        .processes()
        .values()
        .filter(|p| base_name(&p.name().to_string_lossy()) == "zellij")
        .map(|p| p.pid())
        .collect();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&zellij_pids),
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    let server_by_session = procs::discover_servers(&sys, &zellij_pids);
    let server_pids: HashSet<u32> = server_by_session.values().copied().collect();
    let children = procs::children_by_server(&sys, &server_pids);

    let sources: Vec<Vec<PaneSource>> = meta
        .iter()
        .map(|m| {
            server_by_session
                .get(&m.name)
                .and_then(|pid| children.get(pid))
                .map(|kids| procs::pane_sources(&sys, kids))
                .unwrap_or_default()
        })
        .collect();

    // Ensure >= 100ms between CPU samples for sysinfo delta accuracy.
    let min_delay = std::time::Duration::from_millis(100);
    let elapsed = cpu_sample_start.elapsed();
    if elapsed < min_delay {
        std::thread::sleep(min_delay - elapsed);
    }

    // Phase B: targeted refresh of the foreground pids — CPU delta for
    // working detection, cwd + full cmd for the pane rows.
    let fg_pids: Vec<Pid> = sources
        .iter()
        .flatten()
        .map(|s| Pid::from_u32(s.fg_pid))
        .collect();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&fg_pids),
        true,
        ProcessRefreshKind::nothing()
            .with_cpu()
            .with_cwd(UpdateKind::Always)
            .with_cmd(UpdateKind::Always),
    );

    let mut fg_by_session: Vec<Vec<u32>> = Vec::with_capacity(meta.len());
    let mut sessions: Vec<Session> = meta
        .iter()
        .zip(&metadata)
        .zip(&sources)
        .map(|((m, metadata), sources)| {
            let panes: Vec<Pane> = {
                let is_agent: Vec<bool> = sources
                    .iter()
                    .map(|s| {
                        sys.process(Pid::from_u32(s.fg_pid))
                            .is_some_and(|p| is_agent_base(base_name(&p.name().to_string_lossy())))
                    })
                    .collect();
                let titles = bind_titles(&terminal_panes(metadata.as_ref()), &is_agent);
                sources
                    .iter()
                    .zip(titles)
                    .map(|(source, title)| {
                        let process = sys.process(Pid::from_u32(source.fg_pid));
                        Pane {
                            command: process.map(command_string).unwrap_or_default(),
                            cwd: process
                                .and_then(|p| p.cwd())
                                .map(|c| c.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            title,
                            agent_state: None,
                        }
                    })
                    .collect()
            };
            fg_by_session.push(sources.iter().map(|s| s.fg_pid).collect());

            let task = panes
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
                connected_clients: metadata
                    .as_ref()
                    .map(|text| parse_connected_clients(text))
                    .unwrap_or(0),
                panes,
                agent_state: None,
                task,
            }
        })
        .collect();

    // Last-activity per session from PTY slave mtime: the kernel touches
    // the slave on pane I/O, unlike the metadata KDL whose mtime is
    // uselessly fresh (rewritten every zellij tick).
    {
        let mut tty_mtime_cache: HashMap<u32, Option<SystemTime>> = HashMap::new();
        let now = SystemTime::now();
        for (session, sources) in sessions.iter_mut().zip(&sources) {
            let latest = sources
                .iter()
                .filter_map(|s| {
                    *tty_mtime_cache
                        .entry(s.tty_dev)
                        .or_insert_with(|| procs::tty_mtime(s.tty_dev))
                })
                .max();
            if let Some(mtime) = latest {
                let secs = now.duration_since(mtime).unwrap_or_default().as_secs();
                session.age_seconds = secs;
                session.age = format_age(secs);
            }
        }
    }

    let agent_pids: HashSet<u32> = sessions
        .iter()
        .zip(&fg_by_session)
        .flat_map(|(session, pids)| {
            session
                .panes
                .iter()
                .zip(pids)
                .filter(|(pane, _)| is_agent_command(&pane.command))
                .map(|(_, &pid)| pid)
        })
        .collect();
    let working_pids = build_working_pids(&sys, &agent_pids, &sockets);
    update_agent_states(&mut sessions, &fg_by_session, &working_pids);

    sort_sessions_for_display(&mut sessions);
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_pane(id: u64, title: &str) -> MetaPane {
        MetaPane {
            id,
            title: title.into(),
            is_plugin: false,
            exited: false,
        }
    }

    #[test]
    fn bind_titles_zips_when_counts_match() {
        let kdl = [
            meta_pane(0, "✳ Fix the bug"),
            meta_pane(1, "voii"),
            meta_pane(4, "make beta"),
        ];
        assert_eq!(
            bind_titles(&kdl, &[true, true, false]),
            vec!["✳ Fix the bug", "voii", "make beta"]
        );
    }

    #[test]
    fn bind_titles_falls_back_to_agent_only_on_mismatch() {
        // One pane closed between the KDL read and the process scan: three
        // titles, two processes. Spinner titles go to agent panes in order;
        // the shell pane stays untitled.
        let kdl = [
            meta_pane(0, "⠐ Refactor collect"),
            meta_pane(1, "~/w/rccc"),
            meta_pane(2, "✳ Write tests"),
        ];
        assert_eq!(
            bind_titles(&kdl, &[false, true]),
            vec!["", "⠐ Refactor collect"]
        );
    }

    #[test]
    fn bind_titles_mismatch_without_agents_yields_untitled() {
        let kdl = [meta_pane(0, "~/Downloads")];
        assert_eq!(bind_titles(&kdl, &[false, false]), vec!["", ""]);
    }
}
