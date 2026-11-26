//! Watching the vault folder for changes made by other apps (and the file
//! tree changing).
//!
//! On Windows this asks the system directly (`ReadDirectoryChangesW` on the
//! vault folder and everything under it) from one thread that sleeps until
//! something changes or the watch stops: a quiet vault costs nothing. The
//! `notify` crate's Windows backend wakes every 100 ms to look for its own
//! commands, which kept the app from ever being fully idle. Other systems
//! use `notify` (inotify, FSEvents), which sleeps until a change.

use std::path::{Path, PathBuf};

use iced::Subscription;
use iced::futures::{SinkExt, StreamExt};
use iced::futures::channel::mpsc::{UnboundedSender, unbounded};

use crate::runs::FsEvent;

/// How long the first change of a batch waits for others to join it.
const GATHER: std::time::Duration = std::time::Duration::from_millis(50);

/// Watches the vault folder. Events arrive in small batches.
pub fn watch(root: PathBuf) -> Subscription<Vec<FsEvent>> {
    Subscription::run_with(root, |root| {
        let root = root.clone();
        iced::stream::channel(16, async move |mut output| {
            let (tx, mut rx) = unbounded::<FsEvent>();
            // Stops the watch when this stream is dropped (another vault).
            let Some(_watching) = start(&root, tx) else { return };
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
        })
    })
}

/// `.git/`, `.obsidian/`, `.obsessed/` and other dot folders. The vault
/// folder itself passes (an overflow is reported as the folder: rescan).
fn ignored(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else { return true };
    rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
}

#[cfg(windows)]
use windows::start;

#[cfg(windows)]
mod windows {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME,
        FILE_ACTION_RENAMED_OLD_NAME, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
        FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
        FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING, ReadDirectoryChangesW,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};

    use super::{FsEvent, UnboundedSender};

    /// A handle closed when the last owner lets go.
    struct Owned(HANDLE);

    // Kernel handles can be used from any thread.
    unsafe impl Send for Owned {}
    unsafe impl Sync for Owned {}

    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    /// While held, the vault is watched; dropping it stops the thread.
    pub struct Watching(Arc<Owned>);

    impl Drop for Watching {
        fn drop(&mut self) {
            unsafe { SetEvent((self.0).0) };
        }
    }

    /// Starts watching `root` and everything under it.
    pub fn start(root: &Path, tx: UnboundedSender<FsEvent>) -> Option<Watching> {
        let wide: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
        let dir = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if dir == INVALID_HANDLE_VALUE || dir.is_null() {
            return None;
        }
        let dir = Owned(dir);
        let (io, stop) = unsafe {
            (
                CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()),
                CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()),
            )
        };
        if io.is_null() || stop.is_null() {
            return None;
        }
        let (io, stop) = (Owned(io), Arc::new(Owned(stop)));
        let root = root.to_path_buf();
        let thread_stop = stop.clone();
        std::thread::Builder::new()
            .name("vault watcher".into())
            .spawn(move || run(&root, &dir, &io, &thread_stop, &tx))
            .ok()?;
        Some(Watching(stop))
    }

    /// Reads changes until the watch stops, the receiver goes or the folder
    /// can't be read any more.
    fn run(root: &Path, dir: &Owned, io: &Owned, stop: &Owned, tx: &UnboundedSender<FsEvent>) {
        // DWORD-aligned, as ReadDirectoryChangesW needs.
        let mut buffer = vec![0u32; 16 * 1024];
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_LAST_WRITE
            | FILE_NOTIFY_CHANGE_SIZE;
        loop {
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            overlapped.hEvent = io.0;
            let asked = unsafe {
                ReadDirectoryChangesW(
                    dir.0,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 4) as u32,
                    1,
                    filter,
                    std::ptr::null_mut(),
                    &mut overlapped,
                    None,
                )
            };
            if asked == 0 {
                return;
            }
            // Sleeps until a change or the stop; no timeout.
            let handles = [io.0, stop.0];
            let woke = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
            let mut bytes = 0u32;
            if woke != WAIT_OBJECT_0 {
                // Stopped (or the wait failed): cancel the read and wait for
                // it to finish, since it writes into `buffer` and `overlapped`.
                unsafe {
                    CancelIoEx(dir.0, &overlapped);
                    GetOverlappedResult(dir.0, &overlapped, &mut bytes, 1);
                }
                return;
            }
            if unsafe { GetOverlappedResult(dir.0, &overlapped, &mut bytes, 0) } == 0 {
                return;
            }
            let events = if bytes == 0 {
                // Too much changed at once for the buffer: look again at all
                // of it.
                vec![FsEvent { path: root.to_path_buf(), structural: true }]
            } else {
                parse(root, &buffer, bytes as usize)
            };
            for event in events {
                if tx.unbounded_send(event).is_err() {
                    return;
                }
            }
        }
    }

    /// The `FILE_NOTIFY_INFORMATION` records in the first `len` bytes.
    fn parse(root: &Path, buffer: &[u32], len: usize) -> Vec<FsEvent> {
        let base = buffer.as_ptr().cast::<u8>();
        let mut out = Vec::new();
        let mut at = 0usize;
        while at + std::mem::size_of::<FILE_NOTIFY_INFORMATION>() <= len {
            // SAFETY: the system wrote whole, DWORD-aligned records here.
            let info = unsafe { &*base.add(at).cast::<FILE_NOTIFY_INFORMATION>() };
            let chars = info.FileNameLength as usize / 2;
            let name = unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), chars) };
            let rel = PathBuf::from(String::from_utf16_lossy(name));
            let structural = match info.Action {
                FILE_ACTION_ADDED | FILE_ACTION_REMOVED | FILE_ACTION_RENAMED_OLD_NAME | FILE_ACTION_RENAMED_NEW_NAME => {
                    Some(true)
                }
                FILE_ACTION_MODIFIED => Some(false),
                _ => None,
            };
            if let Some(structural) = structural {
                out.push(FsEvent { path: root.join(rel), structural });
            }
            if info.NextEntryOffset == 0 {
                break;
            }
            at += info.NextEntryOffset as usize;
        }
        out
    }
}

#[cfg(not(windows))]
use other::start;

#[cfg(not(windows))]
mod other {
    use std::path::Path;

    use notify::{EventKind, RecursiveMode, Watcher};

    use super::{FsEvent, UnboundedSender};

    /// While held, the vault is watched.
    pub struct Watching(#[allow(dead_code)] notify::RecommendedWatcher);

    pub fn start(root: &Path, tx: UnboundedSender<FsEvent>) -> Option<Watching> {
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
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
        })
        .ok()?;
        watcher.watch(root, RecursiveMode::Recursive).ok()?;
        Some(Watching(watcher))
    }
}
