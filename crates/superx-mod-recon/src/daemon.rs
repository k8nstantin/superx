//! Starting and stopping `superx-recon` on its own (operator, 2026-10-24):
//! its own process, independent of the `superx` daemon, so a crash, a
//! restart or a deploy of either never touches the other.
//!
//! `start` runs this executable again as `run` in the background, in its
//! own process group as `superx` runs its daemon, its output appended to
//! its log, and waits for its pid file, which `run` writes once it serves;
//! `stop` asks it to stop (SIGINT) and waits for it to go.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::instance::Instance;

/// How long `start` waits for the process to write its pid file, and
/// `stop` for it to exit, in 200 ms polls.
const WAIT_POLLS: u32 = 150; // skill-allow: §9-const — start/stop poll bound, 30 s
const POLL: Duration = Duration::from_millis(200); // skill-allow: §9-duration — start/stop poll cadence

/// The executable's name, as the system reports a running one.
const EXE_NAME: &str = "superx-recon";

/// The most of a process's name Linux keeps (`TASK_COMM_LEN` less its NUL,
/// proc(5)): its `ps` cuts a longer name to this, and `superx-recon` is
/// shorter. macOS's `ps` prints the whole path.
const LINUX_NAME_LEN: usize = 15; // skill-allow: §9-const — Linux's own process-name limit, not a policy tunable

/// The pid in the pid file, when the process it names is alive and is
/// this executable.
#[must_use]
pub fn live_pid(instance: &Instance) -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(instance.pid_path()).ok()?.trim().parse().ok()?;
    is_ours(pid).then_some(pid)
}

/// Whether the name `ps` prints for a process is this executable's: the
/// whole name, or as much of it as Linux keeps.
#[must_use]
pub fn names_this_executable(name: &str) -> bool {
    name == EXE_NAME || (name.len() == LINUX_NAME_LEN && EXE_NAME.starts_with(name))
}

/// Whether a pid is a running `superx-recon`. A process that died
/// without removing its pid file (a power loss) leaves a pid the system may
/// have given to another program since: `stop` never signals that one, and
/// `start` does not take it for a running module. One that has exited and
/// is not yet reaped (state `Z`) is not running either, though `kill -0`
/// still answers for it; its state says so wherever `ps` runs, whatever
/// name each system's `ps` prints for it.
fn is_ours(pid: u32) -> bool {
    Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("stat=")
        .arg("-o")
        .arg("comm=")
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .is_some_and(|line| {
            let line = line.trim();
            let (state, comm) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            !state.starts_with('Z')
                && Path::new(comm.trim()).file_name().and_then(|name| name.to_str()).is_some_and(names_this_executable)
        })
}

/// Start the module in the background, unless it is already running.
///
/// # Errors
///
/// What kept it from starting: a log it cannot open, a spawn that failed,
/// or a process that exited before it wrote its pid file.
pub fn start(instance: &Instance, exe: &Path) -> Result<String, String> {
    if let Some(pid) = live_pid(instance) {
        return Ok(format!("superx-recon is already running (pid {pid})\n"));
    }
    std::fs::create_dir_all(instance.log_dir()).map_err(|e| format!("{}: {e}", instance.log_dir().display()))?;
    let log_path = instance.log_dir().join("superx-recon.out");
    let open = || {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| format!("{}: {e}", log_path.display()))
    };
    let mut command = Command::new(exe);
    command.arg("--home").arg(&instance.home).arg("run").stdin(Stdio::null()).stdout(open()?).stderr(open()?);
    // Its own process group, as `superx` starts its daemon: a ctrl-c or a
    // hangup at the terminal that started it never reaches it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|e| format!("spawn {}: {e}", exe.display()))?;
    for _ in 0..WAIT_POLLS {
        if let Some(pid) = live_pid(instance) {
            return Ok(format!("superx-recon running in the background (pid {pid}), its log in {}\n", instance.log_dir().display()));
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("superx-recon exited before it was running ({status}): see {}", log_path.display()));
        }
        std::thread::sleep(POLL);
    }
    Err(format!("superx-recon did not report running: see {}", log_path.display()))
}

/// Ask the running module to stop, and wait for it to go.
///
/// # Errors
///
/// A signal that could not be sent, or a process still running after the
/// wait.
pub fn stop(instance: &Instance) -> Result<String, String> {
    let Some(pid) = live_pid(instance) else {
        return Ok("superx-recon is not running\n".to_string());
    };
    let sent = Command::new("kill").arg("-INT").arg(pid.to_string()).status().is_ok_and(|s| s.success());
    if !sent {
        return Err(format!("could not signal pid {pid}"));
    }
    for _ in 0..WAIT_POLLS {
        if !is_ours(pid) {
            return Ok(format!("superx-recon stopped (pid {pid})\n"));
        }
        std::thread::sleep(POLL);
    }
    Err(format!("superx-recon (pid {pid}) is still stopping: check `superx-recon status` shortly"))
}

/// Own the pid file while running: written now, removed when dropped.
pub struct PidFile(std::path::PathBuf);

impl PidFile {
    /// Write this process's pid.
    ///
    /// # Errors
    ///
    /// The pid file's path and why it could not be written.
    pub fn write(instance: &Instance) -> Result<Self, String> {
        let path = instance.pid_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, std::process::id().to_string()).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.0) {
            tracing::warn!(target: "recon", error = %e, path = %self.0.display(), "pid file not removed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::names_this_executable;

    #[test]
    fn names_this_executable_accepts_superx_recon_and_refuses_every_other_name() {
        assert!(names_this_executable("superx-recon"));
        for other in ["superx-performance", "superx-repos", "superx-reco", "superx-recon2", "superx", ""] {
            assert!(!names_this_executable(other), "{other}");
        }
    }
}
