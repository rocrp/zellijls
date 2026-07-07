mod age;
mod agent;
mod collect;
mod display;
mod model;
mod pick;
mod session_info;
#[cfg(target_os = "macos")]
mod tty_age;

use collect::build_sessions;
use display::print_table;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let subcmd = args.get(1).map(|s| s.as_str());

    let sessions = build_sessions();

    match subcmd {
        Some("pick" | "-i") => {
            if sessions.is_empty() {
                use std::os::unix::process::CommandExt;
                let err = std::process::Command::new("zellij").exec();
                eprintln!("Failed to launch zellij: {err}");
                std::process::exit(1);
            }
            if let Some(name) = pick::run(&sessions) {
                use std::os::unix::process::CommandExt;
                let err = std::process::Command::new("zellij")
                    .args(["attach", &name])
                    .exec();
                eprintln!("Failed to attach: {err}");
                std::process::exit(1);
            }
        }
        _ => {
            if sessions.is_empty() {
                println!("No zellij sessions.");
                return;
            }
            print_table(&sessions);
        }
    }
}
