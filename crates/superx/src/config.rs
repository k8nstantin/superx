//! Instance configuration — `params/superx.json` (issue #125).
//!
//! Operator directive: a JSON parameter file controls the OS — db
//! access, directories, everything bootstrap-scoped; nothing
//! hardcoded — inside an explicit instance layout:
//!
//! ```text
//! <home>/                   --home flag / SUPERX_HOME env, default "."
//!   params/superx.json      THE parameter file (this module)
//!   logs/                   self-log + daemon output
//!   db/                     datastore + credentials + pidfile
//! ```
//!
//! **Scope boundary:** this file holds only what must be known BEFORE
//! the substrate is reachable (connection, paths, log filter).
//! Runtime tunables remain substrate parameters (skill §9) — facts and
//! knobs the OS can serve itself once connected.
//!
//! **Precedence:** CLI flag > env var > params file > built-in
//! fallback. `--initialize` writes the file with the effective values
//! when absent, so every knob is visible and editable.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Built-in fallbacks — used only when neither flag, env, nor params
/// file provides a value; `--initialize` then persists the effective
/// choice into the params file where it is visible and editable.
const FALLBACK_ENDPOINT: &str = "ws://127.0.0.1:8000"; // skill-allow: §9-default — persisted to params file at init
const FALLBACK_NAMESPACE: &str = "superx"; // skill-allow: §9-default — persisted to params file at init
const FALLBACK_DATABASE: &str = "kernel"; // skill-allow: §9-default — persisted to params file at init
const FALLBACK_DATA_DIR: &str = "db/superx-v2.db"; // skill-allow: §9-default — persisted to params file at init
const FALLBACK_LOG_DIR: &str = "logs"; // skill-allow: §9-default — persisted to params file at init
const FALLBACK_LOG_FILTER: &str = "info"; // skill-allow: §9-default — persisted to params file at init

/// The on-disk shape of `params/superx.json`. Every field optional —
/// absent fields fall through the precedence chain.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Params {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    /// Relative paths resolve against `<home>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_dir: Option<String>,
    /// `EnvFilter` syntax (`info`, `debug`, `superx_kernel=trace`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_filter: Option<String>,
}

/// The effective, fully-resolved instance configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub home: PathBuf,
    pub endpoint: String,
    pub namespace: String,
    pub database: String,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub log_filter: String,
}

#[must_use]
pub fn params_path(home: &Path) -> PathBuf {
    home.join("params").join("superx.json")
}

/// Load `params/superx.json` if present. A malformed file is an error
/// (never silently ignored — the operator edited it for a reason).
pub fn load_params(home: &Path) -> Result<Params, String> {
    let path = params_path(home);
    if !path.exists() {
        return Ok(Params::default());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("read {path:?}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))
}

/// Persist the effective configuration as the params file (pretty
/// JSON, 0644) — called by `--initialize` when the file is absent.
pub fn save_params(config: &Config) -> Result<PathBuf, String> {
    let path = params_path(&config.home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {parent:?}: {e}"))?;
    }
    let params = Params {
        endpoint: Some(config.endpoint.clone()),
        namespace: Some(config.namespace.clone()),
        database: Some(config.database.clone()),
        data_dir: Some(relative_to_home(&config.home, &config.data_dir)),
        log_dir: Some(relative_to_home(&config.home, &config.log_dir)),
        log_filter: Some(config.log_filter.clone()),
    };
    let text = serde_json::to_string_pretty(&params).map_err(|e| e.to_string())?;
    std::fs::write(&path, text + "\n").map_err(|e| format!("write {path:?}: {e}"))?;
    Ok(path)
}

fn relative_to_home(home: &Path, p: &Path) -> String {
    p.strip_prefix(home)
        .unwrap_or(p)
        .to_string_lossy()
        .to_string()
}

fn against_home(home: &Path, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        home.join(path)
    }
}

/// Resolve the effective config: CLI flag > env var > params file >
/// built-in fallback. `flag_*` args are the raw CLI Options (None =
/// user didn't pass the flag; clap env-vars are folded into the same
/// Option, which keeps env above the file automatically).
#[allow(clippy::too_many_arguments)] // skill-allow: allow — one resolution seam, grouped by field
pub fn resolve(
    home: PathBuf,
    params: &Params,
    flag_endpoint: Option<String>,
    flag_namespace: Option<String>,
    flag_database: Option<String>,
    flag_data_dir: Option<PathBuf>,
    flag_log_dir: Option<PathBuf>,
    flag_log_filter: Option<String>,
) -> Config {
    let endpoint = flag_endpoint
        .or_else(|| params.endpoint.clone())
        .unwrap_or_else(|| FALLBACK_ENDPOINT.to_string());
    let namespace = flag_namespace
        .or_else(|| params.namespace.clone())
        .unwrap_or_else(|| FALLBACK_NAMESPACE.to_string());
    let database = flag_database
        .or_else(|| params.database.clone())
        .unwrap_or_else(|| FALLBACK_DATABASE.to_string());
    let data_dir = flag_data_dir.unwrap_or_else(|| {
        against_home(
            &home,
            params.data_dir.as_deref().unwrap_or(FALLBACK_DATA_DIR),
        )
    });
    let log_dir = flag_log_dir.unwrap_or_else(|| {
        against_home(&home, params.log_dir.as_deref().unwrap_or(FALLBACK_LOG_DIR))
    });
    let log_filter = flag_log_filter
        .or_else(|| params.log_filter.clone())
        .unwrap_or_else(|| FALLBACK_LOG_FILTER.to_string());
    Config {
        home,
        endpoint,
        namespace,
        database,
        data_dir,
        log_dir,
        log_filter,
    }
}
