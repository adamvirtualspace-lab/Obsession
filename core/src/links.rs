//! Links between notes: finding them in a note, the vault-wide index that
//! answers "what links here?", and rewriting them when a note is renamed.
//!
//! The index keeps, per note, only its outgoing links (target, line, a short
//! snippet of the line) and its headings and block ids, so it costs a small
//! fraction of the vault's size. It's built once when a vault opens, on a
//! background thread, and kept current as notes are saved.
//!
//! The index is cached in `.obsessed/cache/links.txt` with each note's
//! modified time and size, so the next start only reads notes that changed
//! ([`LinkIndex::build_cached`], [`LinkIndex::to_cache`]).

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use crate::syntax::{Kind, LineState, scan_line};
use crate::vault::{Resolved, Vault};

/// Where the index cache lives, from the vault root.
pub const CACHE: &str = ".obsessed/cache/links.txt";

/// Writes the index cache (creating `.obsessed/` with its `.gitignore`).
pub fn save_cache(root: &Path, index: &LinkIndex) -> std::io::Result<()> {
    crate::log::ensure(root)?;
    let path = root.join(CACHE);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::write::atomic_write(&path, index.to_cache().as_bytes())
}

/// One link in a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// 0-based line.
    pub line: usize,
    /// Bytes within the line, from `!`/`[` to the closing `]]` or `)`.
    pub range: Range<usize>,
    /// `![[...]]` or `![](...)`.
    pub embed: bool,
    /// `[[...]]` (otherwise a markdown `[label](url)` link).
    pub wiki: bool,
    /// The note part, as written (`anim/sh040`), `%20` decoded for markdown
    /// links. Empty for a link to a heading in the same note (`[[#Notes]]`).
    pub target: String,
    /// What follows the first `#`: a heading path or a `^block` id.
    pub sub: Option<String>,
    /// `[[target|alias]]`'s alias, or a markdown link's label.
    pub label: Option<String>,
}

/// How a link stands, for drawing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Ok,
    /// Several notes have that name; a click opens the one Obsidian would.
    Ambiguous,
    /// No such note; a click creates it.
    Missing,
    /// The note exists but not the `#heading` or `^block`.
    MissingSection,
}

/// Headings and block ids of a note, for checking `#heading` and `^block`
/// links and jumping to them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outline {
    /// (normalized heading text, line)
    pub headings: Vec<(String, usize)>,
    /// (lowercased id without `^`, line)
    pub blocks: Vec<(String, usize)>,
}

impl Outline {
    /// The line a `#heading`, `#a#b` (last part counts) or `#^block` points
    /// at, if the note has it.
    pub fn find(&self, sub: &str) -> Option<usize> {
        let sub = sub.trim();
        if let Some(id) = sub.strip_prefix('^') {
            let id = id.trim().to_lowercase();
            return self.blocks.iter().find(|(b, _)| *b == id).map(|&(_, line)| line);
        }
        let last = sub.rsplit('#').next().unwrap_or(sub);
        let key = normalize_heading(last);
        if key.is_empty() {
            return Some(0);
        }
        self.headings.iter().find(|(h, _)| *h == key).map(|&(_, line)| line)
    }
}

/// Heading text as links match it: lowercased, with the characters Obsidian
/// drops from heading links removed and spaces collapsed.
pub fn normalize_heading(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if matches!(c, '#' | '|' | '^' | ':' | '[' | ']' | '%') { ' ' } else { c })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The key a link is looked up by when drawing it: the part before `|`,
/// lowercased and trimmed (`Anim/Sh040 # Notes` → `anim/sh040#notes`).
pub fn dest_key(dest: &str) -> String {
    let dest = dest.split('|').next().unwrap_or(dest);
    match dest.split_once('#') {
        Some((target, sub)) => format!("{}#{}", target.trim(), sub.trim()).to_lowercase(),
        None => dest.trim().to_lowercase(),
    }
}

/// Every link in `text`, skipping frontmatter, code and `%%` comments.
pub fn links(text: &str) -> Vec<Link> {
    let mut out = Vec::new();
    let mut state = LineState::Start;
    for (index, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let (spans, next) = scan_line(state, line);
        let skip_line = matches!(state, LineState::Frontmatter | LineState::Fence { .. })
            || (state == LineState::Start && next == LineState::Frontmatter)
            || matches!(next, LineState::Fence { .. }) && !matches!(state, LineState::Fence { .. });
        state = next;
        if skip_line {
            continue;
        }
        let skip: Vec<Range<usize>> = spans
            .into_iter()
            .filter(|(_, kind)| matches!(kind, Kind::Code | Kind::Comment))
            .map(|(range, _)| range)
            .collect();
        line_links(index, line, &skip, &mut out);
    }
    out
}

fn line_links(index: usize, line: &str, skip: &[Range<usize>], out: &mut Vec<Link>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(range) = skip.iter().find(|r| r.contains(&i)) {
            i = range.end.max(i + 1);
            continue;
        }
        let embed = bytes[i] == b'!';
        let open = if embed { i + 1 } else { i };
        if bytes.get(open) != Some(&b'[') {
            i += 1;
            continue;
        }
        // Wiki link.
        if line[open..].starts_with("[[") {
            let Some(close) = line[open + 2..].find("]]") else {
                i = open + 2;
                continue;
            };
            let inner = &line[open + 2..open + 2 + close];
            let end = open + 2 + close + 2;
            let (dest, alias) = match inner.split_once('|') {
                Some((dest, alias)) => (dest, Some(alias.trim().to_owned())),
                None => (inner, None),
            };
            let (target, sub) = match dest.split_once('#') {
                Some((target, sub)) => (target, Some(sub.trim().to_owned())),
                None => (dest, None),
            };
            if !target.trim().is_empty() || sub.is_some() {
                out.push(Link {
                    line: index,
                    range: i..end,
                    embed,
                    wiki: true,
                    target: target.trim().to_owned(),
                    sub,
                    label: alias,
                });
            }
            i = end;
            continue;
        }
        // Markdown link: [label](url)
        if let Some((label, url, end)) = markdown_link(line, open) {
            if let Some((target, sub)) = note_url(url) {
                out.push(Link {
                    line: index,
                    range: i..end,
                    embed,
                    wiki: false,
                    target,
                    sub,
                    label: Some(label.to_owned()),
                });
            }
            i = end;
            continue;
        }
        i = open + 1;
    }
}

/// `[label](url)` starting at `open` (the `[`): (label, url, end).
fn markdown_link(line: &str, open: usize) -> Option<(&str, &str, usize)> {
    let rest = &line[open + 1..];
    let close = rest.find(']')?;
    let label = &rest[..close];
    let after = &rest[close + 1..];
    let inner = after.strip_prefix('(')?;
    let (url, used) = if let Some(angle) = inner.strip_prefix('<') {
        let end = angle.find('>')?;
        angle[end + 1..].strip_prefix(')')?;
        (&angle[..end], end + 3)
    } else {
        let end = inner.find(')')?;
        (inner[..end].split(' ').next().unwrap_or(""), end + 1)
    };
    let end = open + 1 + close + 1 + 1 + used;
    Some((label, url.trim(), end))
}

/// A markdown link's URL as (note path, `#sub`), or `None` for web links,
/// mail links and anchors.
pub fn note_url(url: &str) -> Option<(String, Option<String>)> {
    if url.is_empty() || url.starts_with('#') || url.contains("://") || url.starts_with("mailto:") {
        return None;
    }
    let decoded = percent_decode(url);
    let (target, sub) = match decoded.split_once('#') {
        Some((t, s)) => (t.to_owned(), Some(s.to_owned())),
        None => (decoded, None),
    };
    Some((target, sub))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

fn percent_encode(s: &str) -> String {
    s.replace('%', "%25").replace(' ', "%20")
}

/// Headings and block ids in `text` (outside code and frontmatter).
pub fn outline(text: &str) -> Outline {
    let mut out = Outline::default();
    let mut state = LineState::Start;
    for (index, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let before = state;
        let (spans, next) = scan_line(state, line);
        state = next;
        if !matches!(before, LineState::Start | LineState::Normal) {
            continue;
        }
        if spans.first().is_some_and(|(_, k)| *k == Kind::Heading) {
            let text = line.trim_start().trim_start_matches('#').trim_end_matches('#');
            out.headings.push((normalize_heading(text), index));
        }
        if let Some(cut) = crate::live::trailing_block_id(line) {
            let id = line[cut..].trim().trim_start_matches('^').to_lowercase();
            out.blocks.push((id, index));
        }
    }
    out
}

/// Which task a `#^block` or `#heading` link into `text` points at, for
/// ending a graph wire at that task's row. `tasks` holds the note's task
/// lines in order; the answer is an index into it.
///
/// A block id points at the task on its own line. A heading points at the
/// first task in its section, which runs until the next heading of the
/// same or a higher level. `None` when the note has no such block or
/// heading, or no task there (the wire then ends at the card).
pub fn task_at(text: &str, sub: &str, tasks: &[usize]) -> Option<usize> {
    let sub = sub.trim();
    if !sub.starts_with('^') && normalize_heading(sub.rsplit('#').next().unwrap_or(sub)).is_empty() {
        return None;
    }
    let outline = outline(text);
    let line = outline.find(sub)?;
    if sub.starts_with('^') {
        return tasks.iter().position(|&t| t == line);
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let level = |l: usize| {
        let t = lines.get(l).copied().unwrap_or("").trim_start();
        t.bytes().take_while(|&b| b == b'#').count()
    };
    let own = level(line);
    let end = outline
        .headings
        .iter()
        .map(|&(_, l)| l)
        .find(|&l| l > line && level(l) <= own)
        .unwrap_or(usize::MAX);
    tasks.iter().position(|&t| t > line && t < end)
}

/// The state of every link in `text` (a note at `rel`), keyed by
/// [`dest_key`]. Links to the note's own headings use `own`.
pub fn link_states(
    text: &str,
    rel: &str,
    vault: &Vault,
    index: &LinkIndex,
) -> HashMap<String, LinkState> {
    let own = outline(text);
    let mut states = HashMap::new();
    for link in links(text) {
        if !link.wiki {
            continue;
        }
        let key = match &link.sub {
            Some(sub) => dest_key(&format!("{}#{sub}", link.target)),
            None => dest_key(&link.target),
        };
        if states.contains_key(&key) {
            continue;
        }
        let state = if link.target.is_empty() {
            section_state(&own, link.sub.as_deref())
        } else {
            match vault.resolve_from(&link.target, Some(rel)) {
                Resolved::NotFound => LinkState::Missing,
                Resolved::Attachment => LinkState::Ok,
                Resolved::Ambiguous { .. } => LinkState::Ambiguous,
                Resolved::One(entry) => {
                    let outline = if *entry.rel == *rel { Some(&own) } else { index.outline(&entry.rel) };
                    match outline {
                        Some(outline) => section_state(outline, link.sub.as_deref()),
                        // Not indexed yet: can't tell, so don't warn.
                        None => LinkState::Ok,
                    }
                }
            }
        };
        states.insert(key, state);
    }
    states
}

fn section_state(outline: &Outline, sub: Option<&str>) -> LinkState {
    match sub {
        Some(sub) if outline.find(sub).is_none() => LinkState::MissingSection,
        _ => LinkState::Ok,
    }
}

/// A link to a note, as the backlinks panel lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlink {
    pub source: Box<str>,
    pub line: usize,
    pub snippet: Box<str>,
}

#[derive(Debug, Clone, Default)]
struct Indexed {
    links: Vec<IndexedLink>,
    outline: Outline,
    /// `@name`s on task lines, with the task's status.
    mentions: Vec<(String, char)>,
    /// The note's own `statuses:`, if it has them.
    format: Option<crate::status::Format>,
    /// The file's modified time (ns since 1970) and size when it was read,
    /// for the cache. `None` when indexed from text being edited.
    stamp: Option<(u64, u64)>,
}

/// How a cached build went.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuildStats {
    /// Notes read and parsed.
    pub parsed: usize,
    /// Notes taken from the cache.
    pub cached: usize,
}

fn stamp_of(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some((modified.as_nanos() as u64, meta.len()))
}

/// Escapes a cache field (no tabs or line breaks inside).
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Reads one note's text into what the index keeps.
fn index_text(text: &str) -> Indexed {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut found = Vec::new();
    for link in links(text) {
        if link.target.is_empty() {
            continue;
        }
        let line = lines.get(link.line).copied().unwrap_or("").trim();
        let snippet = match line.char_indices().nth(SNIPPET) {
            Some((cut, _)) => format!("{}…", &line[..cut]),
            None => line.to_owned(),
        };
        found.push(IndexedLink {
            target: link.target.into(),
            line: link.line as u32,
            snippet: snippet.into(),
        });
    }
    Indexed {
        links: found,
        outline: outline(text),
        mentions: crate::crew::task_mentions(text),
        format: crate::status::from_frontmatter(text.lines()).and_then(|l| l.format),
        stamp: None,
    }
}

/// Reads cache text back into notes.
fn parse_cache(text: &str) -> HashMap<String, Indexed> {
    let mut out: HashMap<String, Indexed> = HashMap::new();
    let mut lines = text.lines();
    if lines.next() != Some("obsession-links 1") {
        return out;
    }
    let mut current: Option<(String, Indexed)> = None;
    let mut statuses: Vec<crate::status::Status> = Vec::new();
    fn finish(
        current: &mut Option<(String, Indexed)>,
        statuses: &mut Vec<crate::status::Status>,
        out: &mut HashMap<String, Indexed>,
    ) {
        if let Some((rel, mut note)) = current.take() {
            if !statuses.is_empty() {
                note.format = crate::status::Format::from_statuses(std::mem::take(statuses));
            }
            out.insert(rel, note);
        }
    }
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        if let ["N", rel, modified, len] = fields.as_slice() {
            finish(&mut current, &mut statuses, &mut out);
            if let (Ok(m), Ok(l)) = (modified.parse(), len.parse()) {
                current = Some((
                    unesc(rel),
                    Indexed {
                        stamp: Some((m, l)),
                        ..Indexed::default()
                    },
                ));
            }
            continue;
        }
        let Some((_, note)) = &mut current else { continue };
        match fields.as_slice() {
            ["L", line, target, snippet] => {
                if let Ok(line) = line.parse() {
                    note.links.push(IndexedLink {
                        target: unesc(target).into(),
                        line,
                        snippet: unesc(snippet).into(),
                    });
                }
            }
            ["H", line, h] => {
                if let Ok(line) = line.parse() {
                    note.outline.headings.push((unesc(h), line));
                }
            }
            ["B", line, b] => {
                if let Ok(line) = line.parse() {
                    note.outline.blocks.push((unesc(b), line));
                }
            }
            ["M", ch, name] => {
                if let Some(c) = unesc(ch).chars().next() {
                    note.mentions.push((unesc(name), c));
                }
            }
            ["F", ch, symbol, label] => {
                if let Some(c) = unesc(ch).chars().next() {
                    statuses.push(crate::status::Status {
                        ch: c,
                        symbol: unesc(symbol).into(),
                        label: unesc(label).into(),
                    });
                }
            }
            _ => {}
        }
    }
    finish(&mut current, &mut statuses, &mut out);
    out
}

#[derive(Debug, Clone)]
struct IndexedLink {
    target: Box<str>,
    line: u32,
    snippet: Box<str>,
}

/// The longest snippet kept per link.
const SNIPPET: usize = 160;

/// Who links to whom, across the vault.
#[derive(Debug, Clone, Default)]
pub struct LinkIndex {
    notes: HashMap<Box<str>, Indexed>,
    /// A link target's lowercased name -> notes with such links.
    by_name: HashMap<String, Vec<Box<str>>>,
}

/// The lowercased name a target would match (`anim/Sh040.md` → `sh040`).
fn name_key(target: &str) -> String {
    let name = target.rsplit(['/', '\\']).next().unwrap_or(target);
    let name = if name.len() > 3 && name[name.len() - 3..].eq_ignore_ascii_case(".md") {
        &name[..name.len() - 3]
    } else {
        name
    };
    name.trim().to_lowercase()
}

impl LinkIndex {
    /// Reads every note once and indexes it. Unreadable notes are skipped.
    pub fn build<'a>(root: &Path, notes: impl IntoIterator<Item = &'a str>) -> Self {
        Self::build_cached(root, notes, None).0
    }

    /// Like [`LinkIndex::build`], but a note whose modified time and size
    /// match `cache` (from [`LinkIndex::to_cache`]) is taken from it, not
    /// read. Only changed and new notes are read from disk.
    pub fn build_cached<'a>(
        root: &Path,
        notes: impl IntoIterator<Item = &'a str>,
        cache: Option<&str>,
    ) -> (Self, BuildStats) {
        let mut cached = cache.map(parse_cache).unwrap_or_default();
        let mut index = Self::default();
        let mut stats = BuildStats::default();
        for rel in notes {
            let path = root.join(rel);
            let stamp = stamp_of(&path);
            if let Some(old) = cached.remove(rel)
                && old.stamp.is_some()
                && old.stamp == stamp
            {
                index.insert(rel, old);
                stats.cached += 1;
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                let mut indexed = index_text(&text);
                indexed.stamp = stamp;
                index.insert(rel, indexed);
                stats.parsed += 1;
            }
        }
        (index, stats)
    }

    /// Records that `rel` was just written to disk with what the index
    /// holds, so the cache can trust it next time.
    pub fn stamp(&mut self, root: &Path, rel: &str) {
        if let Some(note) = self.notes.get_mut(rel) {
            note.stamp = stamp_of(&root.join(rel));
        }
    }

    /// The index as cache text. Notes indexed from unsaved text (no stamp)
    /// are left out, so they're read again next time.
    pub fn to_cache(&self) -> String {
        let mut out = String::from("obsession-links 1\n");
        let mut rels: Vec<&Box<str>> = self.notes.keys().collect();
        rels.sort();
        for rel in rels {
            let note = &self.notes[rel];
            let Some((modified, len)) = note.stamp else { continue };
            out.push_str(&format!("N\t{}\t{modified}\t{len}\n", esc(rel)));
            for l in &note.links {
                out.push_str(&format!("L\t{}\t{}\t{}\n", l.line, esc(&l.target), esc(&l.snippet)));
            }
            for (h, line) in &note.outline.headings {
                out.push_str(&format!("H\t{line}\t{}\n", esc(h)));
            }
            for (b, line) in &note.outline.blocks {
                out.push_str(&format!("B\t{line}\t{}\n", esc(b)));
            }
            for (name, ch) in &note.mentions {
                out.push_str(&format!("M\t{}\t{}\n", esc(&ch.to_string()), esc(name)));
            }
            if let Some(format) = &note.format {
                for s in format.statuses() {
                    out.push_str(&format!("F\t{}\t{}\t{}\n", esc(&s.ch.to_string()), esc(&s.symbol), esc(&s.label)));
                }
            }
        }
        out
    }

    fn insert(&mut self, rel: &str, indexed: Indexed) {
        self.remove(rel);
        for link in &indexed.links {
            let sources = self.by_name.entry(name_key(&link.target)).or_default();
            if sources.last().is_none_or(|last| **last != *rel) {
                sources.push(rel.into());
            }
        }
        self.notes.insert(rel.into(), indexed);
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// Re-indexes one note from its text.
    pub fn update(&mut self, rel: &str, text: &str) {
        self.insert(rel, index_text(text));
    }

    pub fn remove(&mut self, rel: &str) {
        let Some(old) = self.notes.remove(rel) else {
            return;
        };
        for link in &old.links {
            let key = name_key(&link.target);
            if let Some(sources) = self.by_name.get_mut(&key) {
                sources.retain(|s| **s != *rel);
                if sources.is_empty() {
                    self.by_name.remove(&key);
                }
            }
        }
    }

    /// The link targets in `rel`, as written.
    pub fn targets(&self, rel: &str) -> impl Iterator<Item = &str> {
        self.notes
            .get(rel)
            .into_iter()
            .flat_map(|n| n.links.iter().map(|l| &*l.target))
    }

    /// Mentions per person on unfinished tasks across the vault (or only in
    /// `only`), each note judged by its own status format.
    pub fn mention_counts(&self, team: Option<&crate::status::Format>, only: Option<&[&str]>) -> HashMap<String, u32> {
        let builtin = crate::status::Format::builtin();
        let mut out: HashMap<String, u32> = HashMap::new();
        for (rel, note) in &self.notes {
            if only.is_some_and(|o| !o.contains(&&**rel)) {
                continue;
            }
            let format = note.format.as_ref().or(team).unwrap_or(&builtin);
            for (name, ch) in &note.mentions {
                if !format.is_finished(*ch) {
                    *out.entry(name.clone()).or_default() += 1;
                }
            }
        }
        out
    }

    /// Every name mentioned on a task anywhere.
    pub fn mentioned(&self) -> std::collections::BTreeSet<&str> {
        self.notes.values().flat_map(|n| n.mentions.iter().map(|(m, _)| m.as_str())).collect()
    }

    pub fn outline(&self, rel: &str) -> Option<&Outline> {
        self.notes.get(rel).map(|n| &n.outline)
    }

    /// Every link that leads to `rel`, by note then line.
    pub fn backlinks(&self, rel: &str, vault: &Vault) -> Vec<Backlink> {
        let name = name_key(rel);
        let mut out = Vec::new();
        let Some(sources) = self.by_name.get(&name) else {
            return out;
        };
        let mut sources: Vec<&Box<str>> = sources.iter().collect();
        sources.sort_by_key(|s| s.to_lowercase());
        sources.dedup();
        for source in sources {
            let Some(indexed) = self.notes.get(source) else {
                continue;
            };
            for link in &indexed.links {
                if name_key(&link.target) != name {
                    continue;
                }
                let hit = vault
                    .resolve_from(&link.target, Some(source))
                    .entry()
                    .is_some_and(|e| *e.rel == *rel);
                // One entry per line, however many links it has.
                let seen = out
                    .last()
                    .is_some_and(|b: &Backlink| b.source == *source && b.line == link.line as usize);
                if hit && !seen {
                    out.push(Backlink {
                        source: source.clone(),
                        line: link.line as usize,
                        snippet: link.snippet.clone(),
                    });
                }
            }
        }
        out
    }

    /// The notes with at least one link to `rel`.
    pub fn linking_notes(&self, rel: &str, vault: &Vault) -> Vec<Box<str>> {
        let mut notes: Vec<Box<str>> = self.backlinks(rel, vault).into_iter().map(|b| b.source).collect();
        notes.dedup();
        notes
    }
}

/// Rewrites every link in `text` (the note at `source`) that leads to
/// `old_rel` so it leads to `new_rel` instead, keeping `#sections`, aliases,
/// labels and `!` embeds. Wiki links get the shortest form that's still
/// unique (the name alone unless another note shares it), as Obsidian does;
/// markdown links get the same, with `.md` and spaces as `%20`.
///
/// `vault` is the vault before the rename. Returns the new text and how many
/// links changed, or `None` if nothing did.
pub fn rewrite_links(
    text: &str,
    source: &str,
    vault: &Vault,
    old_rel: &str,
    new_rel: &str,
) -> Option<(String, usize)> {
    let new_stem = new_rel.strip_suffix(".md").unwrap_or(new_rel);
    let new_name = new_stem.rsplit('/').next().unwrap_or(new_stem);
    // Unique once the old note is gone (it may have had the same name).
    let others = vault.count_named(new_name)
        - usize::from(name_key(old_rel) == new_name.to_lowercase() && vault.note(old_rel).is_some());
    let shortest = if others == 0 { new_name } else { new_stem };

    let mut edits: Vec<(usize, Range<usize>, String)> = Vec::new();
    for link in links(text) {
        if link.target.is_empty() {
            continue;
        }
        let leads_there = vault
            .resolve_from(&link.target, Some(source))
            .entry()
            .is_some_and(|e| *e.rel == *old_rel);
        if !leads_there {
            continue;
        }
        let bang = if link.embed { "!" } else { "" };
        let sub = link.sub.as_deref().map(|s| format!("#{s}")).unwrap_or_default();
        let new = if link.wiki {
            let alias = link.label.as_deref().map(|a| format!("|{a}")).unwrap_or_default();
            format!("{bang}[[{shortest}{sub}{alias}]]")
        } else {
            let label = link.label.as_deref().unwrap_or("");
            format!("{bang}[{label}]({}.md{})", percent_encode(shortest), percent_encode(&sub))
        };
        edits.push((link.line, link.range, new));
    }
    if edits.is_empty() {
        return None;
    }
    let count = edits.len();
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    // Back to front within a line, so earlier ranges stay valid.
    for (line, range, new) in edits.into_iter().rev() {
        if let Some(l) = lines.get_mut(line) {
            l.replace_range(range, &new);
        }
    }
    Some((lines.join("\n"), count))
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn cache_round_trip_reads_only_changed_notes() {
        let root = std::env::temp_dir().join(format!("obsession-links-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let a = "---\nstatuses:\n  - \"[ ] 🔳 to do\"\n  - \"[x] ✅ done\"\n---\n# Head\\Tab\nSee [[b]] and [[c|C]].\n- [ ] Task @ika ^t1\n";
        std::fs::write(root.join("a.md"), a).unwrap();
        std::fs::write(root.join("b.md"), "[[a#Head]]\n").unwrap();
        let (index, stats) = LinkIndex::build_cached(&root, ["a.md", "b.md"], None);
        assert_eq!(stats, BuildStats { parsed: 2, cached: 0 });
        let cache = index.to_cache();

        // Nothing changed: everything comes from the cache, and it's the same.
        let (again, stats) = LinkIndex::build_cached(&root, ["a.md", "b.md"], Some(&cache));
        assert_eq!(stats, BuildStats { parsed: 0, cached: 2 });
        assert_eq!(again.to_cache(), cache);
        assert_eq!(again.targets("a.md").collect::<Vec<_>>(), ["b", "c"]);
        assert_eq!(again.outline("a.md"), index.outline("a.md"));
        assert_eq!(again.mention_counts(None, None).get("ika"), Some(&1));
        assert_eq!(again.notes["a.md"].format, index.notes["a.md"].format);
        assert!(again.notes["a.md"].format.is_some());

        // One note changed (a different size): only it is read.
        std::fs::write(root.join("b.md"), "[[a]] and more\n").unwrap();
        let (_, stats) = LinkIndex::build_cached(&root, ["a.md", "b.md"], Some(&cache));
        assert_eq!(stats, BuildStats { parsed: 1, cached: 1 });
        // A garbled cache is ignored.
        let (_, stats) = LinkIndex::build_cached(&root, ["a.md", "b.md"], Some("junk"));
        assert_eq!(stats.parsed, 2);
        assert_eq!(unesc(&esc("a\tb\\c\nd")), "a\tb\\c\nd");
        std::fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::tests::vault;

    #[test]
    fn finds_links() {
        let text = "---\nup: [[not this]]\n---\nSee [[anim/sh040#Notes|Shot]] and ![[board.png]].\n\
                    `[[code]]` %% [[hidden]] %%\n```\n[[fenced]]\n```\n[brief](notes/the%20brief.md#^a1) [web](https://x.com)\n\
                    ## Head [[in heading]]";
        let found = links(text);
        let targets: Vec<&str> = found.iter().map(|l| l.target.as_str()).collect();
        assert_eq!(targets, ["anim/sh040", "board.png", "notes/the brief.md", "in heading"]);
        let first = &found[0];
        assert_eq!((first.line, first.sub.as_deref(), first.label.as_deref()), (3, Some("Notes"), Some("Shot")));
        assert_eq!(&"See [[anim/sh040#Notes|Shot]] and"[first.range.clone()], "[[anim/sh040#Notes|Shot]]");
        assert!(found[1].embed);
        assert!(!found[2].wiki);
        assert_eq!(found[2].sub.as_deref(), Some("^a1"));
    }

    #[test]
    fn outline_and_sections() {
        let o = outline("# Shot 040\nText ^intro\n## Notes: lighting\n```\n# not a heading\n```\n  ### Indented");
        assert_eq!(o.find("Shot 040"), Some(0));
        assert_eq!(o.find("notes lighting"), Some(2));
        assert_eq!(o.find("Shot 040#Notes: Lighting"), Some(2));
        assert_eq!(o.find("^INTRO"), Some(1));
        assert_eq!(o.find("Indented"), Some(6));
        assert_eq!(o.find("not a heading"), None);
    }

    #[test]
    fn a_link_into_a_note_finds_its_task() {
        let text = "# Shot\n- [ ] block out ^t1\n## Notes\nSome text\n### Lighting\n- [ ] key light ^t2\n- [x] rim\n## Done\n- [x] wrap\n```\n# not a heading\n```\n## Empty\ntext ^p1";
        let tasks = [1, 5, 6, 8];
        assert_eq!(task_at(text, "^t2", &tasks), Some(1));
        assert_eq!(task_at(text, "^T1", &tasks), Some(0));
        // A heading: the first task in its section, subsections included.
        assert_eq!(task_at(text, "Notes", &tasks), Some(1));
        assert_eq!(task_at(text, "Shot#Lighting", &tasks), Some(1));
        assert_eq!(task_at(text, "Done", &tasks), Some(3));
        // The top heading's section holds everything.
        assert_eq!(task_at(text, "Shot", &tasks), Some(0));
        // No task there, no such place, or the whole note: the card.
        assert_eq!(task_at(text, "Empty", &tasks), None);
        assert_eq!(task_at(text, "^p1", &tasks), None);
        assert_eq!(task_at(text, "Nope", &tasks), None);
        assert_eq!(task_at(text, "^nope", &tasks), None);
        assert_eq!(task_at(text, "", &tasks), None);
    }

    #[test]
    fn states() {
        let v = vault(&["a/x.md", "b/x.md", "brief.md"]);
        let mut index = LinkIndex::default();
        index.update("brief.md", "# Scope\ntext");
        let text = "# Top\n[[x]] [[brief#Scope]] [[brief#Nope]] [[gone]] [[#Top]] [[#Bottom]] [[Brief | B]]";
        let s = link_states(text, "n.md", &v, &index);
        assert_eq!(s["x"], LinkState::Ambiguous);
        assert_eq!(s["brief#scope"], LinkState::Ok);
        assert_eq!(s["brief#nope"], LinkState::MissingSection);
        assert_eq!(s["gone"], LinkState::Missing);
        assert_eq!(s["#top"], LinkState::Ok);
        assert_eq!(s["#bottom"], LinkState::MissingSection);
        assert_eq!(s[&dest_key("Brief | B")], LinkState::Ok);
    }

    #[test]
    fn backlinks_follow_resolution() {
        let v = vault(&["a/x.md", "b/x.md", "one.md", "two.md"]);
        let mut index = LinkIndex::default();
        index.update("one.md", "to [[x]] and [[x|again]]\nand [[b/x|bx]]");
        index.update("b/two.md", "to [[x]]");
        index.update("two.md", "nothing");
        // From one.md, [[x]] opens a/x (shortest path); [[b/x]] is explicit.
        let to_a: Vec<_> = index.backlinks("a/x.md", &v).into_iter().map(|b| (b.source, b.line)).collect();
        assert_eq!(to_a, [("one.md".into(), 0)]);
        let to_b: Vec<_> = index.backlinks("b/x.md", &v).into_iter().map(|b| (b.source, b.line)).collect();
        // b/two.md is in b/, so its [[x]] means b/x.
        assert_eq!(to_b, [("b/two.md".into(), 0), ("one.md".into(), 1)]);
        index.update("one.md", "no links now");
        assert_eq!(index.backlinks("a/x.md", &v), []);
    }

    #[test]
    fn rewrite_on_rename() {
        let v = vault(&["anim/sh040.md", "notes/sh041.md", "other/sh041.md", "brief.md"]);
        let text = "[[sh040]] [[anim/sh040#Notes|Shot]] ![[sh040]] [s](anim/sh040.md) [[brief]]";
        let (new, count) = rewrite_links(text, "brief.md", &v, "anim/sh040.md", "anim/Shot 40.md").unwrap();
        assert_eq!(count, 4);
        assert_eq!(new, "[[Shot 40]] [[Shot 40#Notes|Shot]] ![[Shot 40]] [s](Shot%2040.md) [[brief]]");
        // A name another note has: keep the path.
        let (new, _) = rewrite_links("[[sh040]]", "brief.md", &v, "anim/sh040.md", "anim/sh041.md").unwrap();
        assert_eq!(new, "[[anim/sh041]]");
        assert_eq!(rewrite_links("[[brief]]", "x.md", &v, "anim/sh040.md", "a.md"), None);
    }
}
