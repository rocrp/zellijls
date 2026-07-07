mod age;
mod agent;
mod collect;
mod display;
mod json_output;
mod model;
mod pick;
mod session_info;
#[cfg(target_os = "macos")]
mod tty_age;
mod watch;

use collect::build_sessions;
use display::print_table;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Table,
    Pick,
    Watch,
    Json,
    Help,
    Version,
}

fn usage() -> &'static str {
    "Usage: zellijls [pick|-i|watch|-w|--json]\n\nCommands:\n  pick, -i      choose a session to attach\n  watch, -w     auto-refreshing dashboard\n\nOptions:\n  --json        print machine-readable session data\n  -h, --help    show this help\n  -V, --version show version"
}

fn parse_mode(args: &[String]) -> Result<Mode, String> {
    match args {
        [] => Ok(Mode::Table),
        [arg] => match arg.as_str() {
            "pick" | "-i" => Ok(Mode::Pick),
            "watch" | "-w" => Ok(Mode::Watch),
            "--json" => Ok(Mode::Json),
            "-h" | "--help" => Ok(Mode::Help),
            "-V" | "--version" => Ok(Mode::Version),
            unknown => Err(format!("unknown argument `{unknown}`")),
        },
        _ => Err("--json, pick, watch, help, and version cannot be combined".to_string()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match parse_mode(&args) {
        Ok(mode) => mode,
        Err(err) => {
            eprintln!("error: {err}; run `zellijls --help` for usage");
            std::process::exit(2);
        }
    };

    match mode {
        Mode::Help => {
            println!("{}", usage());
            return;
        }
        Mode::Version => {
            println!("zellijls {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Mode::Table | Mode::Pick | Mode::Watch | Mode::Json => {}
    }

    let sessions = build_sessions();

    match mode {
        Mode::Pick => {
            if sessions.is_empty() {
                use std::os::unix::process::CommandExt;
                let err = std::process::Command::new("zellij").exec();
                eprintln!("Failed to launch zellij: {err}");
                std::process::exit(1);
            }
            if let Some(name) = pick::run(sessions, build_sessions) {
                use std::os::unix::process::CommandExt;
                let err = std::process::Command::new("zellij")
                    .args(["attach", &name])
                    .exec();
                eprintln!("Failed to attach: {err}");
                std::process::exit(1);
            }
        }
        Mode::Json => {
            if let Err(err) = json_output::print_json(&sessions) {
                eprintln!("error: failed to write json: {err}");
                std::process::exit(1);
            }
        }
        Mode::Watch => {
            if let Err(err) = watch::run(sessions, build_sessions) {
                eprintln!("error: watch mode failed: {err}");
                std::process::exit(1);
            }
        }
        Mode::Table => {
            if sessions.is_empty() {
                println!("No zellij sessions.");
                return;
            }
            print_table(&sessions);
        }
        Mode::Help | Mode::Version => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parses_empty_args_as_table() {
        assert_eq!(parse_mode(&args(&[])), Ok(Mode::Table));
    }

    #[test]
    fn parses_known_single_modes() {
        assert_eq!(parse_mode(&args(&["pick"])), Ok(Mode::Pick));
        assert_eq!(parse_mode(&args(&["-i"])), Ok(Mode::Pick));
        assert_eq!(parse_mode(&args(&["watch"])), Ok(Mode::Watch));
        assert_eq!(parse_mode(&args(&["-w"])), Ok(Mode::Watch));
        assert_eq!(parse_mode(&args(&["--json"])), Ok(Mode::Json));
        assert_eq!(parse_mode(&args(&["--help"])), Ok(Mode::Help));
        assert_eq!(parse_mode(&args(&["--version"])), Ok(Mode::Version));
    }

    #[test]
    fn rejects_unknown_args() {
        assert_eq!(
            parse_mode(&args(&["pikc"])),
            Err("unknown argument `pikc`".to_string())
        );
    }

    #[test]
    fn rejects_combined_modes() {
        assert!(parse_mode(&args(&["--json", "pick"])).is_err());
    }
}
