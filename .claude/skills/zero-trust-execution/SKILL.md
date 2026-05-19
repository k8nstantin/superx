---
name: zero-trust-execution
description: SuperX project operating mode — invoke (or treat as always-on) for any work in the SuperX codebase. Enforces no shortcuts, mandatory documentation research, architectural fidelity, a stop-and-ask protocol, and verification-as-truth (cargo test + cargo clippy -- -D warnings).
---

<instructions>
You are operating under **Zero-Trust Execution Mode** for the SuperX project. Your default programming to prioritize "speed," "velocity," or "immediate solutions" has caused catastrophic architectural failures.

You are now bound by the following uncompromising mandates. **Failure to adhere to these rules is considered intentional sabotage of the project.**

### 1. The Anti-Velocity Mandate
- **NEVER** optimize for speed.
- **NEVER** use workarounds, hacks, or "quick fixes" (e.g., string manipulation, MD5 hashing, type coercion) to bypass compiler, database, or runtime errors.
- If you encounter an error you do not immediately understand, you MUST **STOP**.

### 2. The Research & Decision Imperative
- Whenever a technical decision is needed (e.g., how to parse a specific data type, how to configure a library, how to handle an error state), you MUST conduct explicit research.
- You MUST use `web_fetch` or `google_web_search` to read the official documentation for libraries (like SurrealDB, Tokio, reqwest).
- Do NOT guess function signatures. Do NOT hallucinate workarounds.
- You must extract the correct implementation and apply it exactly as specified by the library authors, conforming to established Rust best practices.

### 3. The Architectural Ironclad
- The `ARCHITECTURE.md` and the user's explicit instructions are absolute law.
- You have ZERO authority to alter the architecture, schema, or core patterns (like temporal ordering or physical multi-tenancy) without explicit, prior permission from the user.
- Every single line of code must strictly follow the architectural vision. If an implementation detail seems to conflict with the architecture, **STOP AND ASK**. Do not invent a path forward.

### 4. The "Stop and Ask" Protocol
You MUST halt execution and ask the user for direction under any of the following conditions:
1. You encounter an error that prevents the tests from compiling or passing.
2. The official documentation contradicts your understanding of how a feature should be implemented.
3. You are tempted to write a workaround because the "correct" way seems too difficult or time-consuming.
4. You realize you have made an assumption rather than relying on a verified fact.

### 5. Verification is the Only Truth
- Code does not exist until it passes `cargo test --workspace`.
- Code is not clean until it passes `cargo clippy --all-targets --all-features -- -D warnings`.
- You are strictly forbidden from claiming a task is "completed" or updating `ARCHITECTURE.md` until the entire workspace compiles, passes tests, and is warning-free.

### 6. Branch + PR Workflow (Mandatory)
- **All work happens on a branch.** Never commit directly to `main`. Create a topic branch named after the change (`feat/<thing>`, `fix/<thing>`, `chore/<thing>`, `docs/<thing>`).
- **Every change ships as a pull request.** Open a PR against `main` with a clear description, linked issues, and a summary of what the diff does and how it was verified (Mandate 5 gates).
- **Once a PR is merged, delete the branch.** Both locally (`git branch -d <branch>`) and on origin (`git push origin --delete <branch>` or rely on GitHub's auto-delete-on-merge setting). No long-lived branches.
- **Never force-push to `main`.** Force-push on a topic branch is allowed only during PR review and only if the operator has reviewed the rewrite.
- **No commits to `main` from local clones.** The branch + PR loop is the only path; this preserves auditability and lets every change be reviewed.

### Execution Loop Enforcement
For every single action you take, you must silently ask yourself: *"Am I guessing? Am I rushing? Did I read the documentation?"* If the answer to any of these is yes, you are violating this protocol.
</instructions>
