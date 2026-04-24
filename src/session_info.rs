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

#[derive(Debug)]
pub(crate) struct SessionEntry {
    pub name: String,
    pub age: SessionAge,
    pub is_exited: bool,
}

/// Enumerate zellij sessions from the on-disk session_info dirs, bypassing
/// the ~80ms `zellij ls` subprocess. A session is listed if its directory
/// contains `session-metadata.kdl`. Active vs exited is determined by
/// whether a matching socket dir exists in the runtime dir.
pub(crate) fn list_sessions() -> Vec<SessionEntry> {
    let now = SystemTime::now();
    let alive = live_session_names();
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
            let metadata_path = dir.join("session-metadata.kdl");
            if !metadata_path.exists() {
                continue;
            }
            let is_exited = !alive.contains(&name);
            let age = if is_exited {
                // For exited sessions, directory mtime is unreliable (gets
                // touched by `zellij ls`). Use `creation_time` from the
                // metadata (session uptime in seconds) instead.
                age_from_creation_time(&metadata_path)
                    .unwrap_or_else(|| age_from_mtime(&dir, now))
            } else {
                age_from_mtime(&dir, now)
            };
            out.push(SessionEntry {
                name,
                age,
                is_exited,
            });
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

fn age_from_mtime(dir: &std::path::Path, now: SystemTime) -> SessionAge {
    let modified = fs::metadata(dir)
        .and_then(|m| m.modified())
        .unwrap_or(now);
    SessionAge::from_modified_time(modified, now)
}

/// Parse `creation_time <seconds>` from session-metadata.kdl. This field
/// records the session's total uptime in seconds, which is a stable age
/// indicator for exited sessions whose directory mtime gets clobbered.
fn age_from_creation_time(metadata_path: &std::path::Path) -> Option<SessionAge> {
    let text = fs::read_to_string(metadata_path).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("creation_time ") {
            let secs: u64 = rest.trim().parse().ok()?;
            return Some(SessionAge {
                label: format_age(secs),
                seconds: secs,
            });
        }
    }
    None
}

/// Session names that have a live server (socket dir in the runtime dir).
/// Sessions present in session_info but absent here are EXITED.
fn live_session_names() -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for root in runtime_roots() {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                names.insert(name.to_owned());
            }
        }
    }
    names
}

fn runtime_roots() -> &'static Vec<PathBuf> {
    static ROOTS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    ROOTS.get_or_init(discover_runtime_roots)
}

fn discover_runtime_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let tmp = env::var("TMPDIR")
        .or_else(|_| env::var("TEMP"))
        .unwrap_or_else(|_| "/tmp".into());
    let tmp = PathBuf::from(tmp);

    // Zellij runtime dir: $TMPDIR/zellij-<UID>/contract_version_*/<session>
    let Ok(tmp_entries) = fs::read_dir(&tmp) else {
        return roots;
    };
    for tmp_entry in tmp_entries.flatten() {
        let Some(dname) = tmp_entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !dname.starts_with("zellij-") {
            continue;
        }
        let Ok(entries) = fs::read_dir(tmp_entry.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("contract_version_") && path.is_dir() && !roots.contains(&path)
                {
                    roots.push(path);
                }
            }
        }
    }
    roots
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
