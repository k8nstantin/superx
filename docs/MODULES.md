# Contributing a SuperX Module

A module is a **self-contained mini-app** on the SuperX kernel. It gets
the full facility set, all keyed by its unique name: own schema + data
objects (in its OWN database), own dir, own log, own CLI, own
substrate parameters, a UUIDv7 identity, live enable/disable, and a
first-class row in the kernel's module ledger. Modules depend on the
kernel ONLY — never on each other; everything cross-module flows
through the substrate and the telemetry firehose.

The reference implementation is
[`crates/superx-mod-hello`](../crates/superx-mod-hello) — copy it.
The full-scale example is the UI module
([`crates/superx-mod-ui`](../crates/superx-mod-ui)).

## The contract, step by step

1. **Crate**: `crates/superx-mod-<name>/`, depending on `superx-kernel`
   (and `superx-ops` if you render CLI-style output). Use the kernel's
   re-exported `superx_kernel::types` — no direct surrealdb dependency.
2. **Register** (compiled-in v1 contract):

   ```rust
   pub struct MyModule;

   #[async_trait::async_trait]
   impl KernelModule for MyModule {
       fn descriptor(&self) -> KernelModuleDescriptor { /* name, version, kind, deps */ }
       async fn startup(&self, kernel: &Kernel) -> Result<()> { /* idempotent */ }
       fn schema_ddl(&self) -> Option<&'static str> { Some(include_str!("../schema/my.surql")) }
       fn needs_dir(&self) -> bool { true }
       async fn cli(&self, kernel: &Kernel, args: &[String]) -> Result<String> { /* superx <name> … */ }
   }

   #[linkme::distributed_slice(KERNEL_MODULES)]
   static REG: &'static (dyn KernelModule + Sync) = &MyModule;
   ```

   Then: add the crate to the workspace `members`, and a
   `use superx_mod_<name> as _;` link reference in `crates/superx/src/lib.rs`
   so the registration survives linking. Rebuild = installed.
3. **Own schema** (optional): ship `schema/<name>.surql` defining your
   own service account (`superx_mod_<name>`, password placeholder
   `$SUPERX_MODULE_PASSWORD`) and your tables — append-only SCD-2
   style, `PERMISSIONS FOR update/delete NONE` documented like the
   kernel's. The operator applies it with
   `superx modules provision <name>` (into database `superx/<name>`);
   your code reaches it via `kernel.module_db("<name>")`. The KERNEL
   schema is locked — you never touch it.
4. **Own dir**: declare `needs_dir()`; use `kernel.module_dir("<name>")`
   (`<home>/modules/<name>/`).
5. **Own log**: log via `tracing` with `target: "<name>"` — events
   route to `<home>/modules/<name>/logs/<name>.log.<date>` (and the
   merged self-log).
6. **Own CLI**: implement `cli()`; users call `superx <name> [args…]`.
7. **Telemetry**: emit typed events for everything observable
   (`kernel.log_telemetry*`); the firehose is the OS's audit trail and
   every module may observe it.
8. **Parameters, never hardcoded knobs**: runtime tunables are
   substrate parameters on YOUR registry entity
   (`kernel.get_parameter`); bootstrap fallbacks carry `skill-allow`
   markers and must be overridable.
9. **Tests + gates**: contract tests on the `mem://` engine (see
   hello's tests); the repo gates are `cargo test --workspace`,
   `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
   `python3 tools/skill_audit.py` — all green before any PR.
10. **Process**: GitHub issue → branch `feat/<issue>-…` → PR
    `closes #<issue>`. PRs touching `crates/*/schema/*.surql` need an
    `Operator-approved:` marker in the body.

## Same-kind modules coexist

Two UIs (yours + a contributed one) run side by side: distinct names →
distinct databases, dirs, logs, CLI namespaces (`superx ui …`,
`superx ui-contrib …`), parameters (e.g. different ports), UUIDv7
identities. Nothing in the framework assumes a singleton.

## Runtime management

```
superx modules list                 # inventory × substrate: intent, lifecycle, PROV, uuid7
superx modules enable|disable <n>   # live effect within one capture tick (uuid fragment works)
superx modules provision <n>        # apply the module's own schema (operator path)
superx <n> …                        # the module's own CLI
superx logs --module <n>            # its own log (also under <home>/modules/<n>/logs/)
```
