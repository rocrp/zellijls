use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use crate::age::{format_age, parse_age_seconds};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionAge {
    pub label: String,
    pub seconds: u64,
}

pub(crate) fn session_age(session_name: &str, created_age: &str) -> SessionAge {
    updated_session_age(session_name).unwrap_or_else(|| created_session_age(created_age))
}

pub(crate) fn connected_clients(session_name: &str) -> u32 {
    let Some(dir) = session_info_dir(session_name) else {
        return 0;
    };
    let Ok(text) = fs::read_to_string(dir.join("session-metadata.kdl")) else {
        return 0;
    };
    parse_connected_clients(&text)
}

fn parse_connected_clients(text: &str) -> u32 {
    for line in text.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("connected_clients ") {
            return rest.trim().parse().unwrap_or(0);
        }
    }
    0
}

fn updated_session_age(session_name: &str) -> Option<SessionAge> {
    let session_info_dir = session_info_dir(session_name)?;
    let modified = fs::metadata(session_info_dir).ok()?.modified().ok()?;
    Some(SessionAge::from_modified_time(modified, SystemTime::now()))
}

fn created_session_age(created_age: &str) -> SessionAge {
    let seconds = parse_age_seconds(created_age)
        .unwrap_or_else(|| panic!("unsupported zellij age format: {created_age}"));
    SessionAge {
        label: created_age.to_string(),
        seconds,
    }
}

fn session_info_dir(session_name: &str) -> Option<PathBuf> {
    session_info_roots()
        .iter()
        .map(|root| root.join(session_name))
        .find(|path| path.is_dir())
}

fn session_info_roots() -> &'static Vec<PathBuf> {
    static ROOTS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    ROOTS.get_or_init(discover_session_info_roots)
}

fn discover_session_info_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    for cache_dir in cache_dirs() {
        let Ok(entries) = fs::read_dir(cache_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.starts_with("contract_version_") {
                continue;
            }
            let session_info = path.join("session_info");
            if session_info.is_dir() && !roots.contains(&session_info) {
                roots.push(session_info);
            }
        }
    }

    roots
}

fn cache_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(cache_dir) = zellij_cache_dir() {
        dirs.push(cache_dir);
    }

    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        let macos_cache_dir = home.join("Library/Caches/org.Zellij-Contributors.Zellij");
        if !dirs.contains(&macos_cache_dir) {
            dirs.push(macos_cache_dir);
        }

        let xdg_cache_dir = home.join(".cache/zellij");
        if !dirs.contains(&xdg_cache_dir) {
            dirs.push(xdg_cache_dir);
        }
    }

    dirs
}

fn zellij_cache_dir() -> Option<PathBuf> {
    let output = Command::new("zellij")
        .args(["setup", "--check"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    parse_cache_dir(&String::from_utf8_lossy(&output.stdout))
}

fn parse_cache_dir(output: &str) -> Option<PathBuf> {
    output
        .lines()
        .find_map(|line| line.strip_prefix("[CACHE DIR]: "))
        .map(|path| PathBuf::from(path.trim_matches('"')))
}

impl SessionAge {
    fn from_modified_time(modified: SystemTime, now: SystemTime) -> Self {
        let elapsed = now
            .duration_since(modified)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        Self {
            label: format_age(elapsed),
            seconds: elapsed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_cache_dir, parse_connected_clients, SessionAge};
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn parses_cache_dir_from_setup_output() {
        let output = r#"
[Version]: "0.44.1"
[CACHE DIR]: /Users/test/Library/Caches/org.Zellij-Contributors.Zellij
[DATA DIR]: "/Users/test/Library/Application Support/org.Zellij-Contributors.Zellij"
"#;

        assert_eq!(
            parse_cache_dir(output),
            Some(PathBuf::from(
                "/Users/test/Library/Caches/org.Zellij-Contributors.Zellij"
            ))
        );
    }

    #[test]
    fn parses_connected_clients_from_metadata() {
        let attached = r#"name "aimd"
tabs {
    tab {
        position 0
        other_focused_clients 2
    }
}
connected_clients 1
"#;
        assert_eq!(parse_connected_clients(attached), 1);

        let detached = "name \"x\"\nconnected_clients 0\n";
        assert_eq!(parse_connected_clients(detached), 0);

        assert_eq!(parse_connected_clients("name \"x\"\n"), 0);
    }

    #[test]
    fn builds_updated_age_from_modified_time() {
        let modified = UNIX_EPOCH + Duration::from_secs(100);
        let now = UNIX_EPOCH + Duration::from_secs(3_823);

        assert_eq!(
            SessionAge::from_modified_time(modified, now),
            SessionAge {
                label: "1h 2m 3s".to_string(),
                seconds: 3_723,
            }
        );
    }
}
