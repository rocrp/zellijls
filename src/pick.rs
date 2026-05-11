use std::io::{self, Write};

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, ClearType, EnterAlternateScreen, LeaveAlternateScreen};

use crate::{
    AgentState, BOLD, BRIGHT_BLACK, BRIGHT_CYAN, DIM, GREEN, RESET, Session, UNDERLINE, YELLOW,
    age::{AgeTier, age_tier, freshest_age_seconds},
    base_name, cmd_summary, display_width, paint, status_color,
};

pub fn run(sessions: &[Session]) -> Option<String> {
    // Pre-compute display strings
    let cmd_texts: Vec<String> = sessions.iter().map(cmd_summary).collect();
    let freshest_age = freshest_age_seconds(sessions);

    // Column widths for alignment
    let max_name = sessions.iter().map(|s| s.name.len()).max().unwrap_or(0);
    let max_cmd = cmd_texts
        .iter()
        .map(|t| display_width(t))
        .max()
        .unwrap_or(0);
    let max_age = sessions
        .iter()
        .map(|s| display_width(&s.age))
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
        for (list_idx, s) in sessions.iter().enumerate() {
            let tier = age_tier(s, freshest_age);
            let cmd = &cmd_texts[list_idx];
            let cmd_w = display_width(cmd);
            let is_sel = list_idx == sel;

            let marker = if is_sel {
                paint("\u{25b8}", &[GREEN])
            } else if matches!(tier, AgeTier::Freshest) {
                paint("\u{2022}", &[BRIGHT_CYAN])
            } else {
                " ".into()
            };

            let mut name_styles = Vec::new();
            if s.is_exited {
                name_styles.push(DIM);
            } else {
                if is_sel {
                    name_styles.push(BOLD);
                }
                if s.is_current {
                    name_styles.push(GREEN);
                } else if !is_sel {
                    match tier {
                        AgeTier::Freshest => name_styles.push(BRIGHT_CYAN),
                        AgeTier::Recent => {}
                        AgeTier::Stale => name_styles.push(DIM),
                        AgeTier::Old | AgeTier::Exited => name_styles.push(BRIGHT_BLACK),
                    }
                } else if matches!(tier, AgeTier::Freshest) {
                    name_styles.push(BRIGHT_CYAN);
                }
                if s.connected_clients > 0 {
                    name_styles.push(UNDERLINE);
                }
            }
            let name = paint(&s.name, &name_styles);
            let name_pad = " ".repeat(max_name.saturating_sub(s.name.len()));

            let mut cmd_styles = Vec::new();
            if is_sel || matches!(tier, AgeTier::Freshest) {
                cmd_styles.push(BOLD);
            } else if matches!(tier, AgeTier::Stale) {
                cmd_styles.push(DIM);
            } else if matches!(tier, AgeTier::Old | AgeTier::Exited) {
                cmd_styles.push(BRIGHT_BLACK);
            }
            if !matches!(tier, AgeTier::Old | AgeTier::Exited)
                && let Some(color) = status_color(s)
            {
                cmd_styles.push(color);
            }
            let cmd_display = paint(cmd, &cmd_styles);
            let cmd_pad = " ".repeat(max_cmd.saturating_sub(cmd_w));

            let age = if s.age.is_empty() {
                String::new()
            } else {
                let age_text = format!("{:<max_age$}", s.age);
                match tier {
                    AgeTier::Freshest => paint(&age_text, &[GREEN, BOLD]),
                    AgeTier::Recent => paint(&age_text, &[GREEN]),
                    AgeTier::Stale => paint(&age_text, &[DIM]),
                    AgeTier::Old | AgeTier::Exited => paint(&age_text, &[BRIGHT_BLACK]),
                }
            };

            write!(
                stdout,
                " {marker} {name}{name_pad}  {cmd_display}{cmd_pad}  {age}\r\n"
            )
            .ok();
        }

        // Detail section for selected session
        let sel_sess = &sessions[sel];
        write!(stdout, "\r\n {DIM}\u{2500}\u{2500}\u{2500}{RESET}\r\n").ok();

        if !sel_sess.task.is_empty() {
            let state = match sel_sess.agent_state {
                Some(AgentState::Working) => format!(" {BRIGHT_CYAN}working{RESET}"),
                Some(AgentState::Waiting) => format!(" {YELLOW}waiting{RESET}"),
                None => String::new(),
            };
            write!(stdout, " {DIM}task:{RESET} {}{state}\r\n", sel_sess.task).ok();
        }

        for pane in &sel_sess.panes {
            if pane.command.is_empty() {
                continue;
            }
            let base = base_name(&pane.command);
            let cwd = pane.cwd.rsplit('/').next().unwrap_or(&pane.cwd);
            write!(stdout, " {DIM}pane:{RESET} {base} {DIM}@ {cwd}{RESET}\r\n").ok();
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
                    if sel + 1 < sessions.len() {
                        sel += 1;
                    }
                }
                KeyCode::Enter => {
                    result = Some(sessions[sel].name.clone());
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
