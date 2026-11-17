//! The RAM-first file store.
//!
//! Every note the app works with has exactly one copy here. A note is read
//! from disk once; after that everything reads the copy in RAM. Changes are
//! made in RAM, and the disk only receives the result, once, and only when it
//! differs from what is already on disk.
//!
//! Writing happens off the UI thread: [`Store::take_save`] hands out a
//! [`SaveJob`], which the caller runs anywhere, then reports back with
//! [`Store::finish_save`].

use std::collections::HashMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::write::atomic_write;

/// Unpinned notes are dropped (least recently used first) above this size.
pub const DEFAULT_BUDGET: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub reads: u64,
    pub bytes_read: u64,
    pub writes: u64,
    pub bytes_written: u64,
    /// Saves that were skipped because nothing had changed.
    pub skipped_writes: u64,
}

/// What the store last saw on disk for a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiskState {
    modified: Option<SystemTime>,
    len: u64,
    hash: u64,
}

#[derive(Debug)]
struct Doc {
    text: String,
    hash: u64,
    disk: DiskState,
    /// Hash of a save that is currently being written, if any.
    saving: Option<u64>,
    pins: u32,
    last_used: u64,
}

#[derive(Debug)]
pub struct Store {
    root: PathBuf,
    docs: HashMap<Box<str>, Doc>,
    clock: u64,
    budget: usize,
    stats: Stats,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("not valid UTF-8, so it can't be edited safely")]
    NotUtf8,
}

/// What [`Store::check_outside`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outside {
    /// Nothing changed, or it was the app's own write.
    Same,
    /// Another program changed it; the store now holds the new text.
    Changed,
    /// Another program changed it, but unsaved changes here win.
    Conflict,
    Deleted,
    /// The store doesn't hold it, so it can't tell.
    NotLoaded,
}

/// A write prepared by [`Store::take_save`]. Run it on any thread.
#[derive(Debug, Clone)]
pub struct SaveJob {
    pub rel: Box<str>,
    path: PathBuf,
    bytes: String,
    hash: u64,
}

#[derive(Debug, Clone)]
pub struct SaveResult {
    rel: Box<str>,
    hash: u64,
    len: u64,
    outcome: Result<Option<SystemTime>, String>,
}

impl SaveResult {
    pub fn rel(&self) -> &str {
        &self.rel
    }

    pub fn error(&self) -> Option<&str> {
        self.outcome.as_ref().err().map(String::as_str)
    }
}

impl SaveJob {
    pub fn run(self) -> SaveResult {
        let outcome = atomic_write(&self.path, self.bytes.as_bytes())
            .and_then(|()| fs::metadata(&self.path))
            .map(|meta| meta.modified().ok())
            .map_err(|err| err.to_string());
        SaveResult {
            rel: self.rel,
            hash: self.hash,
            len: self.bytes.len() as u64,
            outcome,
        }
    }
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            docs: HashMap::new(),
            clock: 0,
            budget: DEFAULT_BUDGET,
            stats: Stats::default(),
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Bytes of note text held in RAM.
    pub fn bytes_in_ram(&self) -> usize {
        self.docs.values().map(|d| d.text.len()).sum()
    }

    pub fn is_loaded(&self, rel: &str) -> bool {
        self.docs.contains_key(rel)
    }

    /// The note's text. Read from disk only the first time.
    pub fn get(&mut self, rel: &str) -> Result<&str, Error> {
        self.clock += 1;
        let clock = self.clock;

        if !self.docs.contains_key(rel) {
            let doc = self.read(rel)?;
            self.docs.insert(rel.into(), doc);
            self.evict();
        }
        let doc = self.docs.get_mut(rel).expect("just inserted");
        doc.last_used = clock;
        Ok(&doc.text)
    }

    /// Keeps a note in RAM until every pin is released.
    pub fn pin(&mut self, rel: &str) -> Result<(), Error> {
        self.get(rel)?;
        if let Some(doc) = self.docs.get_mut(rel) {
            doc.pins += 1;
        }
        Ok(())
    }

    pub fn unpin(&mut self, rel: &str) {
        if let Some(doc) = self.docs.get_mut(rel) {
            doc.pins = doc.pins.saturating_sub(1);
        }
        self.evict();
    }

    /// Replaces the note's text in RAM. Nothing touches the disk.
    pub fn set(&mut self, rel: &str, text: String) {
        self.clock += 1;
        let hash = hash(&text);
        if let Some(doc) = self.docs.get_mut(rel) {
            doc.text = text;
            doc.hash = hash;
            doc.last_used = self.clock;
        }
    }

    /// Whether RAM differs from what is on disk (or being written).
    pub fn is_dirty(&self, rel: &str) -> bool {
        self.docs
            .get(rel)
            .is_some_and(|d| d.hash != d.saving.unwrap_or(d.disk.hash))
    }

    /// Prepares a write if the note changed. Returns `None` (and counts a
    /// skipped write) when the disk already has this exact text.
    pub fn take_save(&mut self, rel: &str) -> Option<SaveJob> {
        let path = self.root.join(rel);
        let doc = self.docs.get_mut(rel)?;
        if doc.hash == doc.saving.unwrap_or(doc.disk.hash) {
            self.stats.skipped_writes += 1;
            return None;
        }
        doc.saving = Some(doc.hash);
        Some(SaveJob {
            rel: rel.into(),
            path,
            bytes: doc.text.clone(),
            hash: doc.hash,
        })
    }

    pub fn finish_save(&mut self, result: &SaveResult) {
        let Some(doc) = self.docs.get_mut(&*result.rel) else {
            return;
        };
        if doc.saving == Some(result.hash) {
            doc.saving = None;
        }
        if let Ok(modified) = result.outcome {
            self.stats.writes += 1;
            self.stats.bytes_written += result.len;
            doc.disk = DiskState {
                modified,
                len: result.len,
                hash: result.hash,
            };
        }
    }

    /// Cheap check (metadata only, no read) for changes made by other
    /// programs. Our own writes are recognised and don't count.
    pub fn changed_on_disk(&self, rel: &str) -> io::Result<bool> {
        let Some(doc) = self.docs.get(rel) else {
            return Ok(false);
        };
        let meta = fs::metadata(self.root.join(rel))?;
        Ok(meta.len() != doc.disk.len || meta.modified().ok() != doc.disk.modified)
    }

    /// After the watcher reports `rel`: did another program change it?
    /// Costs a metadata call; the file is read only when its modified time
    /// or size moved. The app's own writes (matched by content hash) and
    /// touches without a content change are recognised and absorbed.
    pub fn check_outside(&mut self, rel: &str) -> Outside {
        let path = self.root.join(rel);
        let Some(doc) = self.docs.get_mut(rel) else {
            return if path.exists() { Outside::NotLoaded } else { Outside::Deleted };
        };
        let meta = match fs::metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Outside::Deleted,
            Err(_) => return Outside::Same,
        };
        if meta.len() == doc.disk.len && meta.modified().ok() == doc.disk.modified {
            return Outside::Same;
        }
        let Ok(bytes) = fs::read(&path) else {
            return Outside::Same;
        };
        self.stats.reads += 1;
        self.stats.bytes_read += bytes.len() as u64;
        let Ok(text) = String::from_utf8(bytes) else {
            return Outside::Same;
        };
        let disk_hash = hash(&text);
        let disk = DiskState {
            modified: meta.modified().ok(),
            len: text.len() as u64,
            hash: disk_hash,
        };
        // Our own write (finished or in flight), or what we already hold.
        if Some(disk_hash) == doc.saving || disk_hash == doc.disk.hash || disk_hash == doc.hash {
            if doc.saving.is_none() {
                doc.disk = disk;
            }
            return Outside::Same;
        }
        let dirty = doc.hash != doc.saving.unwrap_or(doc.disk.hash);
        doc.disk = disk;
        if dirty {
            // Unsaved changes here win; they're written over it next.
            return Outside::Conflict;
        }
        doc.text = text;
        doc.hash = disk_hash;
        Outside::Changed
    }

    /// Re-reads a note after another program changed it.
    pub fn reload(&mut self, rel: &str) -> Result<&str, Error> {
        let pins = self.docs.get(rel).map_or(0, |d| d.pins);
        let mut doc = self.read(rel)?;
        doc.pins = pins;
        self.docs.insert(rel.into(), doc);
        self.get(rel)
    }

    /// Forgets a note (e.g. it was deleted). Unsaved changes are lost.
    pub fn forget(&mut self, rel: &str) {
        self.docs.remove(rel);
    }

    fn read(&mut self, rel: &str) -> Result<Doc, Error> {
        let path = self.root.join(rel);
        let bytes = fs::read(&path)?;
        let meta = fs::metadata(&path)?;
        self.stats.reads += 1;
        self.stats.bytes_read += bytes.len() as u64;

        let text = String::from_utf8(bytes).map_err(|_| Error::NotUtf8)?;
        let hash = hash(&text);
        Ok(Doc {
            disk: DiskState {
                modified: meta.modified().ok(),
                len: text.len() as u64,
                hash,
            },
            text,
            hash,
            saving: None,
            pins: 0,
            last_used: self.clock,
        })
    }

    /// Drops unpinned, saved notes, least recently used first, until the
    /// unpinned ones fit the budget.
    fn evict(&mut self) {
        let mut unpinned: usize = self
            .docs
            .values()
            .filter(|d| d.pins == 0)
            .map(|d| d.text.len())
            .sum();
        if unpinned <= self.budget {
            return;
        }
        let mut candidates: Vec<(u64, Box<str>, usize)> = self
            .docs
            .iter()
            .filter(|(_, d)| {
                d.pins == 0
                    && d.saving.is_none()
                    && d.hash == d.disk.hash
                    // Never drop the note that was just asked for.
                    && d.last_used != self.clock
            })
            .map(|(rel, d)| (d.last_used, rel.clone(), d.text.len()))
            .collect();
        candidates.sort_unstable_by_key(|(used, _, _)| *used);
        for (_, rel, len) in candidates {
            if unpinned <= self.budget {
                break;
            }
            self.docs.remove(&rel);
            unpinned -= len;
        }
    }

    #[cfg(test)]
    fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
    }
}

fn hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vault(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("obsession-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn tells_outside_edits_from_own_writes() {
        let dir = temp_vault("own-writes");
        fs::write(dir.join("a.md"), "one").unwrap();
        let mut store = Store::new(&dir);
        store.get("a.md").unwrap();
        assert_eq!(store.check_outside("a.md"), Outside::Same);

        // Our own write is recognised.
        store.set("a.md", "two".into());
        let result = store.take_save("a.md").unwrap().run();
        assert_eq!(store.check_outside("a.md"), Outside::Same);
        store.finish_save(&result);
        assert_eq!(store.check_outside("a.md"), Outside::Same);

        // Someone else's isn't (a different length, so no clock games).
        fs::write(dir.join("a.md"), "three!").unwrap();
        assert_eq!(store.check_outside("a.md"), Outside::Changed);
        assert_eq!(store.get("a.md").unwrap(), "three!");
        assert_eq!(store.check_outside("a.md"), Outside::Same);

        // Unsaved changes here win.
        store.set("a.md", "mine".into());
        fs::write(dir.join("a.md"), "theirs!!").unwrap();
        assert_eq!(store.check_outside("a.md"), Outside::Conflict);
        assert_eq!(store.get("a.md").unwrap(), "mine");

        fs::write(dir.join("b.md"), "b").unwrap();
        assert_eq!(store.check_outside("b.md"), Outside::NotLoaded);
        fs::remove_file(dir.join("a.md")).unwrap();
        assert_eq!(store.check_outside("a.md"), Outside::Deleted);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_once_then_serves_from_ram() {
        let dir = temp_vault("once");
        fs::write(dir.join("a.md"), "- [ ] task\r\n").unwrap();
        let mut store = Store::new(&dir);

        for _ in 0..10 {
            assert_eq!(store.get("a.md").unwrap(), "- [ ] task\r\n");
        }
        assert_eq!(store.stats().reads, 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writes_only_when_changed() {
        let dir = temp_vault("changed");
        fs::write(dir.join("a.md"), "one").unwrap();
        let mut store = Store::new(&dir);
        store.get("a.md").unwrap();

        // Same text: no write.
        store.set("a.md", "one".into());
        assert!(store.take_save("a.md").is_none());

        // New text: one write.
        store.set("a.md", "two".into());
        let job = store.take_save("a.md").unwrap();
        // A second save while the first is in flight is skipped.
        assert!(store.take_save("a.md").is_none());
        let result = job.run();
        store.finish_save(&result);

        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "two");
        assert!(!store.is_dirty("a.md"));
        assert!(!store.changed_on_disk("a.md").unwrap());
        assert_eq!(store.stats().writes, 1);
        assert_eq!(store.stats().skipped_writes, 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn edits_during_a_save_stay_dirty() {
        let dir = temp_vault("during");
        fs::write(dir.join("a.md"), "one").unwrap();
        let mut store = Store::new(&dir);
        store.get("a.md").unwrap();

        store.set("a.md", "two".into());
        let job = store.take_save("a.md").unwrap();
        store.set("a.md", "three".into());
        store.finish_save(&job.run());

        assert!(store.is_dirty("a.md"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn notices_outside_edits() {
        let dir = temp_vault("outside");
        fs::write(dir.join("a.md"), "one").unwrap();
        let mut store = Store::new(&dir);
        store.get("a.md").unwrap();

        fs::write(dir.join("a.md"), "changed elsewhere").unwrap();
        assert!(store.changed_on_disk("a.md").unwrap());
        assert_eq!(store.reload("a.md").unwrap(), "changed elsewhere");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn evicts_unpinned_but_keeps_pinned() {
        let dir = temp_vault("evict");
        for name in ["a.md", "b.md", "c.md"] {
            fs::write(dir.join(name), "0123456789").unwrap();
        }
        let mut store = Store::new(&dir);
        store.set_budget(15);

        store.pin("a.md").unwrap();
        store.get("b.md").unwrap();
        store.get("c.md").unwrap();

        assert!(store.is_loaded("a.md"), "pinned notes stay");
        assert!(!store.is_loaded("b.md"), "oldest unpinned note goes first");
        assert!(store.is_loaded("c.md"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_non_utf8() {
        let dir = temp_vault("utf8");
        fs::write(dir.join("a.md"), [0xff, 0xfe, 0x00]).unwrap();
        let mut store = Store::new(&dir);
        assert!(matches!(store.get("a.md"), Err(Error::NotUtf8)));
        fs::remove_dir_all(&dir).unwrap();
    }
}
