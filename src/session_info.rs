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
///
/// Age is the mtime of `session-metadata.kdl`: zellij rewrites this file
/// while the session is alive, so for active sessions it tracks last
/// activity, and for exited sessions it tracks the last write before the
/// server died. The parent dir mtime doesn't move on inner-file rewrites,
/// so it can't be used here.
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
            let metadata_path = entry.path().join("session-metadata.kdl");
            if !metadata_path.exists() {
                continue;
            }
            let is_exited = !alive.contains(&name);
            out.push(SessionEntry {
                name,
                age: age_from_mtime(&metadata_path, now),
                is_exited,
            });
        }
    }
    out
}

/// Raw text of a session's metadata KDL. One read serves every extractor
/// (`parse_connected_clients`, `parse_panes`).
pub(crate) fn read_metadata(session_name: &str) -> Option<String> {
    let dir = session_info_dir(session_name)?;
    fs::read_to_string(dir.join("session-metadata.kdl")).ok()
}

pub(crate) fn parse_connected_clients(text: &str) -> u32 {
    for line in text.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("connected_clients ") {
            return rest.trim().parse().unwrap_or(0);
        }
    }
    0
}

/// One pane record from the metadata KDL `panes { }` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetaPane {
    pub id: u64,
    pub title: String,
    pub is_plugin: bool,
    pub exited: bool,
}

/// Extract pane records from the top-level `panes { }` block, line-based
/// like `parse_connected_clients` — the four fields we need don't justify
/// a KDL parser dependency.
pub(crate) fn parse_panes(text: &str) -> Vec<MetaPane> {
    let mut out = Vec::new();
    let mut in_panes = false;
    let mut depth = 0u32;
    let mut current: Option<MetaPane> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if !in_panes {
            if trimmed == "panes {" && !line.starts_with(char::is_whitespace) {
                in_panes = true;
                depth = 1;
            }
            continue;
        }

        if trimmed == "pane {" {
            depth += 1;
            current = Some(MetaPane {
                id: 0,
                title: String::new(),
                is_plugin: false,
                exited: false,
            });
        } else if trimmed == "}" {
            depth -= 1;
            match depth {
                1 => out.extend(current.take()),
                0 => break,
                _ => {}
            }
        } else if let Some(pane) = current.as_mut() {
            if let Some(v) = trimmed.strip_prefix("id ") {
                pane.id = v.trim().parse().unwrap_or(0);
            } else if let Some(v) = trimmed.strip_prefix("is_plugin ") {
                pane.is_plugin = v.trim() == "true";
            } else if let Some(v) = trimmed.strip_prefix("exited ") {
                pane.exited = v.trim() == "true";
            } else if let Some(v) = trimmed.strip_prefix("title ") {
                pane.title = unquote_kdl(v.trim());
            }
        }
    }
    out
}

/// Strip surrounding quotes and undo the common KDL string escapes.
fn unquote_kdl(v: &str) -> String {
    let inner = v
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(v);
    if !inner.contains('\\') {
        return inner.to_string();
    }
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn session_info_dir(session_name: &str) -> Option<PathBuf> {
    session_info_roots()
        .iter()
        .map(|root| root.join(session_name))
        .find(|path| path.is_dir())
}

fn age_from_mtime(path: &std::path::Path, now: SystemTime) -> SessionAge {
    let modified = fs::metadata(path).and_then(|m| m.modified()).unwrap_or(now);
    SessionAge::from_modified_time(modified, now)
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
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && name.starts_with("contract_version_")
                && path.is_dir()
                && !roots.contains(&path)
            {
                roots.push(path);
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
    use super::{MetaPane, SessionAge, parse_connected_clients, parse_panes};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn parses_panes_block() {
        let text = r#"name "voii"
tabs {
    tab {
        position 0
        name "Tab #1"
    }
}
panes {
    pane {
        id 0
        is_plugin false
        title "✳ Debug push notification certificate issue"
        exited false
        tab_position 0
    }
    pane {
        id 0
        is_plugin true
        title "tab-bar"
        exited false
    }
    pane {
        id 4
        is_plugin false
        title "say \"hi\" \\ done"
        exited true
    }
}
connected_clients 1
pane_history {
    client {
        id 1
        history {
            pane_id type="terminal" 0
        }
    }
}
"#;
        assert_eq!(
            parse_panes(text),
            vec![
                MetaPane {
                    id: 0,
                    title: "✳ Debug push notification certificate issue".into(),
                    is_plugin: false,
                    exited: false,
                },
                MetaPane {
                    id: 0,
                    title: "tab-bar".into(),
                    is_plugin: true,
                    exited: false,
                },
                MetaPane {
                    id: 4,
                    title: "say \"hi\" \\ done".into(),
                    is_plugin: false,
                    exited: true,
                },
            ]
        );
    }

    #[test]
    fn parse_panes_without_block_is_empty() {
        assert!(parse_panes("name \"x\"\nconnected_clients 0\n").is_empty());
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
                label: "1h".to_string(),
                seconds: 3_723,
            }
        );
    }
}
