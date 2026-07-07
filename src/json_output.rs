use serde::Serialize;

use crate::agent::is_agent_command;
use crate::display::cmd_summary;
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
            status: cmd_summary(session),
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
