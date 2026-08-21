//! OS signal handling for graceful shutdown.
//!
//! Provides [`shutdown_signal()`], which resolves when either `SIGTERM` or
//! `SIGINT` (Ctrl-C) is received. This is used internally by
//! [`App::run`](crate::app::App::run) and
//! [`App::serve_listener`](crate::app::App::serve_listener) to trigger
//! graceful connection draining.

/// Wait for a shutdown signal (SIGTERM or SIGINT).
///
/// Resolves when either signal is received. Whichever fires first wins.
pub(crate) async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to register SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.expect("failed to listen for Ctrl-C");
    }
}
