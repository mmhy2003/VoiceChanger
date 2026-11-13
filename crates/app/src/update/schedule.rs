//! When to check for updates, and when a found release is worth showing. Pure: the caller passes the clock.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::version::Version;

/// Checks run every 6 hours while the app is open, and on the next start once 6 hours have passed.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// The first check of a session waits this long after startup, so the window and audio come up first and
/// startup never waits on the network.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(5);

/// Seconds since the Unix epoch; 0 if the system clock is before 1970.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// How long until the next automatic check; `None` if automatic checks are off.
///
/// `last_check` is persisted, so a restart keeps the 6-hour timer instead of checking on every start.
/// `last_check == 0` means never checked. A `now` before `last_check` means the clock went backwards;
/// the check is due then, because otherwise a clock set back by days would stop checks for days.
pub fn next_check(
    auto_check: bool,
    last_check: u64,
    now: u64,
    since_start: Duration,
) -> Option<Duration> {
    if !auto_check {
        return None;
    }
    let startup_wait = FIRST_CHECK_DELAY.saturating_sub(since_start);
    let interval_wait = if last_check == 0 || now < last_check {
        Duration::ZERO
    } else {
        Duration::from_secs(
            last_check
                .saturating_add(CHECK_INTERVAL.as_secs())
                .saturating_sub(now),
        )
    };
    Some(startup_wait.max(interval_wait))
}

/// An automatic check should run now.
pub fn check_due(auto_check: bool, last_check: u64, now: u64, since_start: Duration) -> bool {
    next_check(auto_check, last_check, now, since_start) == Some(Duration::ZERO)
}

/// Show `latest` only if it is newer than the running version. A version the user skipped stays hidden
/// on automatic checks, but a manual "Check for updates" always shows it. A skipped value that does not
/// parse never hides anything.
pub fn should_notify(
    latest: Version,
    current: Version,
    skipped: Option<&str>,
    manual: bool,
) -> bool {
    latest > current && (manual || skipped.and_then(Version::parse) != Some(latest))
}

#[cfg(test)]
mod tests {
    use super::*;

    const H6: u64 = 21600;
    const LATE: Duration = Duration::from_secs(60);

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn off_never_due() {
        assert_eq!(next_check(false, 0, 1000, LATE), None);
        assert!(!check_due(false, 0, 1000, LATE));
    }

    #[test]
    fn never_checked_due_after_startup_delay() {
        let now = 1_000_000;
        assert_eq!(
            next_check(true, 0, now, Duration::ZERO),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            next_check(true, 0, now, Duration::from_secs(2)),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            next_check(true, 0, now, Duration::from_secs(5)),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn due_at_six_hours() {
        assert_eq!(
            next_check(true, 1000, 1000 + H6 - 1, LATE),
            Some(Duration::from_secs(1))
        );
        assert!(!check_due(true, 1000, 1000 + H6 - 1, LATE));
        assert!(check_due(true, 1000, 1000 + H6, LATE));
        assert!(check_due(true, 1000, 1000 + H6 + 500, LATE));
    }

    #[test]
    fn restart_keeps_timer() {
        let now = 1_000_000;
        assert_eq!(
            next_check(true, now - 3600, now, Duration::ZERO),
            Some(Duration::from_secs(5 * 3600))
        );
    }

    #[test]
    fn clock_backwards_is_due() {
        assert!(check_due(true, 5000, 4000, LATE));
    }

    #[test]
    fn notify_newer_only() {
        assert!(should_notify(v("1.1.0"), v("1.0.0"), None, false));
        for manual in [false, true] {
            assert!(!should_notify(v("1.0.0"), v("1.0.0"), None, manual));
            assert!(!should_notify(v("0.9.0"), v("1.0.0"), None, manual));
        }
    }

    #[test]
    fn skipped_hidden_unless_manual() {
        let current = v("1.0.0");
        assert!(!should_notify(v("1.1.0"), current, Some("1.1.0"), false));
        assert!(should_notify(v("1.1.0"), current, Some("1.1.0"), true));
        assert!(should_notify(v("1.2.0"), current, Some("1.1.0"), false));
        assert!(should_notify(v("1.1.0"), current, Some("garbage"), false));
    }

    #[test]
    fn unix_now_is_recent() {
        assert!(unix_now() > 1_700_000_000);
    }
}
