//! Recurrence (epic #189 S3, phase R4): after a firing completes, an
//! `every:<n><unit>` chain gets a NEW scheduled row at
//! `fired_at + duration` — the chain IS the perpetual history.
//! `none` rests at `fired`. A cancelled chain never re-enqueues.

use chrono::Utc;
use superx_kernel::{Db, Kernel};

use crate::schedule::{self, ScheduleRow};

/// Re-enqueue a recurring chain after its firing. Best-effort: a
/// re-enqueue failure is logged, never propagated (the firing's
/// facts are already recorded).
pub async fn re_enqueue(kernel: &Kernel, db: &Db, fired: &ScheduleRow) {
    let Some(token) = fired.recurrence.strip_prefix("every:") else {
        return; // 'none' rests at fired
    };
    // Cancelled mid-firing wins over recurrence (D27).
    match schedule::chain_current(db, &fired.uid).await {
        Ok(Some(current)) if current.status == "cancelled" => return,
        Err(e) => {
            tracing::warn!(target: "runner", "recurrence chain read failed: {e}");
            return;
        }
        _ => {}
    }
    let Ok((duration, _)) = schedule::parse_duration(token) else {
        tracing::warn!(target: "runner", "unparseable recurrence '{}' on {}", fired.recurrence, fired.uid);
        return;
    };
    let next = Utc::now() + duration;
    let renewed = ScheduleRow { run_at: next.to_rfc3339(), ..fired.clone() };
    if let Err(e) = schedule::append_status(db, &renewed, "scheduled").await {
        tracing::warn!(target: "runner", "re-enqueue failed for {}: {e}", fired.uid);
        return;
    }
    let payload = serde_json::json!({ "schedule": fired.uid, "next_run_at": next.to_rfc3339() });
    let subject = kernel
        .find_module_by_name(superx_kernel::NodeKind::KernelModule, crate::MODULE_NAME)
        .await
        .ok()
        .flatten();
    if let Err(e) = kernel
        .log_telemetry("recurrence_enqueued", superx_kernel::message::value_from_json(&payload), subject)
        .await
    {
        tracing::warn!(target: "runner", "telemetry write failed: {e}");
    }
}
