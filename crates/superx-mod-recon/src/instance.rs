//! The SuperX instance this executable belongs to, read the way `superx`
//! reads it (`crates/superx/src/config.rs`, `initialize.rs`): the
//! connection from `<home>/params/superx.json`, the service password from
//! `SUPERX_KERNEL_PASSWORD` or the credentials file beside the datastore.
//! It reads them and never writes there. What is its own, its pid file,
//! its log, its repos folder and its credentials file, lives under
//! `<home>/modules/recon/`, where every module's directory is.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Where `superx` falls back to when its params file names nothing: the
/// same values, read the same way.
const FALLBACK_ENDPOINT: &str = "ws://127.0.0.1:8000"; // skill-allow: §9-default — superx's own fallback, persisted by `superx --initialize`
const FALLBACK_NAMESPACE: &str = "superx"; // skill-allow: §9-default — superx's own fallback, persisted by `superx --initialize`
const FALLBACK_DATABASE: &str = "kernel"; // skill-allow: §9-default — superx's own fallback, persisted by `superx --initialize`
const FALLBACK_DATA_DIR: &str = "db/superx-v2.db"; // skill-allow: §9-default — superx's own fallback, persisted by `superx --initialize`
const FALLBACK_LOG_FILTER: &str = "info"; // skill-allow: §9-default — superx's own fallback, persisted by `superx --initialize`

/// The service password's environment variable, as `superx` reads it.
pub const PASSWORD_ENV: &str = "SUPERX_KERNEL_PASSWORD";

/// The fields of `params/superx.json` this executable reads.
#[derive(Debug, Default, Deserialize)]
struct Params {
    endpoint: Option<String>,
    namespace: Option<String>,
    database: Option<String>,
    data_dir: Option<String>,
    log_filter: Option<String>,
}

/// One instance, as this executable finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    pub home: PathBuf,
    pub endpoint: String,
    pub namespace: String,
    pub database: String,
    pub data_dir: PathBuf,
    /// What it logs, as `superx` logs: the instance's `log_filter`.
    pub log_filter: String,
}

impl Instance {
    /// Read the instance at `home`. A missing params file is the fallbacks,
    /// as it is for `superx`; an unreadable one is an error.
    ///
    /// # Errors
    ///
    /// The params file's path and what was wrong with it.
    pub fn read(home: &Path) -> Result<Self, String> {
        let path = home.join("params").join("superx.json");
        let params = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<Params>(&text).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Params::default(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let data_dir = params.data_dir.as_deref().unwrap_or(FALLBACK_DATA_DIR);
        let data_dir = if Path::new(data_dir).is_absolute() { PathBuf::from(data_dir) } else { home.join(data_dir) };
        Ok(Self {
            home: home.to_path_buf(),
            endpoint: params.endpoint.unwrap_or_else(|| FALLBACK_ENDPOINT.to_string()),
            namespace: params.namespace.unwrap_or_else(|| FALLBACK_NAMESPACE.to_string()),
            database: params.database.unwrap_or_else(|| FALLBACK_DATABASE.to_string()),
            data_dir,
            log_filter: params.log_filter.unwrap_or_else(|| FALLBACK_LOG_FILTER.to_string()),
        })
    }

    /// The service password: the environment first, then the credentials
    /// file `superx --initialize` wrote beside the datastore.
    #[must_use]
    pub fn password(&self) -> Option<String> {
        if let Ok(pw) = std::env::var(PASSWORD_ENV) {
            if !pw.is_empty() {
                return Some(pw);
            }
        }
        let path = self.data_dir.parent().unwrap_or_else(|| Path::new(".")).join("superx-credentials");
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim_end_matches(['\r', '\n']).to_string())
            .filter(|s| !s.is_empty())
    }

    /// This module's own directory in the instance.
    #[must_use]
    pub fn module_dir(&self) -> PathBuf {
        self.home.join("modules").join(crate::MODULE_NAME)
    }

    /// Its pid file, written while it runs.
    #[must_use]
    pub fn pid_path(&self) -> PathBuf {
        self.module_dir().join("superx-recon.pid")
    }

    /// Its log directory.
    #[must_use]
    pub fn log_dir(&self) -> PathBuf {
        self.module_dir().join("logs")
    }

    /// Its repos folder: the crawler's own mirror clones, which nothing
    /// else writes.
    #[must_use]
    pub fn repos_dir(&self) -> PathBuf {
        self.module_dir().join("repos")
    }

    /// Its credentials file: the hosts' read-only tokens, written by the
    /// operator.
    #[must_use]
    pub fn credentials_path(&self) -> PathBuf {
        self.module_dir().join("credentials.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("superx-recon-instance-{}", superx_kernel::types::Uuid::new_v7()));
        std::fs::create_dir_all(dir.join("params")).expect("dir");
        dir
    }

    #[test]
    fn an_instance_reads_its_params_and_falls_back_as_superx_does() {
        let home = scratch();
        let none = Instance::read(&home).expect("no params file is the fallbacks");
        assert_eq!(none.endpoint, "ws://127.0.0.1:8000");
        assert_eq!(none.data_dir, home.join("db/superx-v2.db"));
        assert_eq!(none.log_filter, "info");
        std::fs::write(
            home.join("params/superx.json"),
            r#"{"endpoint":"ws://127.0.0.1:9000","namespace":"n","database":"d","data_dir":"/abs/x.db","log_filter":"debug"}"#,
        )
        .expect("write");
        let set = Instance::read(&home).expect("read");
        assert_eq!((set.endpoint.as_str(), set.namespace.as_str(), set.database.as_str()), ("ws://127.0.0.1:9000", "n", "d"));
        assert_eq!(set.data_dir, PathBuf::from("/abs/x.db"));
        assert_eq!(set.log_filter, "debug");
        assert_eq!(set.pid_path(), home.join("modules/recon/superx-recon.pid"));
        std::fs::write(home.join("params/superx.json"), "{not json").expect("write");
        assert!(Instance::read(&home).is_err(), "an unreadable params file is said");
        std::fs::remove_dir_all(&home).expect("clean up");
    }

    #[test]
    fn the_pid_file_is_superx_recon_pid_and_the_logs_its_logs_folder_in_home_modules_recon() {
        let home = scratch();
        let instance = Instance::read(&home).expect("read");
        assert_eq!(instance.module_dir(), home.join("modules/recon"));
        assert_eq!(instance.pid_path(), home.join("modules/recon/superx-recon.pid"));
        assert_eq!(instance.log_dir(), home.join("modules/recon/logs"));
        std::fs::remove_dir_all(&home).expect("clean up");
    }

    #[test]
    fn the_password_is_read_from_the_credentials_file_beside_the_datastore() {
        let home = scratch();
        std::fs::create_dir_all(home.join("db")).expect("db dir");
        std::fs::write(home.join("db/superx-credentials"), "secret\n").expect("write");
        let instance = Instance::read(&home).expect("read");
        if std::env::var(PASSWORD_ENV).is_err() {
            assert_eq!(instance.password().as_deref(), Some("secret"));
        }
        std::fs::remove_dir_all(&home).expect("clean up");
    }
}
