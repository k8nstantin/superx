//! The runner's own settings (issue #284).
//!
//! The runner owns its database, its schema, its directory and its CLI.
//! Its four knobs lived somewhere else entirely — as kernel substrate
//! parameters named `attr_runner_*` — so the kernel held settings whose
//! meaning it cannot know, in a namespace this module had to prefix its
//! way out of, and removing the module would have left its settings
//! behind in someone else's store.
//!
//! They live in `<home>/modules/runner/params.json` now: JSON rather
//! than a table, per the operator's decision on module params, so this
//! needs no schema of its own.
//!
//! **Nothing is lost.** A value already set as a kernel parameter is
//! adopted on first read and written here, so an operator who set
//! `attr_runner_agent_cmd` keeps it without doing anything. The kernel
//! parameter is left where it is — nothing is deleted — and simply
//! stops being consulted once this file has a value.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use superx_kernel::{Kernel, KernelError, Result};

use crate::MODULE_NAME;

/// The file, inside the module's own directory.
const FILE: &str = "params.json"; // skill-allow: §9-const — the module's own storage location, not a tunable

/// Every knob is optional, so "the operator has not chosen" is a state
/// the file can hold. Defaults belong to the readers, in one place each,
/// rather than being baked in here as though someone had set them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The command that runs an agent. Unset means dispatch refuses:
    /// nothing spawns an agent the operator never configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_cmd: Option<String>,
    /// Concurrent dispatch ceiling per firing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<usize>,
    /// Due-schedule poll cadence, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tick_secs: Option<u64>,
    /// How deep a plan walks the product graph.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_depth: Option<usize>,
}

/// Where this module keeps its settings, if there is a real instance.
///
/// Test kernels have no home, and a module with nowhere to write is not
/// an error — it simply has no stored settings and every reader falls
/// back to its default.
#[must_use]
pub fn path_for(kernel: &Kernel) -> Option<PathBuf> {
    kernel.module_dir(MODULE_NAME).ok().map(|d| d.join(FILE))
}

/// Read the settings, or the defaults if there are none.
///
/// A file that cannot be read or parsed yields defaults and a warning
/// rather than an error: a corrupt settings file must not stop the
/// runner from starting, and it must not be silently overwritten either.
#[must_use]
pub fn read_at(path: &Path) -> Settings {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Settings::default();
    };
    match serde_json::from_str(&raw) {
        Ok(settings) => settings,
        Err(e) => {
            tracing::warn!(
                target: "runner",
                path = %path.display(),
                error = %e,
                "settings file is unreadable — using defaults, and leaving the file alone"
            );
            Settings::default()
        }
    }
}

/// Write the settings.
///
/// Written to a neighbouring temporary file and renamed, so an
/// interrupted write leaves the previous settings intact rather than a
/// half-file the next read would reject.
///
/// # Errors
///
/// [`KernelError::Config`] when the file cannot be written.
pub fn write_at(path: &Path, settings: &Settings) -> Result<()> {
    let body = serde_json::to_string_pretty(settings)
        .map_err(|e| KernelError::Config(format!("serialize runner settings: {e}")))?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, body + "\n")
        .map_err(|e| KernelError::Config(format!("write {temp:?}: {e}")))?;
    std::fs::rename(&temp, path)
        .map_err(|e| KernelError::Config(format!("rename into {path:?}: {e}")))?;
    Ok(())
}

/// This module's settings.
#[must_use]
pub fn load(kernel: &Kernel) -> Settings {
    path_for(kernel).as_deref().map(read_at).unwrap_or_default()
}

/// Change one setting, leaving the rest as they are.
///
/// # Errors
///
/// [`KernelError::Config`] when there is no instance home, or the write
/// fails.
pub fn update(kernel: &Kernel, change: impl FnOnce(&mut Settings)) -> Result<Settings> {
    let path = path_for(kernel).ok_or_else(|| {
        KernelError::Config(
            "no instance home — the runner's settings live in its own module directory".into(),
        )
    })?;
    let mut settings = read_at(&path);
    change(&mut settings);
    write_at(&path, &settings)?;
    Ok(settings)
}

/// Take a value the kernel still holds and make it this module's own.
///
/// Called on the read path when a knob has no value here: the operator
/// set it before the runner owned its settings, and losing it because
/// the storage moved would be the module's fault, not theirs.
pub fn adopt(kernel: &Kernel, change: impl FnOnce(&mut Settings)) {
    if let Err(e) = update(kernel, change) {
        tracing::warn!(
            target: "runner",
            error = %e,
            "could not adopt a kernel parameter into the module's own settings — \
             still using the kernel value, and trying again next read"
        );
    }
}
