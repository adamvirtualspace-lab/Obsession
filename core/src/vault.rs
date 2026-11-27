//! Scanning a vault folder.
//!
//! The vault is kept as one flat, sorted list of entries (folders first, then
//! notes, case-insensitive). The file tree draws straight from it, which keeps
//! memory small and makes virtual scrolling trivial.
//!
//! Attachments (pictures, PDFs, videos: see `is_attachment`) are kept in a
//! second sorted list of paths, so `![[shot.png]]` resolves from any folder
//! the way Obsidian finds it. They aren't in the tree.

use std::collections::HashMap;
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
    /// Lowercased note name (no folder, no `.md`) -> entry indices.
    names: HashMap<String, Vec<u32>>,
    /// Lowercased path without `.md` -> entry index.
    paths: HashMap<String, u32>,
    /// Attachment paths from the vault root (`refs/shot.png`), sorted.
    pub files: Vec<Box<str>>,
    /// Lowercased file name (with extension) -> `files` indices.
    file_names: HashMap<String, Vec<u32>>,
    /// Lowercased path (with extension) -> `files` index.
    file_paths: HashMap<String, u32>,
}

impl Vault {
    /// Scans `root`. Hidden entries (starting with `.`) are skipped, which
    /// covers `.git/`, `.obsidian/` and `.obsessed/`. Only `.md` files are
    /// listed as notes; attachments go in `files`; anything else is left out.
    pub fn scan(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        let mut entries = Vec::new();
        let mut files = Vec::new();
        scan_dir(&root, "", 0, &mut entries, &mut files)?;
        Ok(Self::from_entries(root, entries).with_files(files))
    }

    /// Adds the attachments (paths from the vault root).
    pub fn with_files(mut self, mut files: Vec<Box<str>>) -> Self {
        files.sort_by_cached_key(|f| f.to_lowercase());
        let mut names: HashMap<String, Vec<u32>> = HashMap::new();
        let mut paths = HashMap::new();
        for (i, rel) in files.iter().enumerate() {
            let key = rel.to_lowercase();
            let name = key.rsplit('/').next().unwrap_or(&key).to_owned();
            names.entry(name).or_default().push(i as u32);
            paths.insert(key, i as u32);
        }
        self.files = files;
        self.file_names = names;
        self.file_paths = paths;
        self
    }

    pub fn from_entries(root: PathBuf, entries: Vec<Entry>) -> Self {
        let mut names: HashMap<String, Vec<u32>> = HashMap::new();
        let mut paths = HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            if entry.kind != Kind::Note {
                continue;
            }
            let stem = stem(&entry.rel).to_lowercase();
            let name = stem.rsplit('/').next().unwrap_or(&stem).to_owned();
            names.entry(name).or_default().push(i as u32);
            paths.insert(stem, i as u32);
        }
        Self {
            root,
            entries,
            names,
            paths,
            files: Vec::new(),
            file_names: HashMap::new(),
            file_paths: HashMap::new(),
        }
    }

    /// The note at exactly this path (any case, `.md` optional).
    pub fn note(&self, rel: &str) -> Option<&Entry> {
        let key = stem(rel.trim_start_matches('/')).to_lowercase();
        self.paths.get(&key).map(|&i| &self.entries[i as usize])
    }

    /// How many notes have this name (no folder, no `.md`).
    pub fn count_named(&self, name: &str) -> usize {
        self.names.get(&name.to_lowercase()).map_or(0, Vec::len)
    }

    /// How many attachments have this file name (with its extension).
    pub fn count_files_named(&self, name: &str) -> usize {
        self.file_names.get(&name.to_lowercase()).map_or(0, Vec::len)
    }

    /// The folder at exactly this path (any case), as the tree has it.
    pub fn folder(&self, rel: &str) -> Option<&Entry> {
        let rel = rel.trim_matches('/');
        self.entries
            .iter()
            .find(|e| e.kind == Kind::Folder && e.rel.eq_ignore_ascii_case(rel))
    }

    /// The notes inside folder `rel`, at any depth.
    pub fn notes_in<'a>(&'a self, rel: &'a str) -> impl Iterator<Item = &'a Entry> + 'a {
        self.notes().filter(move |e| moved(&e.rel, rel, rel).is_some())
    }

    /// The attachments inside folder `rel`, at any depth.
    pub fn files_in<'a>(&'a self, rel: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.files.iter().map(|f| &**f).filter(move |f| moved(f, rel, rel).is_some())
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

    /// Resolves a link target from anywhere (no linking note).
    pub fn resolve(&self, target: &str) -> Resolved<'_> {
        self.resolve_from(target, None)
    }

    /// Resolves a link target the way Obsidian does, from the note `from`:
    ///
    /// 1. `./x` and `../x` are relative to `from`'s folder.
    /// 2. A path from the vault root, or from `from`'s folder, wins outright.
    /// 3. Otherwise every note whose path ends with the target counts (so
    ///    `sh040` finds `anim/sh040.md`). One match is the answer. Several
    ///    are ambiguous: Obsidian still opens one, preferring `from`'s
    ///    folder, then the shortest path, so we pick the same one and say
    ///    how many there were.
    ///
    /// `.md` is optional, case doesn't matter, and anything after `#` or `|`
    /// is ignored. A picture or other attachment (by its extension) is
    /// found the same way among the attachments, its extension kept.
    pub fn resolve_from(&self, target: &str, from: Option<&str>) -> Resolved<'_> {
        let target = target.split(['#', '|']).next().unwrap_or("").trim().replace('\\', "/");
        let target = target.trim_start_matches('/');
        if target.is_empty() {
            return Resolved::NotFound;
        }
        let folder = from
            .and_then(|f| f.rsplit_once('/').map(|(dir, _)| dir))
            .unwrap_or("")
            .to_lowercase();
        if is_attachment(target) {
            let file = |i: u32| &*self.files[i as usize];
            let found = find(&target.to_lowercase(), &folder, &self.file_names, &self.file_paths, |i| file(i).to_lowercase(), file);
            return Resolved::Attachment(found.map(|(i, _)| file(i)));
        }
        let entry = |i: u32| &self.entries[i as usize];
        let found = find(&stem(target).to_lowercase(), &folder, &self.names, &self.paths, |i| stem(&entry(i).rel).to_lowercase(), |i| &*entry(i).rel);
        match found {
            None => Resolved::NotFound,
            Some((i, 1)) => Resolved::One(entry(i)),
            Some((i, count)) => Resolved::Ambiguous { picked: entry(i), count },
        }
    }

    /// The attachment a link or embed leads to (`refs/shot.png`), if it's
    /// in the vault.
    pub fn attachment(&self, target: &str, from: Option<&str>) -> Option<&str> {
        match self.resolve_from(target, from) {
            Resolved::Attachment(rel) => rel,
            _ => None,
        }
    }
}

/// The steps of `Vault::resolve_from`, for notes or attachments: `target`
/// and `folder` lowercased, `names` and `paths` the lookups, `key` an item's
/// lowercased path as `paths` has it, `rel` its path for the tie-break.
/// Returns the item Obsidian would pick and how many matched.
fn find<'a>(
    target: &str,
    folder: &str,
    names: &HashMap<String, Vec<u32>>,
    paths: &HashMap<String, u32>,
    key: impl Fn(u32) -> String,
    rel: impl Fn(u32) -> &'a str,
) -> Option<(u32, usize)> {
    if target.starts_with("./") || target.starts_with("../") {
        let path = normalize(&format!("{folder}/{target}"))?;
        return paths.get(&path).map(|&i| (i, 1));
    }
    if let Some(&i) = paths.get(target) {
        return Some((i, 1));
    }
    if !folder.is_empty()
        && target.contains('/')
        && let Some(&i) = paths.get(&format!("{folder}/{target}"))
    {
        return Some((i, 1));
    }
    let name = target.rsplit('/').next().unwrap_or(target);
    let suffix = format!("/{target}");
    let mut matches: Vec<u32> = names
        .get(name)?
        .iter()
        .copied()
        .filter(|&i| {
            let path = key(i);
            path == target || path.ends_with(&suffix)
        })
        .collect();
    let count = matches.len();
    if count > 1 {
        matches.sort_by_cached_key(|&i| {
            let r = rel(i);
            let parent = r.rsplit_once('/').map_or("", |(p, _)| p).to_lowercase();
            (parent != folder, r.matches('/').count(), r.to_lowercase())
        });
    }
    matches.first().map(|&i| (i, count))
}

/// A path without its `.md` (any case).
fn stem(rel: &str) -> &str {
    if rel.len() > 3 && rel[rel.len() - 3..].eq_ignore_ascii_case(".md") {
        &rel[..rel.len() - 3]
    } else {
        rel
    }
}

/// Resolves `.` and `..` in a `/` path. `None` if it climbs above the root.
fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// Links to these open files, not notes (Obsidian embeds and attachments).
fn is_attachment(target: &str) -> bool {
    let name = target.rsplit('/').next().unwrap_or(target);
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "avif" | "pdf" | "mp4" | "webm"
            | "mov" | "mkv" | "mp3" | "wav" | "ogg" | "m4a" | "flac" | "canvas" | "base"
            | "excalidraw" | "blend" | "psd" | "kra" | "fbx" | "glb" | "gltf" | "zip"
    )
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
    Ok(format!("{}.md", checked_path(input)?))
}

/// Turns what someone typed in the rename box for a folder into its path
/// from the vault root (`shots/anim/` → `shots/anim`), with the same rules
/// for names as notes.
pub fn folder_rel_from_input(input: &str) -> Result<String, &'static str> {
    let input = input.trim().replace('\\', "/");
    let input = input.trim_matches('/');
    if input.trim().is_empty() {
        return Err("Type a name for the folder");
    }
    checked_path(input)
}

/// Where `rel` (a note, attachment or folder path) is after `from` moves to
/// `to`: `from` itself, or anything inside it when it's a folder. `None`
/// when the move doesn't touch it.
pub fn moved(rel: &str, from: &str, to: &str) -> Option<String> {
    if rel == from {
        return Some(to.to_owned());
    }
    let rest = rel.strip_prefix(from)?.strip_prefix('/')?;
    Some(format!("{to}/{rest}"))
}

/// `input`'s `/` parts trimmed and checked (see `note_rel_from_input`).
fn checked_path(input: &str) -> Result<String, &'static str> {
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
    Ok(parts.join("/"))
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
    /// Several notes match; `picked` is the one Obsidian would open.
    Ambiguous { picked: &'a Entry, count: usize },
    NotFound,
    /// An image or other file, not a note: its path from the vault root,
    /// or `None` when no such file is in the vault.
    Attachment(Option<&'a str>),
}

impl<'a> Resolved<'a> {
    /// The note a click would open, if any.
    pub fn entry(&self) -> Option<&'a Entry> {
        match *self {
            Resolved::One(entry) | Resolved::Ambiguous { picked: entry, .. } => Some(entry),
            Resolved::NotFound | Resolved::Attachment(_) => None,
        }
    }
}

fn scan_dir(dir: &Path, rel: &str, depth: u16, out: &mut Vec<Entry>, files: &mut Vec<Box<str>>) -> io::Result<()> {
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
        } else if file_type.is_file() && is_attachment(name) {
            files.push(join(rel, name).into_boxed_str());
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
        let _ = scan_dir(&dir.join(&name), &child_rel, depth + 1, out, files);
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
pub(crate) mod tests {
    use super::*;

    pub(crate) fn vault(paths: &[&str]) -> Vault {
        Vault::from_entries(
            PathBuf::new(),
            paths
                .iter()
                .map(|p| Entry {
                    rel: (*p).into(),
                    depth: p.matches('/').count() as u16,
                    kind: Kind::Note,
                })
                .collect(),
        )
    }

    fn rel<'a>(r: Resolved<'a>) -> Option<&'a str> {
        r.entry().map(|e| &*e.rel)
    }

    #[test]
    fn shortest_unique_name() {
        let v = vault(&["anim/sh040.md", "lighting/sh040.md", "notes/brief.md"]);
        assert!(matches!(v.resolve("brief"), Resolved::One(e) if &*e.rel == "notes/brief.md"));
        assert!(matches!(v.resolve("sh040"), Resolved::Ambiguous { count: 2, .. }));
        assert!(matches!(v.resolve("anim/sh040"), Resolved::One(e) if &*e.rel == "anim/sh040.md"));
        assert!(matches!(v.resolve("anim/sh040#^a2"), Resolved::One(_)));
        assert!(matches!(v.resolve("brief|Brief"), Resolved::One(_)));
        assert!(matches!(v.resolve("Notes/Brief.MD"), Resolved::One(_)));
        assert_eq!(v.resolve("missing"), Resolved::NotFound);
        // "040" must not match "sh040": only whole path components count.
        assert_eq!(v.resolve("040"), Resolved::NotFound);
        assert_eq!(v.resolve("diagram.png"), Resolved::Attachment(None));
    }

    #[test]
    fn ambiguous_picks_like_obsidian() {
        let v = vault(&["a/x.md", "b/c/x.md", "b/x.md", "deep/er/x.md"]);
        // From a note in b/, its own folder wins.
        assert_eq!(rel(v.resolve_from("x", Some("b/note.md"))), Some("b/x.md"));
        // Otherwise the shortest path, then alphabetical.
        assert_eq!(rel(v.resolve_from("x", Some("z/note.md"))), Some("a/x.md"));
        // A partial path narrows it down.
        assert_eq!(rel(v.resolve_from("c/x", None)), Some("b/c/x.md"));
    }

    #[test]
    fn folder_names_and_moves() {
        assert_eq!(folder_rel_from_input(" shots/anim/ ").as_deref(), Ok("shots/anim"));
        assert_eq!(folder_rel_from_input("a\\b").as_deref(), Ok("a/b"));
        assert!(folder_rel_from_input("/").is_err());
        assert!(folder_rel_from_input("a/.hidden").is_err());
        assert!(folder_rel_from_input("a:b").is_err());
        assert_eq!(moved("anim/sh040.md", "anim", "work/anim").as_deref(), Some("work/anim/sh040.md"));
        assert_eq!(moved("anim", "anim", "work/anim").as_deref(), Some("work/anim"));
        assert_eq!(moved("animation/x.md", "anim", "work"), None);
        assert_eq!(moved("a.md", "a.md", "b.md").as_deref(), Some("b.md"));
        let v = vault(&["anim/a.md", "anim/deep/b.md", "animation/c.md", "d.md"])
            .with_files(vec!["anim/p.png".into(), "q.png".into()]);
        let inside: Vec<&str> = v.notes_in("anim").map(|e| &*e.rel).collect();
        assert_eq!(inside, ["anim/a.md", "anim/deep/b.md"]);
        assert_eq!(v.files_in("anim").collect::<Vec<_>>(), ["anim/p.png"]);
        assert_eq!(v.count_files_named("P.PNG"), 1);
    }

    #[test]
    fn relative_paths() {
        let v = vault(&["anim/sh040.md", "anim/shots/sh041.md", "brief.md"]);
        assert_eq!(rel(v.resolve_from("./shots/sh041", Some("anim/sh040.md"))), Some("anim/shots/sh041.md"));
        assert_eq!(rel(v.resolve_from("../brief.md", Some("anim/sh040.md"))), Some("brief.md"));
        assert_eq!(rel(v.resolve_from("shots/sh041", Some("anim/sh040.md"))), Some("anim/shots/sh041.md"));
        assert_eq!(v.resolve_from("../../x", Some("anim/sh040.md")), Resolved::NotFound);
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
    fn attachments_resolve_from_any_folder() {
        let v = vault(&["anim/sh040.md", "refs/brief.md", "shot.png.md"]).with_files(
            ["refs/shot.png", "comp/sh040/shot.png", "comp/Plate.JPG", "refs/deck.pdf"]
                .map(Box::from)
                .to_vec(),
        );
        let at = |t: &str, from: Option<&str>| v.attachment(t, from).map(str::to_owned);
        // By name from anywhere, any case.
        assert_eq!(at("plate.jpg", None).as_deref(), Some("comp/Plate.JPG"));
        assert_eq!(at("deck.pdf", Some("anim/sh040.md")).as_deref(), Some("refs/deck.pdf"));
        // Two with that name: the linking note's folder first, else the
        // shortest path, as Obsidian picks.
        assert_eq!(at("shot.png", Some("comp/sh040/notes.md")).as_deref(), Some("comp/sh040/shot.png"));
        assert_eq!(at("shot.png", Some("anim/sh040.md")).as_deref(), Some("refs/shot.png"));
        // A path wins; `./` and `../` are relative; `|size` and `#page` go.
        assert_eq!(at("sh040/shot.png", None).as_deref(), Some("comp/sh040/shot.png"));
        assert_eq!(at("../refs/shot.png|300", Some("anim/sh040.md")).as_deref(), Some("refs/shot.png"));
        assert_eq!(at("./deck.pdf#page=2", Some("refs/brief.md")).as_deref(), Some("refs/deck.pdf"));
        assert_eq!(at("comp\\Plate.jpg", None).as_deref(), Some("comp/Plate.JPG"));
        // Not in the vault: still an attachment, never a note to create.
        assert_eq!(v.resolve("gone.png"), Resolved::Attachment(None));
        assert_eq!(at("other/shot.png", None), None);
        // Notes resolve as before; a note named like a picture stays a note
        // only when linked with `.md`.
        assert!(matches!(v.resolve("brief"), Resolved::One(e) if &*e.rel == "refs/brief.md"));
        assert!(matches!(v.resolve("shot.png.md"), Resolved::One(e) if &*e.rel == "shot.png.md"));
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
        fs::write(dir.join("anim/Ref.JPG"), "x").unwrap();
        fs::write(dir.join("anim/notes.txt"), "x").unwrap();
        fs::write(dir.join(".obsidian/app.md"), "x").unwrap();
        fs::write(dir.join(".obsidian/icon.png"), "x").unwrap();

        let v = Vault::scan(&dir).unwrap();
        let rels: Vec<&str> = v.entries.iter().map(|e| &*e.rel).collect();
        assert_eq!(rels, ["anim", "anim/sh040.md", "README.md"]);
        // Attachments are listed apart, hidden folders and other files not.
        let files: Vec<&str> = v.files.iter().map(|f| &**f).collect();
        assert_eq!(files, ["anim/Ref.JPG", "image.png"]);
        assert_eq!(v.attachment("ref.jpg", None), Some("anim/Ref.JPG"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
