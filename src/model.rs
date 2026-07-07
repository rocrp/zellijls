#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentState {
    Working,
    Waiting,
}

impl AgentState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            AgentState::Working => "working",
            AgentState::Waiting => "waiting",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Pane {
    pub command: String,
    pub cwd: String,
    pub title: String,
    pub agent_state: Option<AgentState>,
}

#[derive(Debug, Clone)]
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
