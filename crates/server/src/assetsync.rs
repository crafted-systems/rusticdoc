//! Keeps this replica's copy of the stored assets in step with the store, as
//! `fontsync` does for fonts, so an image uploaded through any replica reaches the
//! render workers of all of them. Nothing restarts: workers read the copy when a
//! template or page names an asset.

use std::sync::atomic::Ordering;

use rusticdoc_core::store::StoreError;

use crate::metrics::OutcomeLabels;
use crate::state::AppState;

/// Brings the copy up to date.
pub async fn run_once(state: &AppState) -> Result<(), StoreError> {
    let Some(store) = &state.store else {
        return Ok(());
    };
    let outcome = |name: &str| {
        state
            .metrics
            .asset_syncs
            .get_or_create(&OutcomeLabels {
                outcome: name.to_string(),
            })
            .inc();
    };
    let report = match store.sync_assets(&state.config.asset_cache_dir).await {
        Ok(report) => report,
        Err(e) => {
            outcome("failed");
            return Err(e);
        }
    };
    if report.changed {
        tracing::info!(assets = report.assets, "stored assets changed");
    }
    if report.failed > 0 {
        outcome("failed");
        return Err(StoreError::Backend(format!(
            "{} stored assets could not be fetched",
            report.failed
        )));
    }
    outcome(if report.changed { "changed" } else { "ok" });
    // As for fonts, one bad object is left out and logged and holds nothing up.
    state.assets_synced.store(true, Ordering::Release);
    Ok(())
}

/// Syncs now and then on the configured interval, for as long as the service runs.
/// A failure keeps the last good copy. A replica that has never synced stays
/// unready, and one that has stays in service.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(state.config.store_sync_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = run_once(&state).await {
                tracing::warn!(error = %e, "asset sync failed; keeping the assets in hand");
            }
        }
    });
}
