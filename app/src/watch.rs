//! Watching the vault folder for changes made by other apps (and the file
//! tree changing), the same way on every system: `notify` 9 (Windows
//! `ReadDirectoryChangesW`, Linux inotify, macOS FSEvents). Its watcher
//! thread sleeps until a change: a quiet vault costs nothing. (Up to 8.2,
//! its Windows backend woke every 100 ms to look for its own commands.)

use std::path::{Path, PathBuf};

use iced::Subscription;
use iced::futures::channel::mpsc::unbounded;
use iced::futures::{SinkExt, StreamExt};
use notify::{EventKind, RecursiveMode, Watcher};

use crate::runs::FsEvent;

/// How long the first change of a batch waits for others to join it.
const GATHER: std::time::Duration = std::time::Duration::from_millis(50);

/// Watches the vault folder. Events arrive in small batches.
pub fn watch(root: PathBuf) -> Subscription<Vec<FsEvent>> {
    Subscription::run_with(root, |root| {
        let root = root.clone();
        iced::stream::channel(16, async move |mut output| {
            let (tx, mut rx) = unbounded::<FsEvent>();
            let vault = root.clone();
            let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else { return };
                // The system dropped changes: look at the whole vault again.
                if event.need_rescan() {
                    let _ = tx.unbounded_send(FsEvent { path: vault.clone(), structural: true });
                    return;
                }
                if matches!(event.kind, EventKind::Access(_)) {
                    return;
                }
                let structural = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
                );
                for path in event.paths {
                    let _ = tx.unbounded_send(FsEvent { path, structural });
                }
            });
            // Held until this stream is dropped (another vault), which stops it.
            let Ok(mut watcher) = watcher else { return };
            if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
                return;
            }
            while let Some(first) = rx.next().await {
                // Let a burst gather for a moment, then send it as one batch:
                // each batch is a message, and each message a redraw.
                crate::timer::after(GATHER).await;
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                let mut events: Vec<FsEvent> = Vec::new();
                for event in batch {
                    if ignored(&root, &event.path) {
                        continue;
                    }
                    match events.iter_mut().find(|e| e.path == event.path) {
                        Some(e) => e.structural |= event.structural,
                        None => events.push(event),
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

/// `.git/`, `.obsidian/`, `.obsessed/` and other dot folders. The vault
/// folder itself passes (a rescan is reported as the folder).
fn ignored(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else { return true };
    rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
}
