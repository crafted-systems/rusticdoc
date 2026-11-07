//! Keeps this replica's copy of the stored fonts in step with the store, so a font
//! uploaded through any replica reaches LibreOffice on all of them without a
//! restart. The store is the only thing replicas share: no messages between them.

use std::sync::atomic::Ordering;

use rusticdoc_core::store::StoreError;

use crate::metrics::OutcomeLabels;
use crate::state::AppState;

/// Brings the copy up to date, and has LibreOffice look again if it changed.
pub async fn run_once(state: &AppState) -> Result<(), StoreError> {
    let Some(store) = &state.store else {
        return Ok(());
    };
    let outcome = |name: &str| {
        state
            .metrics
            .font_syncs
            .get_or_create(&OutcomeLabels {
                outcome: name.to_string(),
            })
            .inc();
    };
    let report = match store.sync_fonts(&state.config.font_cache_dir).await {
        Ok(report) => report,
        Err(e) => {
            outcome("failed");
            return Err(e);
        }
    };
    if report.changed {
        *state.fonts.write().await = None;
        if let Some(lo) = &state.engines.libreoffice
            && let Err(e) = lo.reload_fonts().await
        {
            tracing::warn!(error = %e, "LibreOffice workers did not restart on the new fonts");
        }
        tracing::info!(fonts = report.fonts, "stored fonts changed");
    }
    if report.failed > 0 {
        outcome("failed");
        return Err(StoreError::Backend(format!(
            "{} stored fonts could not be fetched",
            report.failed
        )));
    }
    outcome(if report.changed { "changed" } else { "ok" });
    // A font that fails its hash is left out and logged rather than holding every
    // new replica out of service: one bad object must not take the fleet down.
    state.fonts_synced.store(true, Ordering::Release);
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
                tracing::warn!(error = %e, "font sync failed; keeping the fonts in hand");
            }
        }
    });
}
