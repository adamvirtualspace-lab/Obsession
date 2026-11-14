//! Fuzzy matching for the quick switcher.
//!
//! The query's characters must appear in order (case-insensitive). Matches in
//! the file name, at word starts, and in a row score higher; long paths score
//! a little lower.

/// Returns a score (higher is better), or `None` if `query` doesn't match.
pub fn score(query: &str, candidate: &str) -> Option<i32> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Some(0);
    }

    let name_start = candidate.rfind('/').map_or(0, |i| i + 1);
    let mut score = 0;
    let mut q = 0;
    let mut previous_matched = false;
    let mut previous: Option<char> = None;

    for (index, ch) in candidate.char_indices() {
        if q == query.len() {
            break;
        }
        let matched = ch.to_lowercase().eq(std::iter::once(query[q]));
        if matched {
            score += 1;
            if previous_matched {
                score += 5;
            }
            if previous.is_none_or(|p| matches!(p, '/' | ' ' | '-' | '_' | '.')) {
                score += 8;
            }
            if index >= name_start {
                score += 3;
            }
            q += 1;
        }
        previous_matched = matched;
        previous = Some(ch);
    }

    (q == query.len()).then(|| score * 16 - candidate.len() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_in_order_only() {
        assert!(score("sh04", "anim/sh040.md").is_some());
        assert!(score("40sh", "anim/sh040.md").is_none());
        assert!(score("", "anything").is_some());
    }

    #[test]
    fn prefers_file_name_and_word_starts() {
        let name = score("light", "lighting/notes.md").unwrap();
        let path = score("light", "notes/lighting.md").unwrap();
        assert!(path > name, "matching the file name should win");

        let start = score("sh", "anim/sh040.md").unwrap();
        let middle = score("sh", "anim/fish.md").unwrap();
        assert!(start > middle);
    }
}
