use std::io::{self, Write};

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, ClearType, EnterAlternateScreen, LeaveAlternateScreen};

use crate::{
    base_name, cmd_summary, display_width, AgentState, Session, BOLD, BRIGHT_CYAN, DIM, GREEN,
    RESET, YELLOW,
};

pub fn run(sessions: &[Session]) -> Option<String> {
    let active: Vec<(usize, &Session)> = sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.is_exited)
        .collect();

    if active.is_empty() {
        eprintln!("No active sessions.");
        return None;
    }

    // Pre-compute display strings
    let cmd_texts: Vec<String> = sessions.iter().map(cmd_summary).collect();

    // Column widths for alignment
    let max_name = active.iter().map(|(_, s)| s.name.len()).max().unwrap_or(0);
    let max_cmd = active
        .iter()
        .map(|(i, _)| display_width(&cmd_texts[*i]))
        .max()
        .unwrap_or(0);

    terminal::enable_raw_mode().ok()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide).ok()?;

    let mut sel = 0usize;
    let result;

    loop {
        execute!(
            stdout,
            cursor::MoveTo(0, 0),
            terminal::Clear(ClearType::All)
        )
        .ok();

        // Header
        write!(
            stdout,
            " {DIM}\u{2191}/k up \u{00b7} \u{2193}/j down \u{00b7} enter attach \u{00b7} q quit{RESET}\r\n\r\n"
        )
        .ok();

        // Session list
        for (list_idx, &(sess_idx, s)) in active.iter().enumerate() {
            let cmd = &cmd_texts[sess_idx];
            let cmd_w = display_width(cmd);
            let is_sel = list_idx == sel;

            let marker = if is_sel {
                format!("{GREEN}\u{25b8}{RESET}")
            } else {
                " ".into()
            };

            let name = if is_sel {
                format!("{BOLD}{}{RESET}", s.name)
            } else {
                s.name.clone()
            };
            let name_pad = " ".repeat(max_name.saturating_sub(s.name.len()));

            let cmd_display = if is_sel {
                // Color by agent state
                match s.agent_state {
                    Some(AgentState::Working) => format!("{BRIGHT_CYAN}{cmd}{RESET}"),
                    Some(AgentState::Waiting) => format!("{YELLOW}{cmd}{RESET}"),
                    None => cmd.clone(),
                }
            } else {
                format!("{DIM}{cmd}{RESET}")
            };
            let cmd_pad = " ".repeat(max_cmd.saturating_sub(cmd_w));

            let age = if s.age.is_empty() {
                String::new()
            } else {
                format!("{DIM}{}{RESET}", s.age)
            };

            write!(
                stdout,
                " {marker} {name}{name_pad}  {cmd_display}{cmd_pad}  {age}\r\n"
            )
            .ok();
        }

        // Detail section for selected session
        let sel_sess = active[sel].1;
        write!(stdout, "\r\n {DIM}\u{2500}\u{2500}\u{2500}{RESET}\r\n").ok();

        if !sel_sess.task.is_empty() {
            let state = match sel_sess.agent_state {
                Some(AgentState::Working) => format!(" {BRIGHT_CYAN}working{RESET}"),
                Some(AgentState::Waiting) => format!(" {YELLOW}waiting{RESET}"),
                None => String::new(),
            };
            write!(
                stdout,
                " {DIM}task:{RESET} {}{state}\r\n",
                sel_sess.task
            )
            .ok();
        }

        for pane in &sel_sess.panes {
            if pane.command.is_empty() {
                continue;
            }
            let base = base_name(&pane.command);
            let cwd = pane.cwd.rsplit('/').next().unwrap_or(&pane.cwd);
            write!(
                stdout,
                " {DIM}pane:{RESET} {base} {DIM}@ {cwd}{RESET}\r\n"
            )
            .ok();
        }

        stdout.flush().ok();

        match event::read() {
            Ok(Event::Key(key)) => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    result = None;
                    break;
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    result = None;
                    break;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    sel = sel.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if sel + 1 < active.len() {
                        sel += 1;
                    }
                }
                KeyCode::Enter => {
                    result = Some(active[sel].1.name.clone());
                    break;
                }
                _ => {}
            },
            Ok(_) => {}
            Err(_) => {
                result = None;
                break;
            }
        }
    }

    execute!(stdout, LeaveAlternateScreen, cursor::Show).ok();
    terminal::disable_raw_mode().ok();

    result
}
