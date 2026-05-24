/// The locked kernel DDL, embedded verbatim from the source-of-truth
/// file at `schema/kernel.surql`.
///
/// The string contains a single `$SUPERX_KERNEL_PASSWORD` placeholder
/// in the `DEFINE USER superx_kernel` statement — this matches the
/// placeholder the operator-owned `scripts/deploy-schema.sh`
/// substitutes via `envsubst` at apply time. Tests that need a live
/// schema substitute the placeholder programmatically before calling
/// `db.query(...)`.
///
/// Production paths NEVER apply this DDL — schema application is the
/// operator's one-shot root-account step (SKILL.md §10 / §11). The
/// kernel itself only connects to a substrate where this DDL is
/// already in effect.
///
/// Drivers and apps each ship their own schemas in their own crates.
/// Only the kernel's DDL is referenced from this constant.
pub const SCHEMA_DDL: &str = include_str!("../../../schema/kernel.surql");
