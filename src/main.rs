use std::collections::{HashMap, HashSet};
use std::process::Command;

use netstat2::{AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState};
use serde::Deserialize;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use unicode_width::UnicodeWidthStr;

// ANSI colors
const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";
const BRIGHT_CYAN: &str = "\x1b[96m";
const YELLOW: &str = "\x1b[33m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";

const IDLE_SHELLS: &[&str] = &["zsh", "bash", "sh", "fish"];
const AGENT_COMMANDS: &[&str] = &["claude", "codex", "codex-aarch64-apple-darwin"];

fn is_spinner(c: char) -> bool {
    matches!(c,
        '⠂' | '⠒' | '⠑' | '⠊' | '✳' | '⣾' | '⣽' | '⣻' | '⢿' |
        '⡿' | '⣟' | '⣯' | '⣷' | '⠈' | '⠐' | '⠠'
    )
}

// --- Data types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentState {
    Working,
    Waiting,
}

#[derive(Debug)]
struct Pane {
    command: String,
    cwd: String,
    title: String,
}

#[derive(Debug)]
struct Session {
    name: String,
    age: String,
    is_current: bool,
    is_exited: bool,
    panes: Vec<Pane>,
    agent_state: Option<AgentState>,
    task: String,
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

fn base_name(cmd: &str) -> &str {
    let binary = cmd.split_whitespace().next().unwrap_or("");
    binary.rsplit('/').next().unwrap_or(binary)
}

fn is_shell(cmd: &str) -> bool {
    IDLE_SHELLS.contains(&base_name(cmd))
}

fn is_agent(cmd: &str) -> bool {
    let b = base_name(cmd);
    AGENT_COMMANDS.contains(&b) || b.starts_with("codex-")
}

fn extract_task(title: &str) -> &str {
    let t = title.trim_start_matches(|c: char| is_spinner(c) || c == ' ');
    t.trim_start()
}

fn run_cmd(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

// --- Zellij queries ---

fn get_session_list() -> Vec<(String, String, bool, bool)> {
    let Some(out) = run_cmd("zellij", &["ls", "--no-formatting"]) else {
        return vec![];
    };
    out.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let name = line.split_whitespace().next()?.to_string();
            let is_current = line.contains("(current)");
            let is_exited = line.contains("EXITED");

            // Parse "Created 14h 18m 41s ago" -> "14h"
            let age = line
                .find("Created ")
                .and_then(|i| {
                    let rest = &line[i + 8..];
                    rest.find(" ago").map(|end| {
                        rest[..end]
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string()
                    })
                })
                .unwrap_or_default();

            Some((name, age, is_current, is_exited))
        })
        .collect()
}

fn get_panes(session: &str) -> Vec<Pane> {
    let Some(json_str) = run_cmd(
        "zellij",
        &["-s", session, "action", "list-panes", "--all", "--json"],
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

fn build_agent_pid_map(sys: &System) -> HashMap<String, Vec<u32>> {
    let mut map: HashMap<String, Vec<u32>> = HashMap::new();
    for process in sys.processes().values() {
        let name = process.name().to_string_lossy();
        let base = name.rsplit('/').next().unwrap_or(&name);
        if !AGENT_COMMANDS.contains(&base) && !base.starts_with("codex-") {
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

fn build_working_pids(agent_pids: &HashMap<String, Vec<u32>>) -> HashSet<u32> {
    let all_pids: HashSet<u32> = agent_pids.values().flatten().copied().collect();
    if all_pids.is_empty() {
        return HashSet::new();
    }

    let mut working = HashSet::new();
    let af = AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6;
    let proto = ProtocolFlags::TCP;

    if let Ok(sockets) = netstat2::get_sockets_info(af, proto) {
        for si in sockets {
            if let ProtocolSocketInfo::Tcp(ref tcp) = si.protocol_socket_info {
                if tcp.state != TcpState::Established || tcp.remote_addr.is_loopback() {
                    continue;
                }
                for &pid in &si.associated_pids {
                    if all_pids.contains(&pid) {
                        working.insert(pid);
                    }
                }
            }
        }
    }
    working
}

// --- Display ---

fn cmd_summary(session: &Session) -> String {
    if session.is_exited {
        return "(exited)".into();
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
                Some(AgentState::Working) => "🏗️",
                _ => "⏳",
            };
            commands.push(format!("{base} {ind}"));
        } else {
            commands.push(base.to_string());
        }
    }

    if commands.is_empty() {
        return if shell_count > 0 {
            "(idle)".into()
        } else {
            "(empty)".into()
        };
    }

    let mut result = commands.join(" + ");
    if shell_count > 0 {
        result.push_str(&format!(" +{shell_count}"));
    }
    result
}

fn main() {
    let meta = get_session_list();
    if meta.is_empty() {
        println!("No zellij sessions.");
        return;
    }

    // Process info: single in-process call replaces N subprocess lsof calls
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always),
    );
    let agent_pid_map = build_agent_pid_map(&sys);
    // TCP check: single call replaces N subprocess lsof calls
    let working_pids = build_working_pids(&agent_pid_map);

    // Pane queries must be sequential (zellij race condition drops fields)
    let mut sessions: Vec<Session> = Vec::with_capacity(meta.len());
    for (name, age, is_current, is_exited) in &meta {
        let mut s = Session {
            name: name.clone(),
            age: age.clone(),
            is_current: *is_current,
            is_exited: *is_exited,
            panes: vec![],
            agent_state: None,
            task: String::new(),
        };

        if *is_exited {
            sessions.push(s);
            continue;
        }

        s.panes = get_panes(name);

        for pane in &s.panes {
            if !is_agent(&pane.command) {
                continue;
            }
            s.task = extract_task(&pane.title).to_string();
            s.agent_state = Some(
                if let Some(pids) = agent_pid_map.get(&pane.cwd) {
                    if pids.iter().any(|p| working_pids.contains(p)) {
                        AgentState::Working
                    } else {
                        AgentState::Waiting
                    }
                } else {
                    // Fallback: title spinner
                    if pane.title.starts_with(is_spinner) {
                        AgentState::Working
                    } else {
                        AgentState::Waiting
                    }
                },
            );
            break;
        }

        sessions.push(s);
    }

    // Render
    let max_name = sessions.iter().map(|s| s.name.len()).max().unwrap_or(7).max(7);
    let cmd_texts: Vec<String> = sessions.iter().map(cmd_summary).collect();
    let max_cmd = cmd_texts
        .iter()
        .map(|t| UnicodeWidthStr::width(t.as_str()))
        .max()
        .unwrap_or(7)
        .max(7);
    let max_age = sessions.iter().map(|s| s.age.len()).max().unwrap_or(3).max(3);

    println!(
        "  {:<max_name$}  {:<max_cmd$}  {:>max_age$}  TASK",
        "SESSION", "COMMAND", "AGE"
    );
    println!("{}", "━".repeat(max_name + max_cmd + max_age + 12));

    for (s, cmd_text) in sessions.iter().zip(cmd_texts.iter()) {
        let cmd_w = UnicodeWidthStr::width(cmd_text.as_str());
        let cmd_pad = " ".repeat(max_cmd.saturating_sub(cmd_w));

        let (dot, name_display) = if s.is_current {
            (format!("{GREEN}●{RESET}"), format!("{BOLD}{}{RESET}", s.name))
        } else if s.is_exited {
            (format!("{DIM}✕{RESET}"), format!("{DIM}{}{RESET}", s.name))
        } else {
            (format!("{DIM}○{RESET}"), s.name.clone())
        };
        let name_pad = " ".repeat(max_name.saturating_sub(s.name.len()));

        let cmd_display = match cmd_text.as_str() {
            "(idle)" | "(empty)" | "(exited)" => format!("{DIM}{cmd_text}{RESET}"),
            _ if s.agent_state == Some(AgentState::Working) => {
                format!("{BRIGHT_CYAN}{cmd_text}{RESET}")
            }
            _ if s.agent_state == Some(AgentState::Waiting) => {
                format!("{YELLOW}{cmd_text}{RESET}")
            }
            _ => format!("{CYAN}{cmd_text}{RESET}"),
        };

        let age_display = format!("{DIM}{:>max_age$}{RESET}", s.age);

        let task_display = if s.task.is_empty() {
            String::new()
        } else {
            let task = if s.task.len() > 50 {
                format!("{}…", &s.task[..49])
            } else {
                s.task.clone()
            };
            if s.agent_state == Some(AgentState::Waiting) {
                format!("{DIM}{task}{RESET}")
            } else {
                task
            }
        };

        println!("{dot} {name_display}{name_pad}  {cmd_display}{cmd_pad}  {age_display}  {task_display}");
    }
}
