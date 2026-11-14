//! Scanning a vault folder.
//!
//! The vault is kept as one flat, sorted list of entries (folders first, then
//! notes, case-insensitive). The file tree draws straight from it, which keeps
//! memory small and makes virtual scrolling trivial.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One folder or note in the vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path from the vault root, with `/` separators. Notes keep `.md`.
    pub rel: Box<str>,
    /// Nesting depth: 0 for items at the vault root.
    pub depth: u16,
    pub kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Note,
}

impl Entry {
    /// The last path component, e.g. `sh040.md`.
    pub fn name(&self) -> &str {
        self.rel.rsplit('/').next().unwrap_or(&self.rel)
    }

    /// The name without `.md`, as shown in the tree and in links.
    pub fn title(&self) -> &str {
        let name = self.name();
        match self.kind {
            Kind::Note => name.strip_suffix(".md").unwrap_or(name),
            Kind::Folder => name,
        }
    }

    /// The parent folder's `rel`, or `""` at the root.
    pub fn parent(&self) -> &str {
        self.rel.rsplit_once('/').map_or("", |(parent, _)| parent)
    }
}

#[derive(Debug, Clone)]
pub struct Vault {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
}

impl Vault {
    /// Scans `root`. Hidden entries (starting with `.`) are skipped, which
    /// covers `.git/`, `.obsidian/` and `.obsessed/`. Only `.md` files are
    /// listed as notes.
    pub fn scan(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        let mut entries = Vec::new();
        scan_dir(&root, "", 0, &mut entries)?;
        Ok(Self { root, entries })
    }

    pub fn notes(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.kind == Kind::Note)
    }

    /// The absolute path of an entry.
    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// The vault's display name: its folder name.
    pub fn name(&self) -> &str {
        self.root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("vault")
    }

    /// Resolves a wiki-link target the way Obsidian does by default:
    /// shortest unique name. `target` may include a path (`anim/sh040`) and
    /// may leave out `.md`. Anything after `#` or `|` is ignored.
    pub fn resolve(&self, target: &str) -> Resolved<'_> {
        let target = target.split(['#', '|']).next().unwrap_or("").trim();
        if target.is_empty() {
            return Resolved::NotFound;
        }
        let target = target.strip_suffix(".md").unwrap_or(target);
        let target = target.trim_start_matches('/');

        let mut found: Option<&Entry> = None;
        let mut count = 0;
        for note in self.notes() {
            let stem = note.rel.strip_suffix(".md").unwrap_or(&note.rel);
            // Exact path from the root always wins.
            if stem.eq_ignore_ascii_case(target) {
                return Resolved::One(note);
            }
            let suffix_match = stem.len() > target.len()
                && stem.as_bytes()[stem.len() - target.len() - 1] == b'/'
                && stem[stem.len() - target.len()..].eq_ignore_ascii_case(target);
            if suffix_match {
                count += 1;
                found = Some(note);
            }
        }
        match (count, found) {
            (1, Some(note)) => Resolved::One(note),
            (0, _) => Resolved::NotFound,
            _ => Resolved::Ambiguous(count),
        }
    }
}

/// Turns what someone typed in the "new note" box into a note path from the
/// vault root, e.g. `anim/sh040` → `anim/sh040.md`. `/` makes subfolders.
///
/// Refuses names Windows can't store and names Obsidian links can't reach
/// (`# ^ [ ] |`), plus hidden (`.x`) parts the app would never show.
pub fn note_rel_from_input(input: &str) -> Result<String, &'static str> {
    let input = input.trim().replace('\\', "/");
    let input = input.trim_start_matches('/');
    let input = input.strip_suffix(".md").unwrap_or(input);
    // Empty, or only a folder path such as `anim/`.
    if input.trim().is_empty() || input.ends_with('/') {
        return Err("Type a name for the note");
    }
    let mut parts = Vec::new();
    for part in input.split('/') {
        let part = part.trim();
        if part.is_empty() {
            return Err("A folder name is empty (two / in a row)");
        }
        if part.starts_with('.') {
            return Err("Names can't start with a dot");
        }
        if part.ends_with('.') {
            return Err("Windows doesn't allow names ending in a dot");
        }
        if part.contains([':', '*', '?', '"', '<', '>', '|', '#', '^', '[', ']'])
            || part.chars().any(char::is_control)
        {
            return Err("Names can't contain : * ? \" < > | # ^ [ ]");
        }
        let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit());
        if reserved {
            return Err("That name is reserved by Windows");
        }
        parts.push(part);
    }
    Ok(format!("{}.md", parts.join("/")))
}

/// Creates an empty note (and any missing folders). Never overwrites: if
/// the file already exists this returns `AlreadyExists`.
pub fn create_note(root: &Path, rel: &str) -> io::Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Resolved<'a> {
    One(&'a Entry),
    Ambiguous(usize),
    NotFound,
}

fn scan_dir(dir: &Path, rel: &str, depth: u16, out: &mut Vec<Entry>) -> io::Result<()> {
    let mut folders = Vec::new();
    let mut notes = Vec::new();

    for item in fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let name = item.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            folders.push(name.to_owned());
        } else if file_type.is_file() && has_md_extension(name) {
            notes.push(name.to_owned());
        }
    }

    folders.sort_by_cached_key(|n| n.to_lowercase());
    notes.sort_by_cached_key(|n| n.to_lowercase());

    for name in folders {
        let child_rel = join(rel, &name);
        out.push(Entry {
            rel: child_rel.clone().into_boxed_str(),
            depth,
            kind: Kind::Folder,
        });
        // An unreadable folder is shown but stays empty.
        let _ = scan_dir(&dir.join(&name), &child_rel, depth + 1, out);
    }
    for name in notes {
        out.push(Entry {
            rel: join(rel, &name).into_boxed_str(),
            depth,
            kind: Kind::Note,
        });
    }
    Ok(())
}

fn has_md_extension(name: &str) -> bool {
    name.len() > 3 && name[name.len() - 3..].eq_ignore_ascii_case(".md")
}

fn join(rel: &str, name: &str) -> String {
    if rel.is_empty() {
        name.to_owned()
    } else {
        format!("{rel}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(paths: &[&str]) -> Vault {
        Vault {
            root: PathBuf::new(),
            entries: paths
                .iter()
                .map(|p| Entry {
                    rel: (*p).into(),
                    depth: p.matches('/').count() as u16,
                    kind: Kind::Note,
                })
                .collect(),
        }
    }

    #[test]
    fn shortest_unique_name() {
        let v = vault(&["anim/sh040.md", "lighting/sh040.md", "notes/brief.md"]);
        assert!(matches!(v.resolve("brief"), Resolved::One(e) if &*e.rel == "notes/brief.md"));
        assert_eq!(v.resolve("sh040"), Resolved::Ambiguous(2));
        assert!(matches!(v.resolve("anim/sh040"), Resolved::One(e) if &*e.rel == "anim/sh040.md"));
        assert!(matches!(v.resolve("anim/sh040#^a2"), Resolved::One(_)));
        assert!(matches!(v.resolve("brief|Brief"), Resolved::One(_)));
        assert_eq!(v.resolve("missing"), Resolved::NotFound);
        // "040" must not match "sh040": only whole path components count.
        assert_eq!(v.resolve("040"), Resolved::NotFound);
    }

    #[test]
    fn new_note_names() {
        assert_eq!(note_rel_from_input("  Shot list "), Ok("Shot list.md".into()));
        assert_eq!(note_rel_from_input("anim/sh040.md"), Ok("anim/sh040.md".into()));
        assert_eq!(note_rel_from_input("/anim\\ sh040 "), Ok("anim/sh040.md".into()));
        assert!(note_rel_from_input("").is_err());
        assert!(note_rel_from_input("anim/").is_err());
        assert!(note_rel_from_input("a//b").is_err());
        assert!(note_rel_from_input("what?").is_err());
        assert!(note_rel_from_input("v2#notes").is_err());
        assert!(note_rel_from_input(".hidden").is_err());
        assert!(note_rel_from_input("con").is_err());
        assert!(note_rel_from_input("COM1.txt").is_err());
        assert_eq!(note_rel_from_input("Console"), Ok("Console.md".into()));
    }

    #[test]
    fn create_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("obsession-create-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        create_note(&dir, "new/folder/note.md").unwrap();
        assert_eq!(fs::read(dir.join("new/folder/note.md")).unwrap(), b"");
        fs::write(dir.join("new/folder/note.md"), "keep me").unwrap();
        let err = create_note(&dir, "new/folder/note.md").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(dir.join("new/folder/note.md")).unwrap(), b"keep me");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scan_skips_hidden_and_non_markdown() {
        let dir = std::env::temp_dir().join(format!("obsession-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("anim")).unwrap();
        fs::create_dir_all(dir.join(".obsidian")).unwrap();
        fs::write(dir.join("anim/sh040.md"), "x").unwrap();
        fs::write(dir.join("README.md"), "x").unwrap();
        fs::write(dir.join("image.png"), "x").unwrap();
        fs::write(dir.join(".obsidian/app.md"), "x").unwrap();

        let v = Vault::scan(&dir).unwrap();
        let rels: Vec<&str> = v.entries.iter().map(|e| &*e.rel).collect();
        assert_eq!(rels, ["anim", "anim/sh040.md", "README.md"]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
