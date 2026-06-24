// cancel.rs — Global cancellation token for long-running operations

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

static CANCEL_FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();

fn flag() -> Arc<AtomicBool> {
    CANCEL_FLAG.get_or_init(|| Arc::new(AtomicBool::new(false))).clone()
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
}
