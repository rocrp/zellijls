use std::io::{self, Write};
use std::time::Duration;

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, ClearType, EnterAlternateScreen, LeaveAlternateScreen};

use crate::display::{DIM, RESET, render_table};
use crate::model::Session;

fn draw(stdout: &mut io::Stdout, sessions: &[Session]) -> io::Result<()> {
    execute!(
        stdout,
        cursor::MoveTo(0, 0),
        terminal::Clear(ClearType::All)
    )?;
    write!(
        stdout,
        " {DIM}q/esc/ctrl-c quit · refresh 2s{RESET}\r\n\r\n"
    )?;

    if sessions.is_empty() {
        write!(stdout, "No zellij sessions.\r\n")?;
    } else {
        let width = terminal::size().ok().map(|(cols, _)| cols as usize);
        for line in render_table(sessions, width) {
            write!(stdout, "{line}\r\n")?;
        }
    }

    stdout.flush()
}

pub(crate) fn run<F>(mut sessions: Vec<Session>, mut refresh: F) -> io::Result<()>
where
    F: FnMut() -> Vec<Session>,
{
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;

    let loop_result = (|| -> io::Result<()> {
        draw(&mut stdout, &sessions)?;
        loop {
            if event::poll(Duration::from_secs(2))? {
                match event::read()? {
                    Event::Key(key) => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break;
                        }
                        _ => {}
                    },
                    Event::Resize(_, _) => draw(&mut stdout, &sessions)?,
                    _ => {}
                }
            } else {
                sessions = refresh();
                draw(&mut stdout, &sessions)?;
            }
        }
        Ok(())
    })();

    let leave_result = execute!(stdout, LeaveAlternateScreen, cursor::Show);
    let raw_result = terminal::disable_raw_mode();

    loop_result?;
    leave_result?;
    raw_result
}
