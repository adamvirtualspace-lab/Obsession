//! A one-shot delay without pulling in an async runtime, and work on a
//! thread of its own (`on_thread`).
//!
//! Each call parks one short-lived thread. Notes only start a timer when none
//! is pending, so this happens at most a few times per second while typing,
//! and never while idle.

use std::future::Future;
use std::time::Duration;

use iced::futures::channel::oneshot;

pub fn after(delay: Duration) -> impl Future<Output = ()> + Send + 'static {
    let (sender, receiver) = oneshot::channel();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let _ = sender.send(());
    });
    async move {
        let _ = receiver.await;
    }
}

/// Runs `work` on a thread of its own and resolves with its result. Every
/// task one `update` returns is polled as one future, so blocking work in a
/// `Task::perform` holds up the others batched with it (the function index
/// waited for the whole link index at startup); this doesn't.
pub fn on_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> impl Future<Output = T> + Send + 'static {
    let (sender, receiver) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    async move { receiver.await.expect("background work panicked") }
}
