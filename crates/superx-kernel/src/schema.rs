/// The locked substrate DDL, embedded verbatim from the source-of-truth
/// file at `schema/superx.surql`.
///
/// The string contains a single `$SUPERX_SERVICE_PASSWORD` placeholder in
/// the `DEFINE USER superx` statement — this matches the placeholder the
/// operator-owned `scripts/deploy-schema.sh` substitutes via `envsubst`
/// at apply time. Tests that need a live schema substitute the
/// placeholder programmatically before calling `db.query(...)`.
///
/// Production paths NEVER apply this DDL — schema application is the
/// operator's one-shot root-account step (SKILL.md §10 / §11). The
/// kernel itself only connects to a substrate where this DDL is
/// already in effect.
pub const SCHEMA_DDL: &str = include_str!("../../../schema/superx.surql");
