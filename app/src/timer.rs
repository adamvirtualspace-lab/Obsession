//! A one-shot delay without pulling in an async runtime.
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
