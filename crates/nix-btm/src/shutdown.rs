use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::Notify;

#[derive(Clone)]
pub struct Shutdown {
    inner: Arc<Inner>,
}

struct Inner {
    notified: Notify,
    is_shutdown: AtomicBool,
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

impl Shutdown {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                notified: Notify::new(),
                is_shutdown: AtomicBool::new(false),
            }),
        }
    }

    /// Trigger shutdown exactly once. Subsequent calls are no-ops.
    pub fn trigger(&self) {
        if !self.inner.is_shutdown.swap(true, Ordering::SeqCst) {
            // first time wake all current waiters
            self.inner.notified.notify_waiters();
        }
    }

    /// Wait for shutdown. If shutdown already happened, returns immediately.
    pub async fn wait(&self) {
        // initializing the future here avoids race
        let notified = self.inner.notified.notified();

        // If shutdown already happened, don't await
        if self.inner.is_shutdown.load(Ordering::SeqCst) {
            return;
        }

        notified.await;
    }

    pub fn is_shutdown(&self) -> bool {
        self.inner.is_shutdown.load(Ordering::SeqCst)
    }
}
