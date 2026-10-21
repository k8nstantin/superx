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
//! READ-ONLY, ABSOLUTELY. It reads the kernel's captured sessions with
//! SELECT and writes nothing outside its own database. It reads
//! repositories with git and never fetches, writes or checks out, and runs
//! no program a repository names (`--no-textconv`, no external diff, no
//! optional locks).
//!
//! MODULE CONTRACT (D17):
//!
//!   own database    `superx/repos`, its own service account
//!   own schema      `schema/repos.surql`, applied at provision
//!   own log         `target: "repos"`
//!   own CLI         `superx repos status`
//!   own parameters  `attr_repos_*` on its registry entity ([`params`])
//!
//! Other modules read it through its CLI (D24), never through this crate:
//! nothing else in the workspace names it except the binary's link line.

pub mod params;
pub mod status;

use async_trait::async_trait;
use linkme::distributed_slice;
use superx_kernel::{Kernel, KernelError, KernelModule, KernelModuleDescriptor, NodeKind, Result, KERNEL_MODULES};

/// The module's name: its database, CLI namespace and log target.
pub const MODULE_NAME: &str = "repos";

/// The module's own schema, applied by `superx modules provision repos`
/// into `superx/repos` (the kernel schema is untouched, §7).
pub const SCHEMA_DDL: &str = include_str!("../schema/repos.surql");

const USAGE: &str = "usage: superx repos status";

/// The repos module. It holds no state: everything lives in its database.
pub struct ReposModule;

#[async_trait]
impl KernelModule for ReposModule {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: MODULE_NAME,
            version: env!("CARGO_PKG_VERSION"),
            kind: NodeKind::KernelModule,
            depends_on: &[],
            required_metamodel: &[],
        }
    }

    async fn startup(&self, kernel: &Kernel) -> Result<()> {
        // Provisioning is the operator's step; an unprovisioned module must
        // not fail the boot walk.
        match kernel.module_db(MODULE_NAME).await {
            Ok(_) => tracing::info!(target: "repos", "own database ready"),
            Err(e) => tracing::warn!(target: "repos", "own database unavailable, idle until provisioned: {e}"),
        }
        Ok(())
    }

    fn schema_ddl(&self) -> Option<&'static str> {
        Some(SCHEMA_DDL)
    }

    async fn cli(&self, kernel: &Kernel, args: &[String]) -> Result<String> {
        match args.first().map(String::as_str) {
            Some("status") | None => status::render(kernel).await,
            _ => Err(KernelError::Module(USAGE.into())),
        }
    }
}

#[distributed_slice(KERNEL_MODULES)]
static REPOS_REGISTRATION: &'static (dyn KernelModule + Sync) = &ReposModule;
