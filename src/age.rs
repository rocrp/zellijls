use crate::Session;

const RECENT_AGE_LIMIT_SECS: u64 = 6 * 60 * 60;
const STALE_AGE_LIMIT_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgeTier {
    Freshest,
    Recent,
    Stale,
    Old,
    Exited,
}

pub(crate) fn format_age(age_seconds: u64) -> String {
    const DAY: u64 = 24 * 60 * 60;
    const HOUR: u64 = 60 * 60;
    const MINUTE: u64 = 60;

    if age_seconds >= DAY {
        format!("{}d", age_seconds / DAY)
    } else if age_seconds >= HOUR {
        format!("{}h", age_seconds / HOUR)
    } else if age_seconds >= MINUTE {
        format!("{}m", age_seconds / MINUTE)
    } else {
        format!("{age_seconds}s")
    }
}

pub(crate) fn freshest_age_seconds(sessions: &[Session]) -> Option<u64> {
    sessions
        .iter()
        .filter(|s| !s.is_exited)
        .map(|s| s.age_seconds)
        .min()
}

pub(crate) fn age_tier(session: &Session, freshest_age: Option<u64>) -> AgeTier {
    if session.is_exited {
        return AgeTier::Exited;
    }

    if freshest_age == Some(session.age_seconds) {
        return AgeTier::Freshest;
    }

    if session.age_seconds <= RECENT_AGE_LIMIT_SECS {
        AgeTier::Recent
    } else if session.age_seconds <= STALE_AGE_LIMIT_SECS {
        AgeTier::Stale
    } else {
        AgeTier::Old
    }
}

pub(crate) fn sort_sessions_for_display(sessions: &mut [Session]) {
    sessions.sort_by_key(|s| !s.is_current);
}

#[cfg(test)]
mod tests {
    use super::{age_tier, format_age, freshest_age_seconds, sort_sessions_for_display, AgeTier};
    use crate::{Pane, Session};

    fn session(name: &str, age_seconds: u64, is_current: bool, is_exited: bool) -> Session {
        Session {
            name: name.to_string(),
            age: format!("{age_seconds}s"),
            age_seconds,
            is_current,
            is_exited,
            connected_clients: 0,
            panes: Vec::<Pane>::new(),
            agent_state: None,
            task: String::new(),
        }
    }

    #[test]
    fn formats_multi_part_ages() {
        assert_eq!(format_age(0), "0s");
        assert_eq!(format_age(53), "53s");
        assert_eq!(format_age(53 * 60 + 49), "53m");
        assert_eq!(format_age(20 * 60 * 60 + 7 * 60 + 9), "20h");
        assert_eq!(
            format_age(6 * 24 * 60 * 60 + 21 * 60 * 60 + 57 * 60 + 23),
            "6d"
        );
    }

    #[test]
    fn ignores_exited_sessions_when_finding_freshest() {
        let sessions = vec![
            session("exited", 30, false, true),
            session("fresh", 90, false, false),
            session("older", 600, false, false),
        ];

        assert_eq!(freshest_age_seconds(&sessions), Some(90));
    }

    #[test]
    fn classifies_age_tiers() {
        let freshest = Some(120);

        assert_eq!(
            age_tier(&session("freshest", 120, false, false), freshest),
            AgeTier::Freshest
        );
        assert_eq!(
            age_tier(&session("recent", 3 * 60 * 60, false, false), freshest),
            AgeTier::Recent
        );
        assert_eq!(
            age_tier(&session("stale", 12 * 60 * 60, false, false), freshest),
            AgeTier::Stale
        );
        assert_eq!(
            age_tier(&session("old", 3 * 24 * 60 * 60, false, false), freshest),
            AgeTier::Old
        );
        assert_eq!(
            age_tier(&session("exited", 60, false, true), freshest),
            AgeTier::Exited
        );
    }

    #[test]
    fn pins_current_session_without_reordering_the_rest() {
        let mut sessions = vec![
            session("older", 10_000, false, false),
            session("current", 1_000, true, false),
            session("newest", 100, false, false),
        ];

        sort_sessions_for_display(&mut sessions);

        let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["current", "older", "newest"]);
    }
}
