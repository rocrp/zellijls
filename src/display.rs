use unicode_width::UnicodeWidthChar;

use crate::age::{AgeTier, age_tier, freshest_age_seconds};
use crate::agent::{base_name, is_agent_command, is_shell_command};
use crate::model::{AgentState, Session};

// ANSI colors
pub(crate) const GREEN: &str = "\x1b[32m";
pub(crate) const CYAN: &str = "\x1b[36m";
pub(crate) const BRIGHT_CYAN: &str = "\x1b[96m";
pub(crate) const BRIGHT_BLACK: &str = "\x1b[90m";
pub(crate) const YELLOW: &str = "\x1b[33m";
pub(crate) const DIM: &str = "\x1b[2m";
pub(crate) const RESET: &str = "\x1b[0m";
pub(crate) const BOLD: &str = "\x1b[1m";
pub(crate) const GREY_BG: &str = "\x1b[100m";
pub(crate) const UNDERLINE: &str = "\x1b[4m";
pub(crate) const STRIKETHROUGH: &str = "\x1b[9m";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowStyles {
    pub name: Vec<&'static str>,
    pub status: Vec<&'static str>,
    pub age: Vec<&'static str>,
    pub task: Vec<&'static str>,
}

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

/// Truncate `s` so its display width does not exceed `max_width`. When
/// truncation occurs, the last cell is replaced with `...`. Returns `""` if
/// `max_width < 2`.
pub(crate) fn truncate_to_width(s: &str, max_width: usize) -> String {
    if max_width < 2 {
        return String::new();
    }
    if display_width(s) <= max_width {
        return s.to_string();
    }
    // Reserve 1 cell for '...'.
    let budget = max_width - 1;
    let mut out = String::new();
    let mut width = 0usize;
    let mut prev_char_width = 0usize;
    for c in s.chars() {
        if c == '\u{FE0F}' {
            if prev_char_width < 2 {
                let extra = 2 - prev_char_width;
                if width + extra > budget {
                    break;
                }
                width += extra;
            }
            out.push(c);
            prev_char_width = 0;
            continue;
        }
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if width + cw > budget {
            break;
        }
        out.push(c);
        width += cw;
        prev_char_width = cw;
    }
    out.push('\u{2026}');
    out
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

pub(crate) fn row_styles(session: &Session, tier: AgeTier) -> RowStyles {
    let mut name = Vec::new();
    if session.is_current {
        name.extend([GREEN, BOLD]);
    } else {
        match tier {
            AgeTier::Freshest => name.extend([BRIGHT_CYAN, BOLD]),
            AgeTier::Recent => {}
            AgeTier::Stale => name.push(DIM),
            AgeTier::Old => name.push(BRIGHT_BLACK),
            AgeTier::Exited => name.extend([BRIGHT_BLACK, STRIKETHROUGH]),
        }
    }
    if session.connected_clients > 0 {
        name.push(UNDERLINE);
    }

    let mut status = Vec::new();
    if matches!(tier, AgeTier::Freshest) {
        status.push(BOLD);
    } else if matches!(tier, AgeTier::Stale | AgeTier::Old | AgeTier::Exited) {
        status.push(DIM);
    }
    if !matches!(tier, AgeTier::Old | AgeTier::Exited)
        && let Some(color) = status_color(session)
    {
        status.push(color);
    }

    let age = match tier {
        AgeTier::Freshest => vec![GREEN, BOLD],
        AgeTier::Recent => vec![GREEN],
        AgeTier::Stale => vec![DIM],
        AgeTier::Old | AgeTier::Exited => vec![BRIGHT_BLACK],
    };

    let task = if matches!(tier, AgeTier::Old | AgeTier::Exited) {
        vec![BRIGHT_BLACK]
    } else if session.agent_state == Some(AgentState::Waiting) || matches!(tier, AgeTier::Stale) {
        vec![DIM]
    } else {
        Vec::new()
    };

    RowStyles {
        name,
        status,
        age,
        task,
    }
}

pub(crate) fn cmd_summary(session: &Session) -> String {
    if session.is_exited {
        return "\u{1faa6} exited".into(); // tombstone
    }

    let mut commands = Vec::new();
    let mut shell_count = 0u32;

    for pane in &session.panes {
        if pane.command.is_empty() {
            continue;
        }
        if is_shell_command(&pane.command) {
            shell_count += 1;
            continue;
        }
        let base = base_name(&pane.command);
        if is_agent_command(&pane.command) {
            let ind = match pane.agent_state {
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

pub(crate) fn render_table(sessions: &[Session], term_width: Option<usize>) -> Vec<String> {
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

    // Fit the row to the terminal by shrinking columns in priority order:
    // SESSION and AGE keep their natural widths; STATUS truncates when needed;
    // TASK shrinks first, then drops entirely. Non-TTY callers can pass None to
    // keep full STATUS and the original 50-cell TASK cap.
    let (cmd_cap, task_cap, show_task_column) = match term_width {
        None => (max_cmd, 50, true),
        Some(tw) => {
            let inner_with_task = tw.saturating_sub(max_name + max_age + 6);
            let inner_no_task = tw.saturating_sub(max_name + max_age + 4);
            if inner_with_task >= max_cmd + 4 {
                let task = (inner_with_task - max_cmd).min(50);
                (max_cmd, task, true)
            } else if inner_no_task >= max_cmd {
                (max_cmd, 0, false)
            } else {
                (inner_no_task, 0, false)
            }
        }
    };

    let mut lines = Vec::with_capacity(sessions.len() + 2);
    if show_task_column {
        lines.push(format!(
            "{DIM}{:<max_name$}  {:<cmd_cap$}  {:<max_age$}  TASK{RESET}",
            "SESSION", "STATUS", "AGE"
        ));
    } else {
        lines.push(format!(
            "{DIM}{:<max_name$}  {:<cmd_cap$}  {:<max_age$}{RESET}",
            "SESSION", "STATUS", "AGE"
        ));
    }
    let divider_len = if show_task_column {
        max_name + cmd_cap + max_age + 10
    } else {
        max_name + cmd_cap + max_age + 4
    };
    lines.push(format!("{DIM}{}{RESET}", "\u{2501}".repeat(divider_len)));

    for (session, cmd_text) in sessions.iter().zip(cmd_texts.iter()) {
        let tier = age_tier(session, freshest_age);
        let styles = row_styles(session, tier);
        let cmd_owned;
        let (cmd_rendered, cmd_w): (&str, usize) = {
            let w = display_width(cmd_text);
            if w > cmd_cap {
                cmd_owned = truncate_to_width(cmd_text, cmd_cap);
                let w2 = display_width(&cmd_owned);
                (cmd_owned.as_str(), w2)
            } else {
                (cmd_text.as_str(), w)
            }
        };
        let cmd_pad = " ".repeat(cmd_cap.saturating_sub(cmd_w));

        let name_display = paint(&session.name, &styles.name);
        let name_pad = " ".repeat(max_name.saturating_sub(session.name.len()));
        let cmd_display = paint(cmd_rendered, &styles.status);
        let age_text = format!("{:<max_age$}", session.age);
        let age_display = paint(&age_text, &styles.age);

        let task_display = if !show_task_column || session.task.is_empty() {
            String::new()
        } else {
            let task = truncate_to_width(&session.task, task_cap);
            paint(&task, &styles.task)
        };

        if show_task_column {
            lines.push(format!(
                "{name_display}{name_pad}  {cmd_display}{cmd_pad}  {age_display}  {task_display}"
            ));
        } else {
            lines.push(format!(
                "{name_display}{name_pad}  {cmd_display}{cmd_pad}  {age_display}"
            ));
        }
    }

    lines
}

pub(crate) fn print_table(sessions: &[Session]) {
    let term_width = crossterm::terminal::size().ok().map(|(w, _)| w as usize);
    for line in render_table(sessions, term_width) {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Pane, Session};

    fn session(name: &str, status: &str, task: &str) -> Session {
        Session {
            name: name.to_string(),
            age: "2m".to_string(),
            age_seconds: 120,
            is_current: false,
            is_exited: false,
            connected_clients: 0,
            panes: vec![Pane {
                command: status.to_string(),
                cwd: "/tmp".to_string(),
                title: task.to_string(),
                agent_state: Some(AgentState::Working),
            }],
            agent_state: Some(AgentState::Working),
            task: task.to_string(),
        }
    }

    #[test]
    fn truncate_to_width_no_truncation_when_fits() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
        assert_eq!(truncate_to_width("hello", 5), "hello");
    }

    #[test]
    fn truncate_to_width_ascii_truncates_with_ellipsis() {
        assert_eq!(truncate_to_width("Analyze hermes-agent", 10), "Analyze h…");
        assert_eq!(
            display_width(&truncate_to_width("Analyze hermes-agent", 10)),
            10
        );
    }

    #[test]
    fn truncate_to_width_zero_or_one_returns_empty() {
        assert_eq!(truncate_to_width("anything", 0), "");
        assert_eq!(truncate_to_width("anything", 1), "");
    }

    #[test]
    fn truncate_to_width_multibyte_no_panic() {
        let s = "查询".repeat(40);
        let out = truncate_to_width(&s, 10);
        assert!(display_width(&out) <= 10);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_to_width_emoji_safe() {
        let out = truncate_to_width("🚧🚧🚧🚧🚧", 5);
        assert!(display_width(&out) <= 5);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn render_table_keeps_task_column_when_wide() {
        let lines = render_table(
            &[session(
                "alpha",
                "claude",
                "Debug notification certificate issue",
            )],
            Some(100),
        );

        assert!(lines[0].contains("TASK"));
        assert!(lines[2].contains("Debug notification certificate issue"));
    }

    #[test]
    fn render_table_shrinks_task_before_dropping_it() {
        let lines = render_table(
            &[session(
                "alpha",
                "claude",
                "Debug notification certificate issue",
            )],
            Some(36),
        );

        assert!(lines[0].contains("TASK"));
        assert!(lines[2].contains("Debug noti…"));
    }

    #[test]
    fn render_table_drops_task_when_status_fits_without_it() {
        let lines = render_table(
            &[session(
                "alpha",
                "claude",
                "Debug notification certificate issue",
            )],
            Some(28),
        );

        assert!(!lines[0].contains("TASK"));
        assert!(!lines[2].contains("Debug"));
        assert!(lines[2].contains("claude"));
    }

    #[test]
    fn render_table_truncates_status_as_last_resort() {
        let lines = render_table(
            &[session(
                "alpha",
                "claude",
                "Debug notification certificate issue",
            )],
            Some(17),
        );

        assert!(!lines[0].contains("TASK"));
        assert!(lines[2].contains("cl…"));
    }
}
