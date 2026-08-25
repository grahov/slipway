//! Release ids and retention.
//!
//! An id is the UTC deploy time as `%Y%m%d%H%M%S` plus three millisecond
//! digits, so lexicographic order is chronological order, ids stay
//! shell-safe by construction, and two deploys within the same second
//! cannot land in the same release directory.

use jiff::Timestamp;
use jiff::tz::TimeZone;

pub fn new_id() -> String {
    id_at(Timestamp::now())
}

fn id_at(at: Timestamp) -> String {
    let seconds = at.to_zoned(TimeZone::UTC).strftime("%Y%m%d%H%M%S");
    format!("{seconds}{:03}", at.as_millisecond().rem_euclid(1000))
}

/// True for names slipway itself created. Pruning consults this before
/// deleting anything, so foreign files under releases/ are never touched.
/// 14-digit names are the pre-0.4 second-precision form; they still sort
/// correctly next to the current 17-digit ids.
pub fn is_id(name: &str) -> bool {
    matches!(name.len(), 14 | 17) && name.bytes().all(|b| b.is_ascii_digit())
}

/// Oldest releases beyond `keep`, never the current one, oldest first.
pub fn prune_candidates(names: &[String], keep: usize, current: Option<&str>) -> Vec<String> {
    let mut ids: Vec<&String> = names.iter().filter(|n| is_id(n)).collect();
    ids.sort();
    let cutoff = ids.len().saturating_sub(keep);
    ids.into_iter()
        .take(cutoff)
        .filter(|id| Some(id.as_str()) != current)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn id_is_utc_milliseconds() {
        let at: Timestamp = "2026-08-25T13:15:00.123Z".parse().unwrap();
        assert_eq!(id_at(at), "20260825131500123");
        assert!(is_id(&new_id()));
    }

    #[test]
    fn old_second_precision_ids_are_still_ours() {
        assert!(is_id("20260825131500"));
        assert!(is_id("20260825131500123"));
        assert!(!is_id("2026082513150012"));
        let mixed = names(&["20260825131500", "20260825131500123"]);
        assert_eq!(
            prune_candidates(&mixed, 1, None),
            names(&["20260825131500"])
        );
    }

    #[test]
    fn prune_keeps_the_newest_and_the_current() {
        let list = names(&["20260101000000", "20260102000000", "20260103000000"]);
        assert_eq!(
            prune_candidates(&list, 1, None),
            names(&["20260101000000", "20260102000000"])
        );
        assert_eq!(
            prune_candidates(&list, 1, Some("20260101000000")),
            names(&["20260102000000"])
        );
    }

    #[test]
    fn prune_ignores_foreign_names_and_short_lists() {
        let list = names(&["shared", "20260101000000", "current.tmp"]);
        assert!(prune_candidates(&list, 1, None).is_empty());
        assert!(prune_candidates(&list, 5, None).is_empty());
    }
}
