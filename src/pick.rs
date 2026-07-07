use std::io::{self, Write};
use std::process::Command;

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, ClearType, EnterAlternateScreen, LeaveAlternateScreen};

use crate::{
    age::{age_tier, freshest_age_seconds},
    agent::{base_name, is_agent_command},
    display::{
        BRIGHT_CYAN, DIM, GREY_BG, RESET, YELLOW, cmd_summary, display_width, paint, row_styles,
    },
    model::{AgentState, Session},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmAction {
    Kill,
    Delete,
}

impl ConfirmAction {
    fn command(self) -> &'static str {
        match self {
            ConfirmAction::Kill => "kill-session",
            ConfirmAction::Delete => "delete-session",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ConfirmAction::Kill => "kill",
            ConfirmAction::Delete => "delete",
        }
    }

    fn completed_label(self) -> &'static str {
        match self {
            ConfirmAction::Kill => "killed",
            ConfirmAction::Delete => "deleted",
        }
    }
}

#[derive(Debug, Clone)]
struct Confirm {
    action: ConfirmAction,
    session_name: String,
}

#[derive(Debug)]
struct PickState {
    sessions: Vec<Session>,
    selected: usize,
    filter: String,
    filter_mode: bool,
    confirm: Option<Confirm>,
    message: String,
}

fn session_matches_filter(session: &Session, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    session.name.to_lowercase().contains(&filter.to_lowercase())
}

fn visible_indices(sessions: &[Session], filter: &str) -> Vec<usize> {
    sessions
        .iter()
        .enumerate()
        .filter_map(|(idx, session)| session_matches_filter(session, filter).then_some(idx))
        .collect()
}

fn clamp_selection(selected: &mut usize, visible_len: usize) {
    if visible_len == 0 {
        *selected = 0;
    } else if *selected >= visible_len {
        *selected = visible_len - 1;
    }
}

fn selected_session<'a>(state: &'a PickState, visible: &[usize]) -> Option<&'a Session> {
    visible.get(state.selected).map(|idx| &state.sessions[*idx])
}

fn run_zellij_action(action: ConfirmAction, session_name: &str) -> io::Result<()> {
    let status = Command::new("zellij")
        .args([action.command(), session_name])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "zellij {} exited with {status}",
            action.command()
        )))
    }
}

fn refresh_sessions<F>(state: &mut PickState, refresh: &mut F)
where
    F: FnMut() -> Vec<Session>,
{
    let previous_name = {
        let visible = visible_indices(&state.sessions, &state.filter);
        selected_session(state, &visible).map(|session| session.name.clone())
    };

    state.sessions = refresh();
    let visible = visible_indices(&state.sessions, &state.filter);
    state.selected = previous_name
        .and_then(|name| {
            visible
                .iter()
                .position(|idx| state.sessions[*idx].name == name)
        })
        .unwrap_or_else(|| state.selected.min(visible.len().saturating_sub(1)));
    clamp_selection(&mut state.selected, visible.len());
}

fn draw(stdout: &mut io::Stdout, state: &mut PickState) -> io::Result<()> {
    let visible = visible_indices(&state.sessions, &state.filter);
    clamp_selection(&mut state.selected, visible.len());

    execute!(
        stdout,
        cursor::MoveTo(0, 0),
        terminal::Clear(ClearType::All)
    )?;

    if state.filter_mode || !state.filter.is_empty() {
        write!(
            stdout,
            " {DIM}↑/k up · ↓/j down · enter attach · / filter · x kill · d delete · r refresh · q quit · filter:{RESET} {}\r\n\r\n",
            state.filter
        )?;
    } else {
        write!(
            stdout,
            " {DIM}↑/k up · ↓/j down · enter attach · / filter · x kill · d delete · r refresh · q quit{RESET}\r\n\r\n"
        )?;
    }

    if visible.is_empty() {
        write!(stdout, " {DIM}no matches{RESET}\r\n")?;
        stdout.flush()?;
        return Ok(());
    }

    let cmd_texts: Vec<String> = visible
        .iter()
        .map(|idx| cmd_summary(&state.sessions[*idx]))
        .collect();
    let freshest_age = freshest_age_seconds(&state.sessions);
    let max_name = visible
        .iter()
        .map(|idx| state.sessions[*idx].name.len())
        .max()
        .unwrap_or(0);
    let max_cmd = cmd_texts
        .iter()
        .map(|text| display_width(text))
        .max()
        .unwrap_or(0);
    let max_age = visible
        .iter()
        .map(|idx| display_width(&state.sessions[*idx].age))
        .max()
        .unwrap_or(0);
    let cols = terminal::size().map(|(c, _)| c as usize).unwrap_or(80);

    for (row_idx, session_idx) in visible.iter().enumerate() {
        let session = &state.sessions[*session_idx];
        let tier = age_tier(session, freshest_age);
        let styles = row_styles(session, tier);
        let cmd = &cmd_texts[row_idx];
        let cmd_w = display_width(cmd);
        let name_pad = " ".repeat(max_name.saturating_sub(session.name.len()));
        let cmd_pad = " ".repeat(max_cmd.saturating_sub(cmd_w));
        let age_text = if session.age.is_empty() {
            String::new()
        } else {
            format!("{:<max_age$}", session.age)
        };

        if row_idx == state.selected {
            let plain = format!(" ▸ {}{name_pad}  {cmd}{cmd_pad}  {age_text}", session.name);
            let pad = " ".repeat(cols.saturating_sub(display_width(&plain)));
            write!(stdout, "{GREY_BG}{plain}{pad}{RESET}\r\n")?;
            continue;
        }

        let marker = if row_idx == 0 {
            paint("•", &[BRIGHT_CYAN])
        } else {
            " ".into()
        };
        let name = paint(&session.name, &styles.name);
        let cmd_display = paint(cmd, &styles.status);
        let age = if age_text.is_empty() {
            String::new()
        } else {
            paint(&age_text, &styles.age)
        };

        write!(
            stdout,
            " {marker} {name}{name_pad}  {cmd_display}{cmd_pad}  {age}\r\n"
        )?;
    }

    if let Some(sel_sess) = selected_session(state, &visible) {
        write!(stdout, "\r\n {DIM}───{RESET}\r\n")?;

        if let Some(confirm) = &state.confirm {
            write!(
                stdout,
                " {YELLOW}{} {}? y/n{RESET}\r\n",
                confirm.action.label(),
                confirm.session_name
            )?;
        } else if !state.message.is_empty() {
            write!(stdout, " {DIM}{}{RESET}\r\n", state.message)?;
        }

        if !sel_sess.task.is_empty() {
            let state_text = colored_agent_state(sel_sess.agent_state);
            write!(
                stdout,
                " {DIM}task:{RESET} {}{state_text}\r\n",
                sel_sess.task
            )?;
        }

        for pane in &sel_sess.panes {
            if pane.command.is_empty() {
                continue;
            }
            let base = base_name(&pane.command);
            let cwd = pane.cwd.rsplit('/').next().unwrap_or(&pane.cwd);
            if is_agent_command(&pane.command) {
                let state_text = colored_agent_state(pane.agent_state);
                write!(
                    stdout,
                    " {DIM}pane:{RESET} {base}{state_text} {DIM}@ {cwd}{RESET}\r\n"
                )?;
            } else {
                write!(stdout, " {DIM}pane:{RESET} {base} {DIM}@ {cwd}{RESET}\r\n")?;
            }
        }
    }

    stdout.flush()
}

fn colored_agent_state(state: Option<AgentState>) -> String {
    match state {
        Some(AgentState::Working) => format!(" {BRIGHT_CYAN}working{RESET}"),
        Some(AgentState::Waiting) => format!(" {YELLOW}waiting{RESET}"),
        None => String::new(),
    }
}

fn prompt_for_action(state: &mut PickState, visible: &[usize], action: ConfirmAction) {
    let Some(session) = selected_session(state, visible) else {
        return;
    };

    match action {
        ConfirmAction::Kill if session.is_exited => {
            state.message = format!("{} is already exited", session.name);
        }
        ConfirmAction::Delete if !session.is_exited => {
            state.message = format!(
                "{} is live; delete only applies to exited sessions",
                session.name
            );
        }
        _ => {
            state.confirm = Some(Confirm {
                action,
                session_name: session.name.clone(),
            });
            state.message.clear();
        }
    }
}

fn execute_confirmed_action<F>(state: &mut PickState, refresh: &mut F) -> bool
where
    F: FnMut() -> Vec<Session>,
{
    let Some(confirm) = state.confirm.take() else {
        return false;
    };
    let action = confirm.action;
    let session_name = confirm.session_name;

    match run_zellij_action(action, &session_name) {
        Ok(()) => {
            state.message = format!("{} {session_name}", action.completed_label());
            refresh_sessions(state, refresh);
            state.sessions.is_empty()
        }
        Err(err) => {
            state.message = format!("{} failed for {session_name}: {err}", action.label());
            false
        }
    }
}

pub fn run<F>(sessions: Vec<Session>, mut refresh: F) -> Option<String>
where
    F: FnMut() -> Vec<Session>,
{
    terminal::enable_raw_mode().ok()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide).ok()?;

    let mut state = PickState {
        sessions,
        selected: 0,
        filter: String::new(),
        filter_mode: false,
        confirm: None,
        message: String::new(),
    };
    let mut result = None;

    loop {
        draw(&mut stdout, &mut state).ok()?;
        let visible = visible_indices(&state.sessions, &state.filter);

        match event::read() {
            Ok(Event::Key(key)) => {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    break;
                }
                if key.code == KeyCode::Char('q') && key.modifiers.is_empty() {
                    break;
                }

                if let Some(confirm) = &state.confirm {
                    match key.code {
                        KeyCode::Char('y') => {
                            if execute_confirmed_action(&mut state, &mut refresh) {
                                break;
                            }
                        }
                        KeyCode::Char('n') | KeyCode::Esc => {
                            state.message = format!("canceled {}", confirm.action.label());
                            state.confirm = None;
                        }
                        _ => {}
                    }
                    continue;
                }

                if state.filter_mode {
                    match key.code {
                        KeyCode::Esc => state.filter_mode = false,
                        KeyCode::Backspace => {
                            state.filter.pop();
                            let len = visible_indices(&state.sessions, &state.filter).len();
                            clamp_selection(&mut state.selected, len);
                        }
                        KeyCode::Up => state.selected = state.selected.saturating_sub(1),
                        KeyCode::Down => {
                            let len = visible_indices(&state.sessions, &state.filter).len();
                            if state.selected + 1 < len {
                                state.selected += 1;
                            }
                        }
                        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let len = visible_indices(&state.sessions, &state.filter).len();
                            if state.selected + 1 < len {
                                state.selected += 1;
                            }
                        }
                        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            state.selected = state.selected.saturating_sub(1);
                        }
                        KeyCode::Enter => {
                            if let Some(session) = selected_session(&state, &visible) {
                                result = Some(session.name.clone());
                                break;
                            }
                        }
                        KeyCode::Char(ch) if key.modifiers.is_empty() => {
                            state.filter.push(ch);
                            let len = visible_indices(&state.sessions, &state.filter).len();
                            clamp_selection(&mut state.selected, len);
                        }
                        _ => {}
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Up | KeyCode::Char('k') => {
                        state.selected = state.selected.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if state.selected + 1 < visible.len() {
                            state.selected += 1;
                        }
                    }
                    KeyCode::Char('/') => {
                        state.filter_mode = true;
                        state.message.clear();
                    }
                    KeyCode::Char('r') => {
                        refresh_sessions(&mut state, &mut refresh);
                        if state.sessions.is_empty() {
                            break;
                        }
                        state.message = "refreshed".to_string();
                    }
                    KeyCode::Char('x') => {
                        prompt_for_action(&mut state, &visible, ConfirmAction::Kill)
                    }
                    KeyCode::Char('d') => {
                        prompt_for_action(&mut state, &visible, ConfirmAction::Delete);
                    }
                    KeyCode::Enter => {
                        if let Some(session) = selected_session(&state, &visible) {
                            result = Some(session.name.clone());
                            break;
                        }
                    }
                    _ => {}
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    execute!(stdout, LeaveAlternateScreen, cursor::Show).ok();
    terminal::disable_raw_mode().ok();

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str, is_exited: bool) -> Session {
        Session {
            name: name.to_string(),
            age: "1m".to_string(),
            age_seconds: 60,
            is_current: false,
            is_exited,
            connected_clients: 0,
            panes: Vec::new(),
            agent_state: None,
            task: String::new(),
        }
    }

    #[test]
    fn filters_sessions_by_case_insensitive_substring() {
        let sessions = vec![session("alpha", false), session("Beta", false)];
        let visible = visible_indices(&sessions, "et");
        assert_eq!(visible, vec![1]);
    }

    #[test]
    fn clamps_selection_after_filter_changes() {
        let mut selected = 3;
        clamp_selection(&mut selected, 2);
        assert_eq!(selected, 1);

        clamp_selection(&mut selected, 0);
        assert_eq!(selected, 0);
    }
}
