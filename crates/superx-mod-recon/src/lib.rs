//! superx-mod-recon — the models, the agents, thinking effort and the
//! operator, measured (#466).
//!
//! ONE MODULE, ONE SCHEMA, ONE CRAWLER (operator, 2026-10-25): the main
//! tool. It gathers what the agents did and what the operator asked from
//! every line the kernel captured, and what became of the work from the git
//! repositories the agents worked in, and keeps both in one database for
//! correlation and analysis: which model answered, how long it thought and
//! what it spent, which tools it ran and how they ended, which files it
//! changed, what it committed, tested and built, and what of it landed and
//! lasted. One crawler fills it through its sources ([`crawler`]); a new
//! source (the GitHub and Bitbucket hosts, others) plugs in beside them.
//!
//! ITS OWN EXECUTABLE (operator, 2026-10-24): `superx-recon`, with its
//! own process, pid file and log under `<home>/modules/recon/`, its
//! own database (`superx/recon`, from `schema/recon.surql`) and
//! its own URL. It is never linked into `superx`: nothing it does can reach
//! capture, the activity feed, the sessions, the export or the stats. It
//! keeps its entry in the OS's registry while it runs, so `superx status`
//! and the core UI's Status page list it and
//! `superx param set recon <name> <json>` tunes it, and it publishes
//! its URL there (`attr_module_ui_url`).
//!
//! READ-ONLY toward everything but its own database. It reads the kernel's
//! captured lines with SELECT and the repositories with git, read-only
//! (#446), and writes nothing outside its own database, except its own
//! registry entry.

pub mod crawler;
pub mod credentials;
pub mod daemon;
pub mod folder;
pub mod instance;
pub mod params;
pub mod registry;
pub mod server;
pub mod status;

use superx_kernel::{KernelModuleDescriptor, NodeKind};

/// The module's name: its database, its registry entry and its log target.
pub const MODULE_NAME: &str = "recon";

/// The module's own schema, applied by `superx-recon --initialize`
/// into `superx/recon` (the kernel schema is untouched, §7).
pub const SCHEMA_DDL: &str = include_str!("../schema/recon.surql");

/// What the module registers itself as in the OS's registry.
#[must_use]
pub fn descriptor() -> KernelModuleDescriptor {
    KernelModuleDescriptor {
        name: MODULE_NAME,
        version: env!("CARGO_PKG_VERSION"),
        kind: NodeKind::KernelModule,
        depends_on: &[],
        required_metamodel: &[],
    }
}
