//! Release ids and retention.
//!
//! An id is the UTC deploy time as `%Y%m%d%H%M%S`, so lexicographic order
//! is chronological order and ids stay shell-safe by construction.

use jiff::Timestamp;
use jiff::tz::TimeZone;

pub fn new_id() -> String {
    id_at(Timestamp::now())
}

fn id_at(at: Timestamp) -> String {
    at.to_zoned(TimeZone::UTC)
        .strftime("%Y%m%d%H%M%S")
        .to_string()
}

/// True for names slipway itself created. Pruning consults this before
/// deleting anything, so foreign files under releases/ are never touched.
pub fn is_id(name: &str) -> bool {
    name.len() == 14 && name.bytes().all(|b| b.is_ascii_digit())
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
    fn id_is_utc_seconds() {
        let at: Timestamp = "2026-08-25T13:15:00Z".parse().unwrap();
        assert_eq!(id_at(at), "20260825131500");
        assert!(is_id(&new_id()));
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
