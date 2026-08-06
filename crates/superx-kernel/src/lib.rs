//! SuperX kernel — the core of the agentic operating system.
//!
//! Rebuilt from line zero per the 2026-08-06 reset ([`BLUEPRINT.md`]).
//! The kernel owns exactly five things: substrate access (SurrealDB is
//! the OS's filesystem), boot, the agent-telemetry capture engine, the
//! operator CLI surface, and the module system. Everything else is a
//! module.
//!
//! Substrate verbs land in G2, after the G1 schema design session
//! produces `SUPERX_SCHEMA.md` v2 and the operator applies it.
//!
//! [`BLUEPRINT.md`]: https://github.com/k8nstantin/superx/blob/main/BLUEPRINT.md
