//! The Models view (#436, epic #433): which model to reach for.
//!
//! Every figure is a cut of the join ([`crate::tasks`]). What a model spent
//! in a task is its own. The task's lines and working time are shared among
//! the models that replied by their share of its output, as a squash's
//! lines are shared among the branch commits it landed. A task, and the
//! correction that may follow it, goes to its LEAD: the model with the most
//! output in it.
//!
//! # Only tested work is judged
//!
//! A line nobody has worked around since it was written has not been
//! tested, so whether it still stands says nothing (#434, operator). A task
//! is TESTED once the repositories it worked in have seen the tested
//! threshold of later work. Untested work is counted, never rated.

use std::collections::HashMap;

use serde::Serialize;
use superx_kernel::Kernel;
use ts_rs::TS;

use crate::compare::{pct, split};
use crate::tasks::TaskRow;

/// The later work, in minutes, after which a task's lines are judged. A
/// parameter on the ui module's registry entity.
pub const TESTED_MINUTES_PARAM: &str = "attr_ui_tested_minutes";
const DEFAULT_TESTED_MINUTES: i64 = 60; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// One model, cut from the tasks. Lines are tested work only; a rate with
/// nothing to divide by is -1, never a reading.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct ModelRow {
    /// The model family (`opus 5.5`).
    pub model: String,
    /// Tasks it led.
    pub tasks: i64,
    /// Of those, the ones the operator's next turn pulled back.
    pub corrected: i64,
    pub corrections_per_100: i64,
    pub out_tokens: i64,
    pub unsupervised_out_tokens: i64,
    pub working_minutes: i64,

    pub tested_tasks: i64,
    pub tested_out_tokens: i64,
    pub tested_working_minutes: i64,
    pub written: i64,
    pub standing: i64,
    pub replaced: i64,
    pub never_landed: i64,
    /// On a branch still checked out: not judged yet.
    pub in_flight: i64,
    /// Of the judged lines (written, less in flight).
    pub standing_pct: i64,
    pub replaced_pct: i64,
    pub never_landed_pct: i64,
    /// Output tokens per judged line.
    pub tokens_per_line: i64,
    /// Output tokens per line still standing: the price once rework is paid.
    pub tokens_per_standing: i64,
    pub standing_per_mtok: i64,
    pub standing_per_hour: i64,
    /// The median later work its tested tasks with lines have seen.
    pub median_exposure_minutes: i64,

    pub untested_tasks: i64,
    pub untested_written: i64,
    pub untested_out_tokens: i64,
}

#[derive(Default)]
struct Acc {
    tasks: i64,
    corrected: i64,
    out_tokens: i64,
    unsupervised: i64,
    working_secs: i64,
    tested_tasks: i64,
    tested_out: i64,
    tested_secs: i64,
    written: i64,
    standing: i64,
    replaced: i64,
    never_landed: i64,
    in_flight: i64,
    exposures: Vec<i64>,
    untested_tasks: i64,
    untested_written: i64,
    untested_out: i64,
}

/// The rows, one per model, in a fixed order: by name, then by version.
#[must_use]
pub fn model_rows(tasks: &[TaskRow], tested_minutes: i64) -> Vec<ModelRow> {
    let mut acc: HashMap<&str, Acc> = HashMap::new();
    for t in tasks.iter().filter(|t| !t.models.is_empty()) {
        let tested = t.exposure_minutes >= tested_minutes;
        // A task whose replies reported no usage shares its lines evenly.
        let tokens: Vec<i64> = t.models.iter().map(|m| m.out_tokens).collect();
        let weights = if tokens.iter().sum::<i64>() > 0 { tokens } else { vec![1; t.models.len()] };
        let share = |v: i64| split(v, &weights);
        let (written, standing, replaced) = (share(t.lines_written), share(t.lines_standing), share(t.lines_replaced));
        let (never, flight, secs) = (share(t.lines_never_landed), share(t.lines_in_flight), share(t.working_secs));
        let lead = (0..t.models.len()).fold(0, |b, k| if t.models[k].out_tokens > t.models[b].out_tokens { k } else { b });
        for (k, m) in t.models.iter().enumerate() {
            let a = acc.entry(m.model.as_str()).or_default();
            a.out_tokens += m.out_tokens;
            a.unsupervised += m.unsupervised_out_tokens;
            a.working_secs += secs[k];
            if k == lead {
                a.tasks += 1;
                a.corrected += i64::from(t.needed_correcting);
            }
            if tested {
                a.tested_out += m.out_tokens;
                a.tested_secs += secs[k];
                a.written += written[k];
                a.standing += standing[k];
                a.replaced += replaced[k];
                a.never_landed += never[k];
                a.in_flight += flight[k];
                if k == lead {
                    a.tested_tasks += 1;
                    if t.lines_written > 0 {
                        a.exposures.push(t.exposure_minutes);
                    }
                }
            } else {
                a.untested_out += m.out_tokens;
                a.untested_written += written[k];
                a.untested_tasks += i64::from(k == lead);
            }
        }
    }
    let mut rows: Vec<ModelRow> = acc.into_iter().map(|(model, a)| row(model, a)).collect();
    rows.sort_by_cached_key(|r| order_key(&r.model));
    rows
}

/// A model's place in the fixed order: its name, then its version as
/// numbers, so `opus 5.5` comes after `opus 5` and before `opus 10`.
fn order_key(model: &str) -> (String, Vec<u64>, String) {
    let mut words = model.split(' ');
    let name = words.next().unwrap_or_default().to_string();
    let version: Vec<u64> = words
        .filter(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .flat_map(|w| w.split('.').map(|p| p.parse().unwrap_or(0)).collect::<Vec<u64>>())
        .collect();
    (name, version, model.to_string())
}

fn row(model: &str, mut a: Acc) -> ModelRow {
    let judged = a.standing + a.replaced + a.never_landed;
    let per = |num: i64, den: i64, scale: i64| if den > 0 { num.saturating_mul(scale) / den } else { -1 };
    a.exposures.sort_unstable();
    ModelRow {
        model: model.to_string(),
        tasks: a.tasks,
        corrected: a.corrected,
        corrections_per_100: if a.tasks > 0 { pct(a.corrected, a.tasks) } else { -1 },
        out_tokens: a.out_tokens,
        unsupervised_out_tokens: a.unsupervised,
        working_minutes: a.working_secs / 60,
        tested_tasks: a.tested_tasks,
        tested_out_tokens: a.tested_out,
        tested_working_minutes: a.tested_secs / 60,
        written: a.written,
        standing: a.standing,
        replaced: a.replaced,
        never_landed: a.never_landed,
        in_flight: a.in_flight,
        standing_pct: if judged > 0 { pct(a.standing, judged) } else { -1 },
        replaced_pct: if judged > 0 { pct(a.replaced, judged) } else { -1 },
        never_landed_pct: if judged > 0 { pct(a.never_landed, judged) } else { -1 },
        tokens_per_line: per(a.tested_out, judged, 1),
        tokens_per_standing: per(a.tested_out, a.standing, 1),
        standing_per_mtok: per(a.standing, a.tested_out, 1_000_000),
        standing_per_hour: per(a.standing, a.tested_secs, 3_600),
        median_exposure_minutes: a.exposures.get(a.exposures.len() / 2).copied().unwrap_or(-1),
        untested_tasks: a.untested_tasks,
        untested_written: a.untested_written,
        untested_out_tokens: a.untested_out,
    }
}

/// The tested threshold, in minutes of later work.
pub async fn resolved_tested_minutes(kernel: &Kernel) -> i64 {
    crate::tasks::resolved_positive(kernel, TESTED_MINUTES_PARAM, DEFAULT_TESTED_MINUTES).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{Driver, ModelSpend};

    fn spend(model: &str, out: i64) -> ModelSpend {
        ModelSpend { model: model.to_string(), out_tokens: out, unsupervised_out_tokens: 0 }
    }

    fn task(models: Vec<ModelSpend>, exposure: i64, lines: (i64, i64, i64, i64)) -> TaskRow {
        let (written, standing, replaced, never) = lines;
        TaskRow {
            session: "s".into(),
            opened_at: "2026-10-01T10:00:00Z".into(),
            driver: Driver::Directed,
            needed_correcting: false,
            opening: String::new(),
            out_tokens: models.iter().map(|m| m.out_tokens).sum(),
            models,
            unsupervised_out_tokens: 0,
            working_secs: 3_600,
            operator_secs: 0,
            exposure_minutes: exposure,
            repos: vec!["r".into()],
            files_written: 1,
            commits: 1,
            lines_written: written,
            lines_standing: standing,
            lines_replaced: replaced,
            lines_never_landed: never,
            lines_in_flight: 0,
        }
    }

    #[test]
    fn a_task_is_shared_by_output_and_led_by_the_larger() {
        let mut t = task(vec![spend("fable 5.1", 100), spend("opus 5.5", 300)], 120, (8, 4, 4, 0));
        t.needed_correcting = true;
        let rows = model_rows(&[t], 60);
        let (fable, opus) = (&rows[0], &rows[1]);
        assert_eq!((opus.written, opus.standing, opus.working_minutes), (6, 3, 45), "three quarters of the output");
        assert_eq!((fable.written, fable.standing, fable.working_minutes), (2, 1, 15));
        assert_eq!((opus.tasks, opus.corrected, fable.tasks), (1, 1, 0), "the task and its correction go to the lead");
        assert_eq!((opus.tokens_per_standing, opus.standing_pct), (100, 50));
    }

    #[test]
    fn untested_work_is_counted_never_rated() {
        let fresh = task(vec![spend("opus 5.5", 1_000)], 10, (100, 100, 0, 0));
        let rows = model_rows(&[fresh], 60);
        let r = &rows[0];
        assert_eq!((r.untested_tasks, r.untested_written, r.untested_out_tokens), (1, 100, 1_000));
        assert_eq!((r.written, r.standing_pct, r.tokens_per_standing), (0, -1, -1), "no rate from untested lines");
        assert_eq!(r.median_exposure_minutes, -1);

        let tested = task(vec![spend("opus 5.5", 1_000)], 90, (100, 40, 50, 10));
        let rows = model_rows(&[tested], 60);
        let r = &rows[0];
        assert_eq!((r.standing_pct, r.replaced_pct, r.never_landed_pct), (40, 50, 10));
        assert_eq!((r.tokens_per_line, r.tokens_per_standing, r.standing_per_mtok, r.standing_per_hour), (10, 25, 40_000, 40));
        assert_eq!(r.median_exposure_minutes, 90);
    }

    #[test]
    fn a_task_with_no_usage_shares_evenly() {
        let rows = model_rows(&[task(vec![spend("opus 5", 0), spend("opus 5.5", 0)], 90, (10, 10, 0, 0))], 60);
        assert_eq!(rows.iter().map(|r| r.written).sum::<i64>(), 10);
        assert_eq!((rows[0].written, rows[1].written), (5, 5));
    }

    #[test]
    fn models_keep_one_order_by_name_then_version() {
        let tasks: Vec<TaskRow> = ["opus 5.5", "fable 5.1", "opus 10", "opus 5", "fable 5"]
            .iter()
            .map(|m| task(vec![spend(m, 1)], 90, (1, 1, 0, 0)))
            .collect();
        let order: Vec<String> = model_rows(&tasks, 60).into_iter().map(|r| r.model).collect();
        assert_eq!(order, vec!["fable 5", "fable 5.1", "opus 5", "opus 5.5", "opus 10"]);
    }
}
