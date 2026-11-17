//! People and crew (Syntax.md, section 4): `@name` mentions on task lines,
//! crew cards in `_crew/`, and the OVERLOADED / POWERHOUSE badges.

use std::collections::HashMap;
use std::ops::Range;

use crate::status::{self, Format};
use crate::syntax::{LineState, can_hold_task, scan_line};

/// The crew folder when `_settings.md` doesn't name one.
pub const FOLDER: &str = "_crew";

/// The `_settings.md` frontmatter key that names the crew folder.
pub const SETTING: &str = "crew";

/// One person (or bot), from `_crew/<key>.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The file name without `.md`, lower case: what `@key` matches.
    pub key: String,
    /// The card's path in the vault.
    pub rel: String,
    pub name: String,
    pub role: String,
    /// The portrait's path in the vault (relative to the card, resolved).
    pub portrait: Option<String>,
    pub color: Option<[u8; 3]>,
    pub skills: Vec<String>,
    pub bot: bool,
    /// The card note exists (a person's folder may not have one yet).
    pub exists: bool,
}

/// What `@` matches for a name: lower case, spaces as `-`
/// ("Ika Putri" → `ika-putri`).
pub fn key_of(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
        .collect::<String>()
        .to_lowercase()
}

/// `folder/rest` → `rest` (the folder compared ignoring case).
fn under<'a>(rel: &'a str, folder: &str) -> Option<&'a str> {
    let folder = folder.trim_matches('/');
    if folder.is_empty() || rel.len() <= folder.len() + 1 || !rel.is_char_boundary(folder.len()) {
        return None;
    }
    let (head, tail) = rel.split_at(folder.len());
    (head.eq_ignore_ascii_case(folder) && tail.starts_with('/')).then(|| &tail[1..])
}

/// `name.md` → `name`.
fn stem(file: &str) -> Option<&str> {
    let n = file.len();
    (n > 3 && file.is_char_boundary(n - 3) && file[n - 3..].eq_ignore_ascii_case(".md")).then(|| &file[..n - 3])
}

/// Whether `rel` is a crew card in `folder`: a note directly in it
/// (`Members/ika.md`), or the note named like a person's folder
/// (`Members/Adam/Adam.md`).
pub fn is_card(rel: &str, folder: &str) -> bool {
    let Some(rest) = under(rel, folder) else { return false };
    match rest.split_once('/') {
        None => stem(rest).is_some(),
        Some((dir, file)) => !file.contains('/') && stem(file).is_some_and(|s| s.eq_ignore_ascii_case(dir)),
    }
}

/// One person found in the crew folder, before their card is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seat {
    pub key: String,
    /// Their card: `folder/name.md`, or `folder/Name/Name.md` for a
    /// person's folder (which may not exist yet).
    pub card: String,
    /// The folder's name, for a person's folder.
    pub folder_name: Option<String>,
}

/// Everyone in the crew folder: each note directly in it, and each folder
/// in it (a person's folder). `entries` are the vault's (path, is folder).
pub fn seats<'a>(folder: &str, entries: impl IntoIterator<Item = (&'a str, bool)>) -> Vec<Seat> {
    let mut out: Vec<Seat> = Vec::new();
    for (rel, is_folder) in entries {
        let Some(rest) = under(rel, folder) else { continue };
        if rest.contains('/') {
            continue;
        }
        let seat = if is_folder {
            Seat {
                key: key_of(rest),
                card: format!("{}/{rest}/{rest}.md", folder.trim_matches('/')),
                folder_name: Some(rest.to_owned()),
            }
        } else {
            let Some(name) = stem(rest) else { continue };
            Seat {
                key: key_of(name),
                card: rel.to_owned(),
                folder_name: None,
            }
        };
        if !seat.key.is_empty() && !out.iter().any(|s| s.key == seat.key) {
            out.push(seat);
        }
    }
    out
}

/// Where a new card for `@name` goes: a folder of its own when the crew
/// folder holds people's folders, else a note in it.
pub fn new_card_rel(folder: &str, name: &str, as_folders: bool) -> String {
    let folder = folder.trim_matches('/');
    if as_folders {
        format!("{folder}/{name}/{name}.md")
    } else {
        format!("{folder}/{}.md", name.to_lowercase())
    }
}

/// The crew folder named in `_settings.md` (`crew: Members`), or `_crew`.
pub fn folder_setting(settings: &str) -> String {
    frontmatter_value(settings, SETTING)
        .map(|v| v.replace('\\', "/").trim_matches('/').to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| FOLDER.to_owned())
}

/// A top-level `key: value` from the frontmatter, unquoted.
pub fn frontmatter_value(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim_start_matches('\u{feff}').trim_end() != "---" {
        return None;
    }
    for line in lines {
        if matches!(line.trim_end(), "---" | "...") {
            break;
        }
        if line.starts_with([' ', '\t']) {
            continue;
        }
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case(key)
        {
            return Some(unquote(v));
        }
    }
    None
}

/// Sets a top-level `key: value` in the frontmatter (replacing its line,
/// or adding it before the closing `---`), creating the frontmatter if
/// there is none. Everything else stays as it was.
pub fn set_frontmatter(text: &str, key: &str, value: &str) -> String {
    let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let plain = !value.is_empty()
        && !value.contains([':', '#', '"', '\''])
        && !value.starts_with([' ', '[', '{', '-', '&', '*', '!', '|', '>', '%', '@']);
    let quoted = if plain { value.to_owned() } else { format!("\"{}\"", value.replace('"', "\\\"")) };
    let entry = format!("{key}: {quoted}");
    let mut lines: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned()).collect();
    let has = lines.first().is_some_and(|l| l.trim_start_matches('\u{feff}').trim_end() == "---");
    if has && let Some(close) = (1..lines.len()).find(|&i| matches!(lines[i].trim_end(), "---" | "...")) {
        let found = (1..close).find(|&i| {
            !lines[i].starts_with([' ', '\t'])
                && lines[i].split_once(':').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key))
        });
        match found {
            Some(i) => lines[i] = entry,
            None => lines.insert(close, entry),
        }
        return lines.join(ending);
    }
    let mut out = vec!["---".to_owned(), entry, "---".to_owned()];
    if lines.len() == 1 && lines[0].is_empty() {
        out.push(String::new());
    } else {
        out.extend(lines);
    }
    out.join(ending)
}

/// What a new card holds.
pub fn template(name: &str) -> String {
    format!("---\nname: {name}\nrole: \nkind: human\n---\nNotes about {name}.\n")
}

/// `#e8826b` → RGB.
pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let hex = s.trim().trim_matches(['"', '\'']).trim_start_matches('#');
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// Up to two initials: "Ika Putri" → "IP", "unit7" → "UN".
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let out: String = match words.as_slice() {
        [] => String::new(),
        [one] => one.chars().take(2).collect(),
        [first, .., last] => first.chars().take(1).chain(last.chars().take(1)).collect(),
    };
    out.to_uppercase()
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(s)
        .to_owned()
}

/// Reads a card's frontmatter. Every field is optional.
pub fn card(rel: &str, text: &str) -> Member {
    let file = rel.rsplit('/').next().unwrap_or(rel);
    let stem = stem(file).unwrap_or(file);
    let mut m = Member {
        key: key_of(stem),
        rel: rel.to_owned(),
        name: stem.to_owned(),
        role: String::new(),
        portrait: None,
        color: None,
        skills: Vec::new(),
        bot: false,
        exists: true,
    };
    let mut lines = text.lines();
    if lines.next().map(|l| l.trim_start_matches('\u{feff}').trim_end()) != Some("---") {
        return m;
    }
    let mut list_key: Option<String> = None;
    for line in lines {
        if matches!(line.trim_end(), "---" | "...") {
            break;
        }
        if let Some(item) = line.trim_start().strip_prefix("- ")
            && line.starts_with([' ', '\t', '-'])
            && list_key.as_deref() == Some("skills")
        {
            m.skills.push(unquote(item));
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let key = key.trim().to_lowercase();
        let value = value.trim();
        list_key = value.is_empty().then(|| key.clone());
        match key.as_str() {
            "name" if !value.is_empty() => m.name = unquote(value),
            "role" => m.role = unquote(value),
            "color" | "colour" => m.color = parse_color(value),
            "kind" => m.bot = unquote(value).eq_ignore_ascii_case("bot"),
            "portrait" if !value.is_empty() => {
                let p = unquote(value);
                let dir = rel.rsplit_once('/').map_or("", |(d, _)| d);
                m.portrait = Some(if dir.is_empty() || p.contains('/') && !p.starts_with("./") {
                    p
                } else {
                    format!("{dir}/{}", p.trim_start_matches("./"))
                });
            }
            "skills" => {
                let v = value.trim_start_matches('[').trim_end_matches(']');
                m.skills = v.split(',').map(unquote).filter(|s| !s.is_empty()).collect();
            }
            _ => {}
        }
    }
    m
}

/// The `@name`s in a line: (byte range of `@name`, name in lower case).
/// The `@` has to start a word, so `bob@example.com` isn't one.
pub fn mentions(line: &str) -> Vec<(Range<usize>, String)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let starts_word = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || matches!(bytes[i - 1], b'_' | b'-' | b'.'));
        if bytes[i] == b'@' && starts_word {
            let len = bytes[i + 1..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                .count();
            if len > 0 {
                out.push((i..i + 1 + len, line[i + 1..i + 1 + len].to_lowercase()));
                i += 1 + len;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Every mention on a task line: (name, the task's status character).
/// Code, frontmatter and comments are skipped.
pub fn task_mentions(text: &str) -> Vec<(String, char)> {
    let mut out = Vec::new();
    let mut state = LineState::Start;
    for line in text.lines() {
        if can_hold_task(state)
            && let Some((_, ch)) = status::task_box(line)
        {
            for (_, name) in mentions(line) {
                out.push((name, ch));
            }
        }
        state = scan_line(state, line).1;
    }
    out
}

/// Adds `@name` to the end of a task's text (before a trailing `^id`).
/// `None` if the line already mentions them.
pub fn add_mention(line: &str, name: &str) -> Option<String> {
    let key = name.to_lowercase();
    if mentions(line).iter().any(|(_, n)| *n == key) {
        return None;
    }
    let body = line.trim_end();
    let cut = crate::live::trailing_block_id(body).unwrap_or(body.len());
    let (text, id) = body.split_at(cut);
    let text = text.trim_end();
    Some(format!("{text} @{name}{id}{}", &line[body.len()..]))
}

/// Mentions per person on tasks that aren't finished (not in their file's
/// last status).
pub fn open_counts<'a>(notes: impl IntoIterator<Item = (&'a [(String, char)], Format)>) -> HashMap<String, u32> {
    let mut out: HashMap<String, u32> = HashMap::new();
    for (mentions, format) in notes {
        for (name, ch) in mentions {
            if !format.is_finished(*ch) {
                *out.entry(name.clone()).or_default() += 1;
            }
        }
    }
    out
}

/// The status format a note's own text gives (its `statuses:`, then the
/// team's, then the default).
pub fn format_of(text: &str, team: Option<&Format>) -> Format {
    let lookup = status::from_frontmatter(text.lines());
    status::resolve(lookup.as_ref(), team).0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    /// 2× the others' median or more: a warning.
    Overloaded,
    /// 1.5× up to 2×: a compliment.
    Powerhouse,
}

impl Badge {
    pub fn label(self) -> &'static str {
        match self {
            Badge::Overloaded => "OVERLOADED",
            Badge::Powerhouse => "POWERHOUSE",
        }
    }
}

/// Each person's badge from everyone's mention counts: their count against
/// the median of everyone else's. None with fewer than 3 people, or when
/// the others' median is 0.
pub fn badges(counts: &[u32]) -> Vec<Option<Badge>> {
    if counts.len() < 3 {
        return vec![None; counts.len()];
    }
    (0..counts.len())
        .map(|i| {
            let mut others: Vec<u32> = counts.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, &c)| c).collect();
            others.sort_unstable();
            let n = others.len();
            let median = if n % 2 == 1 {
                f64::from(others[n / 2])
            } else {
                (f64::from(others[n / 2 - 1]) + f64::from(others[n / 2])) / 2.0
            };
            if median <= 0.0 {
                return None;
            }
            let ratio = f64::from(counts[i]) / median;
            if ratio >= 2.0 {
                Some(Badge::Overloaded)
            } else if ratio >= 1.5 {
                Some(Badge::Powerhouse)
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cards() {
        let m = card(
            "_crew/Ika.md",
            "---\nname: Ika Putri\nrole: Animator\nportrait: ika.png\ncolor: \"#e8826b\"\nskills: [anim, rig]\n---\nNotes.\n",
        );
        assert_eq!(m.key, "ika");
        assert_eq!(m.name, "Ika Putri");
        assert_eq!(m.role, "Animator");
        assert_eq!(m.portrait.as_deref(), Some("_crew/ika.png"));
        assert_eq!(m.color, Some([0xe8, 0x82, 0x6b]));
        assert_eq!(m.skills, ["anim", "rig"]);
        assert!(!m.bot);
        let bot = card("_crew/unit7.md", "---\nkind: bot\nskills:\n  - render\n  - \"qc\"\n---\n");
        assert!(bot.bot);
        assert_eq!(bot.name, "unit7");
        assert_eq!(bot.skills, ["render", "qc"]);
        assert_eq!(card("_crew/bo.md", "no frontmatter").name, "bo");
        assert!(is_card("_crew/bo.md", "_crew"));
        assert!(is_card("_Crew/Bo.MD", "_crew"));
        assert!(!is_card("_crew/old/bo.md", "_crew"));
        assert!(!is_card("crew/bo.md", "_crew"));
        assert_eq!(initials("Ika Putri"), "IP");
        assert_eq!(initials("unit7"), "UN");
    }

    #[test]
    fn crew_folder_and_people_as_folders() {
        let members = "Plan/Members";
        assert!(is_card("Plan/Members/Adam/Adam.md", members));
        assert!(is_card("plan/members/ika.md", members));
        assert!(!is_card("Plan/Members/Adam/Adam's Log.md", members));
        assert!(!is_card("Plan/Members/Adam/Log/Adam.md", members));
        assert!(!is_card("Plan/MembersX/ika.md", members));
        let entries = [
            ("Plan/Members", true),
            ("Plan/Members/Adam", true),
            ("Plan/Members/Adam/Adam's Log", true),
            ("Plan/Members/Adam/Adam's Dashboard & Tasks.md", false),
            ("Plan/Members/Ika Putri", true),
            ("Plan/Members/bo.md", false),
            ("Plan/Other/x.md", false),
        ];
        let seats = seats(members, entries);
        let keys: Vec<&str> = seats.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["adam", "ika-putri", "bo"]);
        assert_eq!(seats[0].card, "Plan/Members/Adam/Adam.md");
        assert_eq!(seats[0].folder_name.as_deref(), Some("Adam"));
        assert_eq!(seats[2].card, "Plan/Members/bo.md");
        assert_eq!(new_card_rel("Plan/Members/", "maya", true), "Plan/Members/maya/maya.md");
        assert_eq!(new_card_rel("_crew", "Maya", false), "_crew/maya.md");

        assert_eq!(folder_setting("---\ncrew: \"Plan/Members/\"\n---\n"), "Plan/Members");
        assert_eq!(folder_setting("# no frontmatter"), "_crew");
        let set = set_frontmatter("---\r\nstatuses:\r\n  - \"[ ] a\"\r\n---\r\nBody\r\n", "crew", "Plan/Members");
        assert_eq!(set, "---\r\nstatuses:\r\n  - \"[ ] a\"\r\ncrew: Plan/Members\r\n---\r\nBody\r\n");
        assert_eq!(set_frontmatter(&set, "crew", "_crew"), set.replace("Plan/Members", "_crew"));
        assert_eq!(set_frontmatter("# Settings\n", "crew", "A B"), "---\ncrew: A B\n---\n# Settings\n");
        assert_eq!(set_frontmatter("", "crew", "x"), "---\ncrew: x\n---\n");
    }

    #[test]
    fn finds_mentions() {
        let m: Vec<String> = mentions("- [ ] Comp @Ika and @bo-2, mail bob@example.com @").into_iter().map(|(_, n)| n).collect();
        assert_eq!(m, ["ika", "bo-2"]);
        assert_eq!(mentions("@sam: ok")[0].0, 0..4);
        let text = "---\nowner: @x\n---\n- [ ] One @ika\n- [x] Two @ika @bo\nNot a task @maya\n```\n- [ ] Code @ika\n```\n";
        assert_eq!(
            task_mentions(text),
            [("ika".to_owned(), ' '), ("ika".to_owned(), 'x'), ("bo".to_owned(), 'x')]
        );
        let format = Format::builtin();
        let counts = open_counts([(task_mentions(text).as_slice(), format)]);
        assert_eq!(counts.get("ika"), Some(&1));
        assert_eq!(counts.get("bo"), None);
    }

    #[test]
    fn adds_mentions() {
        assert_eq!(add_mention("- [ ] Comp", "ika").as_deref(), Some("- [ ] Comp @ika"));
        assert_eq!(add_mention("- [ ] Comp ^c1\r", "ika").as_deref(), Some("- [ ] Comp @ika ^c1\r"));
        assert_eq!(add_mention("- [ ] Comp @Ika", "ika"), None);
        assert_eq!(add_mention("- [ ]", "bo").as_deref(), Some("- [ ] @bo"));
    }

    #[test]
    fn badges_follow_the_median() {
        use Badge::*;
        // 4 against median(2, 2, 3) = 2: 2×, overloaded. 3 against
        // median(4, 2, 2) = 2: 1.5×, powerhouse.
        assert_eq!(badges(&[4, 2, 2, 3]), [Some(Overloaded), None, None, Some(Powerhouse)]);
        // 3 against median(2, 2) = 2 → 1.5×.
        assert_eq!(badges(&[3, 2, 2]), [Some(Powerhouse), None, None]);
        assert_eq!(badges(&[9, 1]), [None, None]);
        assert_eq!(badges(&[5, 0, 0]), [None, None, None]);
    }
}
