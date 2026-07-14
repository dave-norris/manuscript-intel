// cancel.rs — Global cancellation for long-running operations
//
// The Tauri command spawns work on a thread. When the user hits Stop:
// 1. Flag is set (checked between steps)
// 2. Notify fires (wakes any select! waiting on the result)
// The command returns "Cancelled." immediately to the UI.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

static CANCEL_FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
static CANCEL_NOTIFY: std::sync::OnceLock<Arc<Notify>> = std::sync::OnceLock::new();

fn flag() -> Arc<AtomicBool> {
    CANCEL_FLAG.get_or_init(|| Arc::new(AtomicBool::new(false))).clone()
}

pub fn notify() -> Arc<Notify> {
    CANCEL_NOTIFY.get_or_init(|| Arc::new(Notify::new())).clone()
}

pub fn is_cancelled() -> bool {
    flag().load(Ordering::Relaxed)
}

pub fn reset() {
    flag().store(false, Ordering::Relaxed);
}

#[tauri::command]
pub fn cancel_operation() {
    flag().store(true, Ordering::Relaxed);
    // Wake any select! that's waiting — the command returns immediately.
    notify().notify_waiters();
}
