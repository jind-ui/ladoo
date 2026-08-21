//! Background LISTEN/NOTIFY listener for Postgres.

use sqlx::postgres::PgListener;
use sqlx::PgPool;
use tokio::sync::watch;

/// Spawns a background task that listens on `_ladoo_jobs_channel`
/// and sends a signal on the returned watch channel whenever a
/// notification arrives.
///
/// If the underlying connection drops, the task waits briefly and
/// retries; in the meantime the worker falls back to polling since
/// [`crate::store::JobStore::subscribe`] only needs to fire occasionally
/// to prompt a re-check, not on every insert.
pub(crate) async fn spawn_listener(
    pool: PgPool,
) -> Result<watch::Receiver<()>, crate::JobStoreError> {
    let (tx, rx) = watch::channel(());

    let mut listener = PgListener::connect_with(&pool)
        .await
        .map_err(|e| crate::JobStoreError::Database(e.to_string()))?;

    listener
        .listen("_ladoo_jobs_channel")
        .await
        .map_err(|e| crate::JobStoreError::Database(e.to_string()))?;

    tokio::spawn(async move {
        loop {
            match listener.recv().await {
                Ok(_) => {
                    let _ = tx.send(());
                }
                Err(_) => {
                    // Connection dropped — try to reconnect after a delay.
                    // The worker falls back to polling in the meantime.
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    });

    Ok(rx)
}
