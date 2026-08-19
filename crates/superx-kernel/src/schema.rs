/// The locked kernel DDL, embedded verbatim from the source-of-truth
/// file at `schema/kernel.surql`.
///
/// The string contains a single `$SUPERX_KERNEL_PASSWORD` placeholder
/// in the `DEFINE USER superx_kernel` statement — the same placeholder
/// the operator-owned `scripts/deploy-schema.sh` substitutes via
/// `envsubst` at apply time. Tests that need a live schema substitute
/// the placeholder programmatically before applying to a `mem://`
/// engine.
///
/// Production paths NEVER apply this DDL — schema application is the
/// operator's one-shot root-account step (skill §10 / §11). The kernel
/// itself only connects to a substrate where this DDL is already in
/// effect.
pub const SCHEMA_DDL: &str = include_str!("../../../schema/kernel.surql");

/// The kernel schema version this binary expects. Bumped with every
/// schema delta (SUPERX_SCHEMA.md tracks the history). The running
/// instance's version is stamped as the `attr_schema_version`
/// parameter on the kernel entity; `start` compares and self-upgrades
/// (tolerant re-apply) on mismatch — a new binary on an older
/// substrate must never brick the boot (issue #158).
pub const SCHEMA_VERSION: &str = "2.2";
