//! All-apps list model: ordering and search by name.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    pub key: String,
    pub label: String,
    lower: String,
}

impl AppEntry {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        let label = label.into();
        AppEntry {
            key: key.into(),
            lower: label.to_lowercase(),
            label,
        }
    }
}

/// Sort apps by name, case-insensitively, keys breaking ties.
pub fn sort_apps(apps: &mut [AppEntry]) {
    apps.sort_by(|a, b| a.lower.cmp(&b.lower).then_with(|| a.key.cmp(&b.key)));
}

/// Indices into `apps` (assumed sorted) matching `query`, best first: name starts
/// with the query, then a word starts with it, then a substring. Blank query
/// returns everything in list order. Ties keep list (alphabetical) order.
pub fn search(apps: &[AppEntry], query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return (0..apps.len()).collect();
    }
    let mut hits: Vec<(u8, usize)> = apps
        .iter()
        .enumerate()
        .filter_map(|(i, a)| score(&a.lower, &q).map(|s| (s, i)))
        .collect();
    hits.sort_by_key(|&(s, i)| (s, i));
    hits.into_iter().map(|(_, i)| i).collect()
}

fn score(name: &str, q: &str) -> Option<u8> {
    if name.starts_with(q) {
        return Some(0);
    }
    let word_start = name
        .match_indices(q)
        .any(|(i, _)| name[..i].chars().next_back().is_some_and(|c| !c.is_alphanumeric()));
    if word_start {
        return Some(1);
    }
    name.contains(q).then_some(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apps() -> Vec<AppEntry> {
        let mut v = vec![
            AppEntry::new("com.cam", "Camera"),
            AppEntry::new("com.cal", "Calendar"),
            AppEntry::new("com.gcal", "Google Calendar"),
            AppEntry::new("com.clock", "Clock"),
            AppEntry::new("com.maps", "Maps"),
            AppEntry::new("com.fcam", "Open Camera"),
            AppEntry::new("com.ru", "Калькулятор"),
        ];
        sort_apps(&mut v);
        v
    }

    fn names(v: &[AppEntry], idx: Vec<usize>) -> Vec<String> {
        idx.into_iter().map(|i| v[i].label.clone()).collect()
    }

    #[test]
    fn blank_returns_all_sorted() {
        let v = apps();
        let r = names(&v, search(&v, "  "));
        assert_eq!(r.len(), 7);
        assert_eq!(r[0], "Calendar");
        assert_eq!(r.last().unwrap(), "Калькулятор");
    }

    #[test]
    fn prefix_before_word_before_substring() {
        let v = apps();
        assert_eq!(names(&v, search(&v, "cal")), ["Calendar", "Google Calendar"]);
        assert_eq!(names(&v, search(&v, "cam")), ["Camera", "Open Camera"]);
        assert_eq!(names(&v, search(&v, "ock")), ["Clock"]);
    }

    #[test]
    fn case_insensitive_unicode() {
        let v = apps();
        assert_eq!(names(&v, search(&v, "КАЛЬК")), ["Калькулятор"]);
    }

    #[test]
    fn no_match_is_empty() {
        let v = apps();
        assert!(search(&v, "zzz").is_empty());
    }
}
