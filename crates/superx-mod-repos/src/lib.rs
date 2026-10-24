//! superx-mod-repos — the git repositories the agents work in, read
//! read-only and kept (#466).
//!
//! The kernel captures what the agents say and do. What their work did to
//! the repositories (what landed on each main line, what was left on a
//! branch, what still stands) only git knows. This module finds the
//! repositories from the working directories in the captured sessions and
//! from the roots the operator names, reads them with git, and keeps what
//! it reads in its own database, so a reader asks the database rather than
//! running git on every request.
//!
//! ITS OWN EXECUTABLE (operator, 2026-10-24): `superx-repos`, with its own
//! process, pid file and log under `<home>/modules/repos/`, its own database
//! (`superx/repos`, from `schema/repos.surql`) and its own URL. It is never
//! linked into `superx`: nothing it does can reach capture, the activity
//! feed, the sessions, the export or the stats. It keeps its entry in the
//! OS's registry while it runs, so `superx status` and the core UI's Status
//! page list it and `superx param set repos <name> <json>` tunes it, and it
//! publishes its URL there (`attr_module_ui_url`), where the core UI links
//! to it. (`superx modules list` lists what is linked into `superx`, so
//! not this.)
//!
//! READ-ONLY, ABSOLUTELY. It reads the kernel's captured sessions with
//! SELECT and writes nothing outside its own database, except its own
//! registry entry. It reads repositories with git and never fetches, writes
//! or checks out, and runs no program a repository names (`--no-textconv`,
//! no external diff, no optional locks).

pub mod daemon;
pub mod instance;
pub mod params;
pub mod registry;
pub mod server;
pub mod status;

use superx_kernel::{KernelModuleDescriptor, NodeKind};

/// The module's name: its database, its registry entry and its log target.
pub const MODULE_NAME: &str = "repos";

/// The module's own schema, applied by `superx-repos --initialize` into
/// `superx/repos` (the kernel schema is untouched, §7).
pub const SCHEMA_DDL: &str = include_str!("../schema/repos.surql");

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
