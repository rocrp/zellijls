use crate::model::Pane;

const IDLE_SHELLS: &[&str] = &["zsh", "bash", "sh", "fish"];

pub(crate) fn base_name(cmd: &str) -> &str {
    let binary = cmd.split_whitespace().next().unwrap_or("");
    binary.rsplit('/').next().unwrap_or(binary)
}

pub(crate) fn is_agent_base(base: &str) -> bool {
    base == "claude" || base == "codex" || base.starts_with("codex-")
}

pub(crate) fn is_agent_command(cmd: &str) -> bool {
    is_agent_base(base_name(cmd))
}

pub(crate) fn is_shell_command(cmd: &str) -> bool {
    IDLE_SHELLS.contains(&base_name(cmd))
}

pub(crate) fn is_spinner(c: char) -> bool {
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

pub(crate) fn strip_spinner_prefix(title: &str) -> &str {
    let task = title.trim_start_matches(|c: char| is_spinner(c) || c == '✳' || c == ' ');
    task.trim_start()
}

pub(crate) fn task_from_agent_pane(pane: &Pane, session_name: &str) -> Option<String> {
    let task = strip_spinner_prefix(&pane.title);
    if task.is_empty() {
        return None;
    }

    if task.eq_ignore_ascii_case("Claude Code")
        || task.eq_ignore_ascii_case(session_name)
        || pane
            .cwd
            .rsplit('/')
            .next()
            .is_some_and(|cwd_base| task.eq_ignore_ascii_case(cwd_base))
    {
        return None;
    }

    Some(task.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Pane;

    fn pane(title: &str, cwd: &str) -> Pane {
        Pane {
            command: "claude".to_string(),
            cwd: cwd.to_string(),
            title: title.to_string(),
            agent_state: None,
        }
    }

    #[test]
    fn suppresses_claude_default_title_after_spinner() {
        assert_eq!(
            task_from_agent_pane(
                &pane("⠐ Claude Code", "/Users/rocry/w/zellijls"),
                "zellijls"
            ),
            None
        );
    }

    #[test]
    fn suppresses_cwd_basename_title() {
        assert_eq!(
            task_from_agent_pane(&pane("eegoread", "/Users/rocry/w/eegoread"), "other"),
            None
        );
    }

    #[test]
    fn suppresses_session_name_title() {
        assert_eq!(
            task_from_agent_pane(&pane("eegoread", "/Users/rocry/w/other"), "eegoread"),
            None
        );
    }

    #[test]
    fn keeps_real_task_title() {
        assert_eq!(
            task_from_agent_pane(
                &pane(
                    "Debug push notification certificate issue",
                    "/Users/rocry/w/vas"
                ),
                "vas"
            ),
            Some("Debug push notification certificate issue".to_string())
        );
    }
}
