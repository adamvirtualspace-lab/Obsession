//! Running functions in the app (Syntax.md, section 10): the file watcher,
//! the run state (one run at a time, at most one a second, changes that
//! arrive meanwhile join one pending run), and the Runs panel with the log
//! and Undo.
//!
//! The work itself is in core: `runner` (which functions a change affects,
//! chains, the loop guard, undo) and `log` (`.obsessed/log/`). The app
//! gathers the texts from the store, runs `runner::run` off the UI thread,
//! then applies the result to the store (and the open note) and saves each
//! changed note once.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iced::futures::{SinkExt, StreamExt};
use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Element, Fill, Subscription};

use obsession_core::log::{self, Kind, Record};
use obsession_core::runner::Index;

use crate::theme;

/// At most one run a second.
pub const GAP: Duration = Duration::from_secs(1);
/// Records shown in the Runs panel.
pub const SHOWN: usize = 50;

#[derive(Debug, Clone)]
pub enum Message {
    Undo(u64),
    Close,
}

/// One thing the watcher saw.
#[derive(Debug, Clone)]
pub struct FsEvent {
    pub path: PathBuf,
    /// Created, removed or renamed (the vault's list may change), not just
    /// written.
    pub structural: bool,
}

pub struct Runs {
    pub index: Arc<Index>,
    /// Notes the index reads or writes, kept pinned in the store.
    pub pinned: Vec<String>,
    /// Notes changed since the last run.
    pub pending: BTreeSet<String>,
    pub running: bool,
    pub scheduled: bool,
    pub last: Option<Instant>,
    /// (graph, wire or node line) → what running it would change.
    pub out_of_date: HashMap<(String, usize), String>,
    /// Newest first.
    pub log: Vec<Record>,
    /// (graph, line) of the functions the last run ran, and when: their
    /// wires glow for a moment.
    pub glow: Vec<((String, usize), Instant)>,
    pub next_id: u64,
    pub show: bool,
}

impl Runs {
    pub fn new(root: &Path) -> Self {
        let (log, next_id) = log::load(root, SHOWN);
        Self {
            index: Arc::new(Index::default()),
            pinned: Vec::new(),
            pending: BTreeSet::new(),
            running: false,
            scheduled: false,
            last: None,
            out_of_date: HashMap::new(),
            log,
            glow: Vec::new(),
            next_id,
            show: false,
        }
    }

    /// How long until the next run may start.
    pub fn wait(&self) -> Duration {
        self.last.map_or(Duration::ZERO, |t| GAP.saturating_sub(t.elapsed()))
    }

    /// Adds a record to the log (on disk and in the panel).
    pub fn record(&mut self, root: &Path, kind: Kind, summary: String, changes: Vec<obsession_core::runner::Change>) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        let rec = Record::new(id, kind, summary, changes);
        let result = log::append(root, &rec).map_err(|e| format!("Couldn't write the run log: {e}"));
        self.log.insert(0, rec);
        self.log.truncate(SHOWN);
        result.map(|()| id)
    }

    /// The newest run that can still be undone.
    pub fn undoable(&self) -> Option<u64> {
        self.log
            .iter()
            .find(|r| r.kind == Kind::Run && !r.changes.is_empty() && !r.undone)
            .map(|r| r.id)
    }

    /// Out-of-date wires in one graph.
    pub fn stale_in(&self, graph: &str) -> usize {
        self.out_of_date.keys().filter(|(g, _)| g == graph).count()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let ago = |t: u64| {
            let s = now.saturating_sub(t);
            match s {
                0..=59 => "just now".to_owned(),
                60..=3599 => format!("{} min ago", s / 60),
                3600..=86_399 => format!("{} h ago", s / 3600),
                _ => format!("{} days ago", s / 86_400),
            }
        };
        let undoable = self.undoable();
        let mut list = column![].spacing(12);
        for rec in &self.log {
            let (icon, color) = match (rec.kind, rec.undone) {
                (Kind::Outside, _) => ("✎", theme::MUTED),
                (Kind::Run, true) => ("↶", theme::FAINT),
                (Kind::Run, false) => ("ƒ", theme::MENTION),
            };
            let mut head = row![
                text(icon).size(theme::TEXT_SIZE).color(color),
                text(rec.summary.clone())
                    .size(theme::SMALL)
                    .color(if rec.undone { theme::FAINT } else { theme::TEXT })
                    .width(Fill),
            ]
            .spacing(8);
            if Some(rec.id) == undoable {
                head = head.push(
                    button(text("Undo").size(theme::SMALL))
                        .on_press(Message::Undo(rec.id))
                        .padding([2, 10])
                        .style(theme::tool(false)),
                );
            }
            let mut item = column![head].spacing(3);
            let when = match (rec.kind, rec.undone) {
                (Kind::Outside, _) => format!("{} · changed in another app", ago(rec.time)),
                (Kind::Run, true) => format!("{} · undone", ago(rec.time)),
                (Kind::Run, false) => ago(rec.time),
            };
            item = item.push(text(when).size(11).color(theme::FAINT));
            for c in rec.changes.iter().take(6) {
                let line = c.new.first().or(c.old.first()).map_or("", String::as_str);
                item = item.push(
                    text(format!("{}:{}  {}", c.rel.trim_end_matches(".md"), c.line + 1, line.trim()))
                        .size(11)
                        .color(theme::MUTED),
                );
            }
            if rec.changes.len() > 6 {
                item = item.push(text(format!("+{} more", rec.changes.len() - 6)).size(11).color(theme::FAINT));
            }
            list = list.push(item);
        }
        if self.log.is_empty() {
            list = list.push(
                text("Nothing yet. Functions run when a note they read changes, here or in another app.")
                    .size(theme::SMALL)
                    .color(theme::MUTED),
            );
        }
        let status = if self.running {
            "Running…".to_owned()
        } else if !self.pending.is_empty() {
            "A run is waiting".to_owned()
        } else {
            format!("{} function{} watching", self.index.jobs.len(), if self.index.jobs.len() == 1 { "" } else { "s" })
        };
        container(
            column![
                row![
                    text("Runs").size(theme::TEXT_SIZE).color(theme::HEADING),
                    Space::new().width(Fill),
                    button(text("×").size(theme::SMALL)).on_press(Message::Close).padding([0, 8]).style(theme::tool(false)),
                ]
                .align_y(iced::Center),
                text(status).size(theme::SMALL).color(theme::MUTED),
                scrollable(list).height(Fill),
            ]
            .spacing(10)
            .padding(14),
        )
        .width(iced::Length::Fixed(320.0))
        .height(Fill)
        .style(theme::panel)
        .into()
    }
}

/// Watches the vault folder. Events arrive in small batches.
pub fn watch(root: PathBuf) -> Subscription<Vec<FsEvent>> {
    Subscription::run_with(root, |root| {
        let root = root.clone();
        iced::stream::channel(16, async move |mut output| {
            use notify::{EventKind, RecursiveMode, Watcher};
            let (tx, mut rx) = iced::futures::channel::mpsc::unbounded::<notify::Event>();
            let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    let _ = tx.unbounded_send(event);
                }
            });
            let Ok(mut watcher) = watcher else { return };
            if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
                return;
            }
            while let Some(first) = rx.next().await {
                // Gather what's already queued with it.
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                let mut events: Vec<FsEvent> = Vec::new();
                for event in batch {
                    let structural = matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
                    );
                    if matches!(event.kind, EventKind::Access(_)) {
                        continue;
                    }
                    for path in event.paths {
                        if !ignored(&root, &path) {
                            match events.iter_mut().find(|e| e.path == path) {
                                Some(e) => e.structural |= structural,
                                None => events.push(FsEvent { path, structural }),
                            }
                        }
                    }
                }
                if !events.is_empty() && output.send(events).await.is_err() {
                    break;
                }
            }
            drop(watcher);
        })
    })
}

/// `.git/`, `.obsidian/`, `.obsessed/` and other dot folders.
fn ignored(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else { return true };
    rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
}

/// A watched path as a vault path (`folder/note.md`), if it's a note.
pub fn rel_of(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let rel = rel.to_string_lossy().replace('\\', "/");
    rel.to_ascii_lowercase().ends_with(".md").then_some(rel)
}
