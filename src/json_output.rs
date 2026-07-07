use serde::Serialize;

use crate::agent::{base_name, is_agent_command, is_shell_command};
use crate::model::{AgentState, Session};

#[derive(Serialize)]
struct JsonPane<'a> {
    command: &'a str,
    cwd: &'a str,
    agent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'static str>,
}

#[derive(Serialize)]
struct JsonSession<'a> {
    name: &'a str,
    exited: bool,
    current: bool,
    attached: bool,
    age_seconds: u64,
    age: &'a str,
    status: String,
    agent_state: Option<&'static str>,
    task: &'a str,
    panes: Vec<JsonPane<'a>>,
}

fn state_name(state: Option<AgentState>) -> Option<&'static str> {
    state.map(AgentState::as_str)
}

fn status_text(session: &Session) -> String {
    if session.is_exited {
        return "exited".to_string();
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
        commands.push(base_name(&pane.command).to_string());
    }

    if commands.is_empty() {
        return if shell_count > 0 {
            "idle".to_string()
        } else {
            "empty".to_string()
        };
    }

    let mut status = commands.join(" + ");
    if shell_count > 0 {
        status.push_str(&format!(" +{shell_count}sh"));
    }
    status
}

pub(crate) fn print_json(sessions: &[Session]) -> serde_json::Result<()> {
    let payload: Vec<JsonSession<'_>> = sessions
        .iter()
        .map(|session| JsonSession {
            name: &session.name,
            exited: session.is_exited,
            current: session.is_current,
            attached: session.connected_clients > 0,
            age_seconds: session.age_seconds,
            age: &session.age,
            status: status_text(session),
            agent_state: state_name(session.agent_state),
            task: &session.task,
            panes: session
                .panes
                .iter()
                .map(|pane| {
                    let agent = is_agent_command(&pane.command);
                    JsonPane {
                        command: &pane.command,
                        cwd: &pane.cwd,
                        agent,
                        state: if agent {
                            state_name(pane.agent_state)
                        } else {
                            None
                        },
                    }
                })
                .collect(),
        })
        .collect();

    serde_json::to_writer_pretty(std::io::stdout(), &payload)?;
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Pane;

    fn session(panes: Vec<Pane>, is_exited: bool) -> Session {
        Session {
            name: "test".to_string(),
            age: "1m".to_string(),
            age_seconds: 60,
            is_current: false,
            is_exited,
            connected_clients: 0,
            panes,
            agent_state: None,
            task: String::new(),
        }
    }

    fn pane(command: &str, state: Option<AgentState>) -> Pane {
        Pane {
            command: command.to_string(),
            cwd: "/tmp".to_string(),
            title: String::new(),
            agent_state: state,
        }
    }

    #[test]
    fn status_text_omits_agent_state_glyphs() {
        let session = session(
            vec![
                pane("claude", Some(AgentState::Waiting)),
                pane("codex", Some(AgentState::Working)),
                pane("zsh", None),
            ],
            false,
        );

        assert_eq!(status_text(&session), "claude + codex +1sh");
    }

    #[test]
    fn status_text_is_plain_for_empty_idle_and_exited() {
        assert_eq!(status_text(&session(vec![], false)), "empty");
        assert_eq!(
            status_text(&session(vec![pane("zsh", None)], false)),
            "idle"
        );
        assert_eq!(status_text(&session(vec![], true)), "exited");
    }
}
