//! The run log in `.obsessed/log/` (Syntax.md, sections 10 and 11).
//!
//! One small text file per record: a run (every change it made, for undo)
//! or an outside edit (shown, not undoable). Files are written once and never
//! rewritten; undoing a run adds a `<id>.undone` marker beside it. The oldest
//! records are dropped past [`KEEP`].
//!
//! ```text
//! obsession-log 1
//! id 12
//! time 1763380000
//! kind run
//! summary Wait For Done: 1 change in comp/sh040
//! change comp/sh040.md|1|1|1|Wait For Done
//! -- [-] Comp sh040
//! +- [ ] Comp sh040
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::runner::Change;

/// How many records are kept.
pub const KEEP: usize = 500;

/// The machine-local folder at the vault root.
pub const FOLDER: &str = ".obsessed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Run,
    /// Another program changed the note.
    Outside,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: u64,
    /// Seconds since 1970.
    pub time: u64,
    pub kind: Kind,
    pub summary: String,
    pub changes: Vec<Change>,
    pub undone: bool,
}

impl Record {
    pub fn new(id: u64, kind: Kind, summary: String, changes: Vec<Change>) -> Self {
        let time = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Self {
            id,
            time,
            kind,
            summary,
            changes,
            undone: false,
        }
    }

    pub fn to_text(&self) -> String {
        let mut out = format!(
            "obsession-log 1\nid {}\ntime {}\nkind {}\nsummary {}\n",
            self.id,
            self.time,
            match self.kind {
                Kind::Run => "run",
                Kind::Outside => "outside",
            },
            self.summary.replace('\n', " ")
        );
        for c in &self.changes {
            out.push_str(&format!(
                "change {}|{}|{}|{}|{}\n",
                c.rel,
                c.line,
                c.old.len(),
                c.new.len(),
                c.by.replace('\n', " ")
            ));
            for l in &c.old {
                out.push('-');
                out.push_str(l);
                out.push('\n');
            }
            for l in &c.new {
                out.push('+');
                out.push_str(l);
                out.push('\n');
            }
        }
        out
    }

    pub fn from_text(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()? != "obsession-log 1" {
            return None;
        }
        let mut rec = Record {
            id: 0,
            time: 0,
            kind: Kind::Run,
            summary: String::new(),
            changes: Vec::new(),
            undone: false,
        };
        while let Some(line) = lines.next() {
            let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "id" => rec.id = rest.parse().ok()?,
                "time" => rec.time = rest.parse().ok()?,
                "kind" => rec.kind = if rest == "outside" { Kind::Outside } else { Kind::Run },
                "summary" => rec.summary = rest.to_owned(),
                "change" => {
                    let mut parts = rest.splitn(5, '|');
                    let rel = parts.next()?.to_owned();
                    let line: usize = parts.next()?.parse().ok()?;
                    let old_n: usize = parts.next()?.parse().ok()?;
                    let new_n: usize = parts.next()?.parse().ok()?;
                    let by = parts.next().unwrap_or("").to_owned();
                    let mut take = |n: usize, mark: char| -> Option<Vec<String>> {
                        (0..n).map(|_| lines.next()?.strip_prefix(mark).map(str::to_owned)).collect()
                    };
                    let old = take(old_n, '-')?;
                    let new = take(new_n, '+')?;
                    rec.changes.push(Change { rel, line, old, new, by });
                }
                _ => {}
            }
        }
        Some(rec)
    }
}

pub fn dir(root: &Path) -> PathBuf {
    root.join(FOLDER).join("log")
}

/// Creates `.obsessed/` (with a `.gitignore` of `*`, so it never reaches
/// git) and its `log/` folder.
pub fn ensure(root: &Path) -> io::Result<()> {
    let base = root.join(FOLDER);
    fs::create_dir_all(base.join("log"))?;
    let ignore = base.join(".gitignore");
    if !ignore.exists() {
        fs::write(ignore, "*\n")?;
    }
    Ok(())
}

fn record_path(root: &Path, id: u64) -> PathBuf {
    dir(root).join(format!("{id:010}.log"))
}

/// Writes a record, then drops the oldest past [`KEEP`].
pub fn append(root: &Path, rec: &Record) -> io::Result<()> {
    ensure(root)?;
    fs::write(record_path(root, rec.id), rec.to_text())?;
    let mut ids = ids(root)?;
    if ids.len() > KEEP {
        ids.sort_unstable();
        for id in &ids[..ids.len() - KEEP] {
            let _ = fs::remove_file(record_path(root, *id));
            let _ = fs::remove_file(record_path(root, *id).with_extension("undone"));
        }
    }
    Ok(())
}

pub fn mark_undone(root: &Path, id: u64) -> io::Result<()> {
    fs::write(record_path(root, id).with_extension("undone"), "")
}

fn ids(root: &Path) -> io::Result<Vec<u64>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir(root))? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        if let Some(id) = name.strip_suffix(".log").and_then(|n| n.parse().ok()) {
            out.push(id);
        }
    }
    Ok(out)
}

/// The newest `limit` records, newest first, and the next free id.
pub fn load(root: &Path, limit: usize) -> (Vec<Record>, u64) {
    let Ok(mut ids) = ids(root) else {
        return (Vec::new(), 1);
    };
    ids.sort_unstable_by(|a, b| b.cmp(a));
    let next = ids.first().map_or(1, |n| n + 1);
    let records = ids
        .into_iter()
        .take(limit)
        .filter_map(|id| {
            let path = record_path(root, id);
            let mut rec = Record::from_text(&fs::read_to_string(&path).ok()?)?;
            rec.undone = path.with_extension("undone").exists();
            Some(rec)
        })
        .collect();
    (records, next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_keep() {
        let rec = Record {
            id: 7,
            time: 100,
            kind: Kind::Run,
            summary: "Wait For Done: 1 change".into(),
            changes: vec![
                Change {
                    rel: "comp/sh 040.md".into(),
                    line: 3,
                    old: vec!["- [-] Comp".into()],
                    new: vec!["- [ ] Comp".into(), "+ odd -line".into()],
                    by: "Wait For Done".into(),
                },
                Change {
                    rel: "a.md".into(),
                    line: 0,
                    old: vec![],
                    new: vec![String::new()],
                    by: "X".into(),
                },
            ],
            undone: false,
        };
        assert_eq!(Record::from_text(&rec.to_text()), Some(rec.clone()));

        let root = std::env::temp_dir().join(format!("obsession-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        for id in 1..=(KEEP as u64 + 3) {
            append(&root, &Record { id, ..rec.clone() }).unwrap();
        }
        mark_undone(&root, KEEP as u64 + 3).unwrap();
        let (recent, next) = load(&root, 2);
        assert_eq!(next, KEEP as u64 + 4);
        assert_eq!(recent.len(), 2);
        assert!(recent[0].undone);
        assert!(!recent[1].undone);
        assert_eq!(ids(&root).unwrap().len(), KEEP);
        assert_eq!(fs::read_to_string(root.join(FOLDER).join(".gitignore")).unwrap(), "*\n");
        fs::remove_dir_all(&root).unwrap();
    }
}
