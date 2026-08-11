use gtk::glib;
use std::{future::Future, sync::OnceLock};
use tokio::runtime::Runtime;

/// Returns the process-wide tokio runtime used for all background work.
pub fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to create tokio runtime"))
}

/// Runs `work` on the shared runtime and delivers its result to `on_done` on
/// the GTK main thread. Must be called from the main thread. If the work
/// panics, `on_done` is never called.
#[expect(dead_code, reason = "call sites arrive with the dispatch conversions")]
pub fn dispatch<T, Fut, Done>(work: Fut, on_done: Done)
where
    T: Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
    Done: FnOnce(T) + 'static,
{
    let handle = runtime().spawn(work);
    glib::spawn_future_local(async move {
        if let Ok(result) = handle.await {
            on_done(result);
        }
    });
}
