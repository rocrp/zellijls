use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use crate::age::format_age;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionAge {
    pub label: String,
    pub seconds: u64,
}

/// Enumerate active zellij sessions from the on-disk session_info dirs,
/// bypassing the ~80ms `zellij ls` subprocess. A session is considered active
/// if its directory contains `session-metadata.kdl`. The directory's mtime is
/// a good proxy for "last active" (zellij rewrites the metadata file every
/// tick so that file's mtime is always ~now).
pub(crate) fn list_active_sessions() -> Vec<(String, SessionAge)> {
    let now = SystemTime::now();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();

    for root in session_info_roots() {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !seen.insert(name.clone()) {
                continue;
            }
            let dir = entry.path();
            if !dir.join("session-metadata.kdl").exists() {
                continue;
            }
            let Ok(modified) = fs::metadata(&dir).and_then(|m| m.modified()) else {
                continue;
            };
            out.push((name, SessionAge::from_modified_time(modified, now)));
        }
    }
    out
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
    let Some(home) = env::var_os("HOME") else {
        return Vec::new();
    };
    let home = PathBuf::from(home);
    vec![
        home.join("Library/Caches/org.Zellij-Contributors.Zellij"),
        home.join(".cache/zellij"),
    ]
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
    use super::{parse_connected_clients, SessionAge};
    use std::time::{Duration, UNIX_EPOCH};

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
                label: "1h".to_string(),
                seconds: 3_723,
            }
        );
    }
}
