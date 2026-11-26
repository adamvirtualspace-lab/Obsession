//! Undo and redo for the editor.
//!
//! Each undo step is a snapshot of the note's text from just before a group
//! of edits, plus where the cursor was. Typing on one line without a pause
//! is one group; a pause of a second, moving to another line, or a
//! structural edit (Enter, joining lines, paste, a status click) starts a
//! new one. A snapshot is taken once per group, not per keystroke.
//!
//! Total snapshot size is capped; the oldest steps go first.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Typing pauses longer than this start a new undo step.
pub const GROUP_PAUSE: Duration = Duration::from_secs(1);
/// Undo history is trimmed beyond this many bytes of snapshots.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_STEPS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub text: String,
    /// (line, byte column) of the cursor.
    pub cursor: (usize, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// Inserting or deleting characters within a line.
    Typing,
    /// Anything that changes line structure or jumps: Enter, joins, paste,
    /// status changes.
    Other,
}

#[derive(Debug, Default)]
pub struct History {
    undo: VecDeque<Snapshot>,
    redo: Vec<Snapshot>,
    bytes: usize,
    last: Option<(Instant, usize, EditKind)>,
}

impl History {
    /// Call before applying an edit. Returns `true` when this edit starts a
    /// new undo step: then [`History::push`] the text as it is now.
    pub fn before_edit(&mut self, kind: EditKind, line: usize, now: Instant) -> bool {
        let continues = match self.last {
            Some((at, last_line, last_kind)) => {
                kind == EditKind::Typing
                    && last_kind == EditKind::Typing
                    && last_line == line
                    && now.duration_since(at) < GROUP_PAUSE
            }
            None => false,
        };
        self.last = Some((now, line, kind));
        self.redo.clear();
        !continues
    }

    /// Stores the state from before a new undo step.
    pub fn push(&mut self, snapshot: Snapshot) {
        self.bytes += snapshot.text.len();
        self.undo.push_back(snapshot);
        while self.undo.len() > MAX_STEPS || (self.bytes > MAX_BYTES && self.undo.len() > 1) {
            if let Some(old) = self.undo.pop_front() {
                self.bytes -= old.text.len();
            }
        }
    }

    /// Ends the current group, so the next edit starts a new undo step.
    pub fn break_group(&mut self) {
        self.last = None;
    }

    /// Returns the state to restore, given the state now.
    pub fn undo(&mut self, now: Snapshot) -> Option<Snapshot> {
        let previous = self.undo.pop_back()?;
        self.bytes -= previous.text.len();
        self.redo.push(now);
        self.last = None;
        Some(previous)
    }

    pub fn redo(&mut self, now: Snapshot) -> Option<Snapshot> {
        let next = self.redo.pop()?;
        self.bytes += now.text.len();
        self.undo.push_back(now);
        self.last = None;
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
}

/// Undo for changes the app makes to notes other than the open one, such
/// as removing a link from the graph or dropping a crew card on a task.
///
/// Each step keeps that note's text before and after, plus a fingerprint
/// of the open note's text when it happened. A step is undone (or redone)
/// only while the open note is back at that same text, so Ctrl+Z takes the
/// open note's own edits and these steps in the order they were made. The
/// app clears it when another note opens.
#[derive(Debug, Default)]
pub struct Elsewhere {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The notes it changed (one, or several for a function renamed in
    /// `_funct.md` and the graphs that use it).
    pub files: Vec<Change>,
    /// [`fingerprint`] of the open note's text when it happened (before
    /// the open note's own edit, for a joined step).
    pub open: u64,
    /// What it did, for the status line ("Removed the link to [[b]]").
    pub what: String,
    /// Made together with an edit of the open note, right after this step:
    /// undone right after that edit is undone, and redone right before it.
    pub joined: bool,
}

impl Step {
    /// The size of the texts it keeps.
    fn bytes(&self) -> usize {
        self.files.iter().map(|f| f.before.len() + f.after.len()).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub rel: String,
    pub before: String,
    pub after: String,
}

/// A cheap fingerprint of a note's text.
pub fn fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

impl Elsewhere {
    /// A new change: it can be undone, and nothing undone can be redone.
    pub fn push(&mut self, step: Step) {
        self.redo.clear();
        self.undo.push(step);
        // Each step holds whole texts (`_funct.md` for a function edit), so
        // the same byte cap as a note's history applies; the oldest go first.
        let mut bytes: usize = self.undo.iter().map(Step::bytes).sum();
        while self.undo.len() > MAX_STEPS || (bytes > MAX_BYTES && self.undo.len() > 1) {
            bytes -= self.undo.remove(0).bytes();
        }
    }

    /// The step to undo now, if the open note's text (fingerprinted as
    /// `open`) is what it was when the newest step was made.
    pub fn undo(&mut self, open: u64) -> Option<Step> {
        if self.undo.last()?.open != open {
            return None;
        }
        let step = self.undo.pop()?;
        self.redo.push(step.clone());
        Some(step)
    }

    /// The joined step to undo now that the open note's own edit was undone
    /// (the open note is back at `open`).
    pub fn undo_joined(&mut self, open: u64) -> Option<Step> {
        if !self.undo.last()?.joined {
            return None;
        }
        self.undo(open)
    }

    /// The step to redo now, under the same rule as [`Elsewhere::undo`].
    pub fn redo(&mut self, open: u64) -> Option<Step> {
        if self.redo.last()?.open != open {
            return None;
        }
        let step = self.redo.pop()?;
        self.undo.push(step.clone());
        Some(step)
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(text: &str) -> Snapshot {
        Snapshot {
            text: text.into(),
            cursor: (0, 0),
        }
    }

    #[test]
    fn groups_typing_on_one_line() {
        let mut h = History::default();
        let t = Instant::now();
        let mut calls = 0;
        let mut step = |h: &mut History, kind, line, at: Duration, text: &str| {
            if h.before_edit(kind, line, t + at) {
                calls += 1;
                h.push(snap(text));
            }
        };
        for (i, text) in ["", "a", "ab"].iter().enumerate() {
            step(&mut h, EditKind::Typing, 0, Duration::from_millis(i as u64 * 100), text);
        }
        // A pause starts a new step.
        step(&mut h, EditKind::Typing, 0, Duration::from_secs(5), "abc");
        // Another line starts a new step.
        step(&mut h, EditKind::Typing, 1, Duration::from_secs(5), "abcd");
        // Enter is always its own step.
        step(&mut h, EditKind::Other, 1, Duration::from_secs(5), "abcde");
        assert_eq!(calls, 4);

        assert_eq!(h.undo(snap("now")).unwrap().text, "abcde");
        assert_eq!(h.undo(snap("abcde")).unwrap().text, "abcd");
        assert_eq!(h.redo(snap("abcd")).unwrap().text, "abcde");
        assert_eq!(h.undo(snap("abcde")).unwrap().text, "abcd");
        assert_eq!(h.undo(snap("abcd")).unwrap().text, "abc");
        assert_eq!(h.undo(snap("abc")).unwrap().text, "");
        assert!(h.undo(snap("")).is_none());
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut h = History::default();
        assert!(h.before_edit(EditKind::Other, 0, Instant::now()));
        h.push(snap("a"));
        h.undo(snap("b")).unwrap();
        assert!(h.before_edit(EditKind::Other, 0, Instant::now()));
        h.push(snap("a"));
        assert!(h.redo(snap("c")).is_none());
    }

    fn step(rel: &str, open: &str) -> Step {
        Step {
            files: vec![Change {
                rel: rel.into(),
                before: "x".into(),
                after: "y".into(),
            }],
            open: fingerprint(open),
            what: String::new(),
            joined: false,
        }
    }

    #[test]
    fn elsewhere_waits_for_the_open_note() {
        let mut e = Elsewhere::default();
        e.push(step("a.md", "graph 1"));
        // The open note changed since: its own undo goes first.
        assert_eq!(e.undo(fingerprint("graph 2")), None);
        // Back at the text it had then: this step is next.
        assert_eq!(e.undo(fingerprint("graph 1")).map(|s| s.files[0].rel.clone()), Some("a.md".into()));
        assert_eq!(e.undo(fingerprint("graph 1")), None);
        // Redo follows the same rule.
        assert_eq!(e.redo(fingerprint("graph 0")), None);
        assert!(e.redo(fingerprint("graph 1")).is_some());
        // A new change drops what could be redone.
        e.undo(fingerprint("graph 1"));
        e.push(step("b.md", "graph 1"));
        assert_eq!(e.redo(fingerprint("graph 1")), None);
        e.clear();
        assert_eq!(e.undo(fingerprint("graph 1")), None);

        // A joined step waits for the open note's own edit to be undone,
        // and a plain one isn't taken as joined.
        e.push(step("c.md", "graph 1"));
        assert_eq!(e.undo_joined(fingerprint("graph 1")), None);
        e.push(Step { joined: true, ..step("d.md", "graph 1") });
        assert_eq!(e.undo_joined(fingerprint("graph 2")), None);
        assert!(e.undo_joined(fingerprint("graph 1")).is_some_and(|s| s.joined));
    }

    #[test]
    fn elsewhere_keeps_to_the_byte_cap() {
        let mut e = Elsewhere::default();
        let big = "x".repeat(MAX_BYTES / 4);
        for i in 0..4 {
            let mut s = step(&format!("{i}.md"), "graph");
            s.files[0].before = big.clone();
            e.push(s);
        }
        // Four steps of a quarter each, plus their "y"s: the oldest went.
        let mut rels = Vec::new();
        while let Some(s) = e.undo(fingerprint("graph")) {
            rels.push(s.files[0].rel.clone());
        }
        assert_eq!(rels, ["3.md", "2.md", "1.md"]);
        // One step bigger than the cap still stays.
        let mut e = Elsewhere::default();
        let mut s = step("huge.md", "graph");
        s.files[0].before = "x".repeat(MAX_BYTES + 1);
        e.push(s);
        assert!(e.undo(fingerprint("graph")).is_some());
    }
}
