mod age;
mod pick;
mod session_info;

use age::{AgeTier, age_tier, freshest_age_seconds, sort_sessions_for_display};
use session_info::{connected_clients, list_sessions};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use netstat2::{AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, SocketInfo, TcpState};
use serde::Deserialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use unicode_width::UnicodeWidthChar;

// ANSI colors
pub(crate) const GREEN: &str = "\x1b[32m";
pub(crate) const CYAN: &str = "\x1b[36m";
pub(crate) const BRIGHT_CYAN: &str = "\x1b[96m";
pub(crate) const BRIGHT_BLACK: &str = "\x1b[90m";
pub(crate) const YELLOW: &str = "\x1b[33m";
pub(crate) const DIM: &str = "\x1b[2m";
pub(crate) const RESET: &str = "\x1b[0m";
pub(crate) const BOLD: &str = "\x1b[1m";
pub(crate) const UNDERLINE: &str = "\x1b[4m";

const IDLE_SHELLS: &[&str] = &["zsh", "bash", "sh", "fish"];
const AGENT_COMMANDS: &[&str] = &["claude", "codex", "codex-aarch64-apple-darwin"];

fn is_spinner(c: char) -> bool {
    matches!(
        c,
        '⠂' | '⠒'
            | '⠑'
            | '⠊'
            | '⣾'
            | '⣽'
            | '⣻'
            | '⢿'
            | '⡿'
            | '⣟'
            | '⣯'
            | '⣷'
            | '⠈'
            | '⠐'
            | '⠠'
    )
}

// --- Data types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentState {
    Working,
    Waiting,
}

#[derive(Debug)]
pub(crate) struct Pane {
    pub command: String,
    pub cwd: String,
    pub title: String,
}

#[derive(Debug)]
pub(crate) struct Session {
    pub name: String,
    pub age: String,
    pub age_seconds: u64,
    pub is_current: bool,
    pub is_exited: bool,
    pub connected_clients: u32,
    pub panes: Vec<Pane>,
    pub agent_state: Option<AgentState>,
    pub task: String,
}

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

// --- Helpers ---

/// Terminal display width that accounts for VS16 (U+FE0F) making preceding
/// text-presentation-default emoji render as 2-wide.
pub(crate) fn display_width(s: &str) -> usize {
    let mut width = 0;
    let mut prev_char_width = 0usize;
    for c in s.chars() {
        if c == '\u{FE0F}' {
            if prev_char_width < 2 {
                width += 2 - prev_char_width;
            }
            prev_char_width = 0;
            continue;
        }
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        width += cw;
        prev_char_width = cw;
    }
    width
}

pub(crate) fn base_name(cmd: &str) -> &str {
    let binary = cmd.split_whitespace().next().unwrap_or("");
    binary.rsplit('/').next().unwrap_or(binary)
}

pub(crate) fn paint(text: &str, styles: &[&str]) -> String {
    if styles.is_empty() {
        return text.to_string();
    }

    format!("{}{}{RESET}", styles.concat(), text)
}

pub(crate) fn status_color(session: &Session) -> Option<&'static str> {
    if session.is_exited {
        return None;
    }

    Some(match session.agent_state {
        Some(AgentState::Working) => BRIGHT_CYAN,
        Some(AgentState::Waiting) => YELLOW,
        None => CYAN,
    })
}

fn is_shell(cmd: &str) -> bool {
    IDLE_SHELLS.contains(&base_name(cmd))
}

fn is_agent(cmd: &str) -> bool {
    let b = base_name(cmd);
    AGENT_COMMANDS.contains(&b) || b.starts_with("codex-")
}

fn extract_task(title: &str) -> &str {
    let t = title.trim_start_matches(|c: char| is_spinner(c) || c == '✳' || c == ' ');
    t.trim_start()
}

/// Run a subprocess with a hard wall-clock timeout. Returns None if the
/// process fails, exits non-zero, or is killed for exceeding `timeout` (e.g.
/// a wedged zellij server). Robustness matters here — without a timeout a
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

// --- Zellij queries ---

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

fn get_panes(session: &str) -> Vec<Pane> {
    // Normal queries finish in ~50-100ms; anything past 1s is a wedged
    // session server and we'd rather degrade to "empty" than hang the CLI.
    let Some(json_str) = run_cmd(
        "zellij",
        &["-s", session, "action", "list-panes", "--all", "--json"],
        std::time::Duration::from_secs(1),
    ) else {
        return vec![];
    };
    let Ok(panes) = serde_json::from_str::<Vec<ZellijPane>>(&json_str) else {
        return vec![];
    };
    panes
        .into_iter()
        .filter(|p| !p.is_plugin.unwrap_or(false))
        .map(|p| Pane {
            command: p.pane_command.or(p.terminal_command).unwrap_or_default(),
            cwd: p.pane_cwd.unwrap_or_default(),
            title: p.title.unwrap_or_default(),
        })
        .collect()
}

// --- Process inspection (no subprocess!) ---

fn is_agent_process(process: &sysinfo::Process) -> bool {
    let name = process.name().to_string_lossy();
    let base = name.rsplit('/').next().unwrap_or(&name);
    AGENT_COMMANDS.contains(&base) || base.starts_with("codex-")
}

fn build_agent_pid_map(sys: &System) -> HashMap<String, Vec<u32>> {
    let mut map: HashMap<String, Vec<u32>> = HashMap::new();
    for process in sys.processes().values() {
        if !is_agent_process(process) {
            continue;
        }
        if let Some(cwd) = process.cwd() {
            map.entry(cwd.to_string_lossy().into_owned())
                .or_default()
                .push(process.pid().as_u32());
        }
    }
    map
}

fn build_working_pids(
    sys: &System,
    agent_pids: &HashMap<String, Vec<u32>>,
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

// --- Session building ---

fn build_sessions() -> Vec<Session> {
    let meta = get_session_list();
    if meta.is_empty() {
        return vec![];
    }

    let cpu_sample_start = Instant::now();

    // Everything below runs concurrently:
    //  - Phase A: broad sysinfo refresh (CPU only; cwd is expensive on macOS
    //    and we only need it for agent PIDs, refreshed later in Phase B).
    //  - TCP socket table scan for detecting network-active agents.
    //  - Pane queries per session. `zellij action list-panes` drops fields
    //    (e.g. `pane_command`) when ≥5 invocations run concurrently, so we
    //    batch 2-wide — empirically stable across 30+ torture runs.
    const PANE_QUERY_CONCURRENCY: usize = 2;
    let (mut sys, sockets, pane_data) = std::thread::scope(|s| {
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
            let mut out: Vec<(u32, Vec<Pane>)> = Vec::with_capacity(meta.len());
            std::thread::scope(|ps| {
                for batch in meta.chunks(PANE_QUERY_CONCURRENCY) {
                    let handles: Vec<_> = batch
                        .iter()
                        .map(|m| {
                            let name = m.name.clone();
                            let exited = m.is_exited;
                            ps.spawn(move || {
                                if exited {
                                    (0, Vec::new())
                                } else {
                                    (connected_clients(&name), get_panes(&name))
                                }
                            })
                        })
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

    // Agent PIDs identified by process name alone; cwd gets populated in Phase B.
    let agent_pids: Vec<Pid> = sys
        .processes()
        .values()
        .filter(|p| is_agent_process(p))
        .map(|p| p.pid())
        .collect();

    let mut sessions: Vec<Session> = meta
        .iter()
        .zip(pane_data)
        .map(|(m, (clients, panes))| {
            let task = panes
                .iter()
                .find(|p| is_agent(&p.command))
                .map(|p| extract_task(&p.title).to_string())
                .unwrap_or_default();
            Session {
                name: m.name.clone(),
                age: m.age.clone(),
                age_seconds: m.age_seconds,
                is_current: m.is_current,
                is_exited: m.is_exited,
                connected_clients: clients,
                panes,
                agent_state: None,
                task,
            }
        })
        .collect();

    // Ensure ≥ 100ms between CPU samples for sysinfo delta accuracy. Pane
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

    let agent_pid_map = build_agent_pid_map(&sys);
    let working_pids = build_working_pids(&sys, &agent_pid_map, &sockets);

    // Determine agent state — spinner (Claude's own UI signal) takes priority.
    for s in &mut sessions {
        for pane in &s.panes {
            if !is_agent(&pane.command) {
                continue;
            }
            s.agent_state = Some(if pane.title.starts_with(is_spinner) {
                AgentState::Working
            } else if let Some(pids) = agent_pid_map.get(&pane.cwd) {
                if pids.iter().any(|p| working_pids.contains(p)) {
                    AgentState::Working
                } else {
                    AgentState::Waiting
                }
            } else {
                AgentState::Waiting
            });
            break;
        }
    }

    sort_sessions_for_display(&mut sessions);
    sessions
}

// --- Display ---

pub(crate) fn cmd_summary(session: &Session) -> String {
    if session.is_exited {
        return "exited".into();
    }

    let mut commands = Vec::new();
    let mut shell_count = 0u32;

    for pane in &session.panes {
        if pane.command.is_empty() {
            continue;
        }
        if is_shell(&pane.command) {
            shell_count += 1;
            continue;
        }
        let base = base_name(&pane.command);
        if is_agent(&pane.command) {
            let ind = match session.agent_state {
                Some(AgentState::Working) => "\u{1f6a7}",
                _ => "\u{1f4a4}",
            };
            commands.push(format!("{base} {ind}"));
        } else {
            commands.push(base.to_string());
        }
    }

    if commands.is_empty() {
        return if shell_count > 0 {
            "idle".into()
        } else {
            "empty".into()
        };
    }

    let mut result = commands.join(" + ");
    if shell_count > 0 {
        result.push_str(&format!(" +{shell_count}sh"));
    }
    result
}

fn print_table(sessions: &[Session]) {
    let freshest_age = freshest_age_seconds(sessions);
    let max_name = sessions
        .iter()
        .map(|s| s.name.len())
        .max()
        .unwrap_or(7)
        .max(7);
    let cmd_texts: Vec<String> = sessions.iter().map(cmd_summary).collect();
    let max_cmd = cmd_texts
        .iter()
        .map(|t| display_width(t))
        .max()
        .unwrap_or(7)
        .max(7);
    let max_age = sessions
        .iter()
        .map(|s| display_width(&s.age))
        .max()
        .unwrap_or(3)
        .max(3);

    println!(
        "{DIM}{:<max_name$}  {:<max_cmd$}  {:<max_age$}  TASK{RESET}",
        "SESSION", "STATUS", "AGE"
    );
    println!(
        "{DIM}{}{RESET}",
        "\u{2501}".repeat(max_name + max_cmd + max_age + 10)
    );

    for (s, cmd_text) in sessions.iter().zip(cmd_texts.iter()) {
        let tier = age_tier(s, freshest_age);
        let cmd_w = display_width(cmd_text);
        let cmd_pad = " ".repeat(max_cmd.saturating_sub(cmd_w));

        let mut name_styles = Vec::new();
        if s.is_current {
            name_styles.extend([GREEN, BOLD]);
        } else {
            match tier {
                AgeTier::Freshest => name_styles.extend([BRIGHT_CYAN, BOLD]),
                AgeTier::Recent => {}
                AgeTier::Stale => name_styles.push(DIM),
                AgeTier::Old | AgeTier::Exited => name_styles.push(BRIGHT_BLACK),
            }
        }
        if s.connected_clients > 0 {
            name_styles.push(UNDERLINE);
        }
        let name_display = paint(&s.name, &name_styles);
        let name_pad = " ".repeat(max_name.saturating_sub(s.name.len()));

        let mut cmd_styles = Vec::new();
        if matches!(tier, AgeTier::Freshest) {
            cmd_styles.push(BOLD);
        } else if matches!(tier, AgeTier::Stale) {
            cmd_styles.push(DIM);
        } else if matches!(tier, AgeTier::Old | AgeTier::Exited) {
            cmd_styles.push(DIM);
        }
        if !matches!(tier, AgeTier::Old | AgeTier::Exited) {
            if let Some(color) = status_color(s) {
                cmd_styles.push(color);
            }
        }
        let cmd_display = paint(cmd_text, &cmd_styles);

        let age_text = format!("{:<max_age$}", s.age);
        let age_display = match tier {
            AgeTier::Freshest => paint(&age_text, &[GREEN, BOLD]),
            AgeTier::Recent => paint(&age_text, &[GREEN]),
            AgeTier::Stale => paint(&age_text, &[DIM]),
            AgeTier::Old | AgeTier::Exited => paint(&age_text, &[BRIGHT_BLACK]),
        };

        let task_display = if s.task.is_empty() {
            String::new()
        } else {
            let task = if s.task.len() > 50 {
                format!("{}\u{2026}", &s.task[..49])
            } else {
                s.task.clone()
            };
            if matches!(tier, AgeTier::Old | AgeTier::Exited) {
                paint(&task, &[BRIGHT_BLACK])
            } else if s.agent_state == Some(AgentState::Waiting) || matches!(tier, AgeTier::Stale) {
                paint(&task, &[DIM])
            } else {
                task
            }
        };

        println!("{name_display}{name_pad}  {cmd_display}{cmd_pad}  {age_display}  {task_display}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let subcmd = args.get(1).map(|s| s.as_str());

    let sessions = build_sessions();

    match subcmd {
        Some("pick" | "-i") => {
            if sessions.is_empty() {
                // No sessions — launch a fresh zellij
                use std::os::unix::process::CommandExt;
                let err = std::process::Command::new("zellij").exec();
                eprintln!("Failed to launch zellij: {err}");
                std::process::exit(1);
            }
            if let Some(name) = pick::run(&sessions) {
                use std::os::unix::process::CommandExt;
                let err = std::process::Command::new("zellij")
                    .args(["attach", &name])
                    .exec();
                eprintln!("Failed to attach: {err}");
                std::process::exit(1);
            }
        }
        _ => {
            if sessions.is_empty() {
                println!("No zellij sessions.");
                return;
            }
            print_table(&sessions);
        }
    }
}
