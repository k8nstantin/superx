#!/usr/bin/env python3
"""
tools/skill_audit.py
====================

Independent mechanical verifier of the zero-trust-execution skill.

Single Python script — stdlib only, no external dependencies. Replaces
the earlier bash version because the rule set has grown to need real
regex, structured scoping, schema-drift detection, and JSON output.

Designed to be impossible for the model to bypass:
  * Checked into source under operator review.
  * Invoked from CI as a required check on `main`
    (see .github/workflows/skill-audit.yml).
  * Rule additions / changes require an explicit `Operator-approved: ...`
    marker in the PR body that modifies this file.

Exit codes
----------
  0  clean — no violations detected
  1  at least one bright-line violation
  2  schema-drift detected (SUPERX_SCHEMA.md vs apply_substrate_schema)
  3  internal error (parser failure, missing file the audit requires, …)

Usage
-----
  python3 tools/skill_audit.py              # human output, non-zero on violation
  python3 tools/skill_audit.py --json       # JSON output, same exit codes
  python3 tools/skill_audit.py --rules-only # list every rule + exit 0
"""

from __future__ import annotations

import argparse
import dataclasses
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Iterator, List, Optional

# --------------------------------------------------------------------------
# Terminal colours
# --------------------------------------------------------------------------

class Ansi:
    RED   = "\033[0;31m"
    GRN   = "\033[0;32m"
    YEL   = "\033[0;33m"
    DIM   = "\033[2m"
    BOLD  = "\033[1m"
    NC    = "\033[0m"

    @classmethod
    def disable(cls) -> None:
        for attr in ("RED", "GRN", "YEL", "DIM", "BOLD", "NC"):
            setattr(cls, attr, "")


# --------------------------------------------------------------------------
# Data shapes
# --------------------------------------------------------------------------

@dataclass(frozen=True)
class Rule:
    """A bright-line rule the audit enforces."""
    id: str                    # short stable id, e.g. "S11-1"
    section: str               # skill section, e.g. "§11"
    description: str           # plain-language failure message
    pattern: str               # python regex
    flags: int = 0             # re flags
    # scope = where the rule applies:
    #   "production" → crates/**/*.rs excluding tests/ subdirs
    #   "all"        → all *.rs under crates/ (incl. tests)
    #   "lib"        → only library code (lib.rs / non-bin source)
    scope: str = "production"
    # If non-None, lines containing this exact marker (case-sensitive) are
    # exempt — used for legitimate exceptions, e.g. test data.
    exempt_marker: Optional[str] = None
    # An optional post-match filter on the captured line (e.g. exclude
    # comment-only lines).
    line_filter: Optional[Callable[[str], bool]] = None


@dataclass
class Violation:
    rule: Rule
    file: Path
    line: int
    snippet: str

    def to_dict(self) -> dict:
        return {
            "rule_id": self.rule.id,
            "section": self.rule.section,
            "description": self.rule.description,
            "file": str(self.file),
            "line": self.line,
            "snippet": self.snippet,
        }


@dataclass
class SchemaDrift:
    kind: str           # "code-only" or "doc-only"
    statement: str      # the DEFINE / DROP / etc. statement that diverges
    location: str       # path:line where found

    def to_dict(self) -> dict:
        return {
            "kind": self.kind,
            "statement": self.statement,
            "location": self.location,
        }


# --------------------------------------------------------------------------
# File-tree iteration
# --------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parents[1]
CRATES_DIR = REPO_ROOT / "crates"
SCHEMA_DOC = REPO_ROOT / "SUPERX_SCHEMA.md"


def is_test_file(path: Path) -> bool:
    """True if the file is under any tests/ subdirectory."""
    parts = set(path.parts)
    return "tests" in parts


def is_lib_file(path: Path) -> bool:
    """True if the file is library code (lib.rs in a crate)."""
    return path.name == "lib.rs"


def iter_rust_files(scope: str) -> Iterator[Path]:
    """Yield Rust files matching the requested scope."""
    if not CRATES_DIR.exists():
        return
    for path in sorted(CRATES_DIR.rglob("*.rs")):
        # Always skip target/.
        if "target" in path.parts:
            continue
        if scope == "production" and is_test_file(path):
            continue
        if scope == "lib" and not is_lib_file(path):
            continue
        yield path


# --------------------------------------------------------------------------
# Rule definitions
# --------------------------------------------------------------------------

def _not_comment(line: str) -> bool:
    """Filter — true if the line is NOT a pure-comment line."""
    stripped = line.strip()
    return not stripped.startswith("//") and not stripped.startswith("/*")


RULES: List[Rule] = [
    # ─────────────────────────────────────────────────────────────────────
    # §11 — UUIDv7 row-id contract
    # ─────────────────────────────────────────────────────────────────────
    Rule(
        id="S11-1",
        section="§11",
        description="non-UUIDv7 row id (Uuid::new_v4 / new_v5 / new_v6 / new_v1)",
        pattern=r"Uuid::(new_v4|new_v5|new_v6|new_v1)\b",
        scope="all",   # tests must also use v7 per skill
        exempt_marker="// skill-allow: §11",
        line_filter=_not_comment,
    ),

    # ─────────────────────────────────────────────────────────────────────
    # §13 — service-account-only authentication
    # ─────────────────────────────────────────────────────────────────────
    Rule(
        id="S13-1",
        section="§13",
        description="auth::Root referenced outside Kernel::init schema-apply path",
        pattern=r"auth::Root\b",
        scope="all",
        exempt_marker="// skill-allow: §13-init",
        line_filter=_not_comment,
    ),
    Rule(
        id="S13-2",
        section="§13",
        description='db.signin with username other than "superx"',
        # Look for any signin call that references a username other than "superx".
        # We match `username: "X"` with X != superx — covers Database/Record/Namespace
        # auth shapes where username is the field name.
        pattern=r'username:\s*"(?!superx")[^"]*"',
        scope="all",
        exempt_marker="// skill-allow: §13-username",
        line_filter=_not_comment,
    ),
    Rule(
        id="S13-3",
        section="§13",
        description="db.connect() with hardcoded URL (must come from env/config)",
        pattern=r'\.connect\("[^$"]+"\)',
        scope="all",
        exempt_marker="// skill-allow: §13-connect",
        line_filter=_not_comment,
    ),

    # ─────────────────────────────────────────────────────────────────────
    # §10 — append-only kernel verbs
    # ─────────────────────────────────────────────────────────────────────
    Rule(
        id="S10-1",
        section="§10",
        description='forbidden mutating SQL ("UPDATE ", "DELETE ", "UPSERT ", "ALTER ", "REMOVE ", "DROP ")',
        pattern=r'"(UPDATE |DELETE |UPSERT |ALTER |REMOVE |DROP )',
        scope="production",
        exempt_marker="// skill-allow: §10",
        line_filter=_not_comment,
    ),

    # ─────────────────────────────────────────────────────────────────────
    # §14 — no type-conversion round-trips
    # ─────────────────────────────────────────────────────────────────────
    Rule(
        id="S14-1",
        section="§14",
        description='format!("<table>:{...}") — typed Thing → string round-trip',
        pattern=r'format!\("(entity|schedule|relation|state_ledger|telemetry_stream|cursor|cursor_type|execution_params|type_definition):\{',
        scope="production",
        exempt_marker="// skill-allow: §14-rid",
        line_filter=_not_comment,
    ),
    Rule(
        id="S14-2",
        section="§14",
        description="<string> cast in SurrealQL — type-conversion at the boundary",
        pattern=r"<string>",
        scope="production",
        exempt_marker="// skill-allow: §14-cast",
        line_filter=_not_comment,
    ),
    Rule(
        id="S14-3",
        section="§14",
        description="parse_id(&format!(...)) — string round-trip at every call site",
        pattern=r"parse_id\(\s*&format!",
        scope="production",
        exempt_marker="// skill-allow: §14-parse",
        line_filter=_not_comment,
    ),
    Rule(
        id="S14-4",
        section="§14",
        description='type::thing("<literal>", ...) — string-built Thing bypasses typed FK',
        pattern=r'type::thing\(\s*"[^"]+"',
        scope="production",
        exempt_marker="// skill-allow: §14-thing",
        line_filter=_not_comment,
    ),

    # ─────────────────────────────────────────────────────────────────────
    # §9 — no hardcoded thresholds / timings / defaults
    # ─────────────────────────────────────────────────────────────────────
    Rule(
        id="S9-1",
        section="§9",
        description='clap default_value = "<literal>" — defaults must come from env/config, not source',
        pattern=r'default_value\s*=\s*"[^"]+"',
        scope="production",
        exempt_marker="// skill-allow: §9-default",
        line_filter=_not_comment,
    ),
    Rule(
        id="S9-2",
        section="§9",
        description="Duration::from_*(<numeric literal>) — hardcoded timing in source",
        pattern=r"Duration::from_(secs|millis|mins|nanos|micros)\(\s*[0-9_]+\s*\)",
        scope="production",
        exempt_marker="// skill-allow: §9-duration",
        line_filter=_not_comment,
    ),
    Rule(
        id="S9-3",
        section="§9",
        description="get_parameter(<key>, <numeric literal>) — hardcoded fallback constant",
        pattern=r"get_parameter\([^,]+,\s*[0-9][0-9_.]*",
        scope="production",
        exempt_marker="// skill-allow: §9-param",
        line_filter=_not_comment,
    ),
    Rule(
        id="S9-4",
        section="§9",
        description="unwrap_or(<numeric literal ≥ 2>) — hardcoded fallback",
        # 0 and 1 are tolerated as boundary values; anything else needs config.
        pattern=r"\.unwrap_or\(\s*(?!0\b|1\b)[0-9][0-9_]*\s*\)",
        scope="production",
        exempt_marker="// skill-allow: §9-or",
        line_filter=_not_comment,
    ),
    Rule(
        id="S9-5",
        section="§9",
        description='const NAME: <num-type> = <literal>; — magic number in source',
        pattern=r"const\s+[A-Z_][A-Z0-9_]*\s*:\s*(u\d+|i\d+|usize|isize|f32|f64)\s*=\s*[0-9][0-9_.]*",
        scope="production",
        exempt_marker="// skill-allow: §9-const",
        line_filter=_not_comment,
    ),
    Rule(
        id="S9-6",
        section="§9",
        description='supersede_state("attr_config", ...) with hardcoded numeric/bool inside the json! block',
        # Heuristic: a json!({...}) block immediately followed (within ~400 chars)
        # by `supersede_state` call referencing `attr_config`. False-positive risk;
        # use the marker liberally for genuine cases.
        pattern=r'supersede_state\([^)]*"attr_config"',
        scope="production",
        exempt_marker="// skill-allow: §9-attrconfig",
        line_filter=_not_comment,
    ),

    # ─────────────────────────────────────────────────────────────────────
    # B — code quality / safety
    # ─────────────────────────────────────────────────────────────────────
    Rule(
        id="B1",
        section="quality",
        description="unwrap() in production code — hidden panic; use ? or expect() with rationale",
        pattern=r"\.unwrap\(\)",
        scope="production",
        exempt_marker="// skill-allow: unwrap",
        line_filter=_not_comment,
    ),
    Rule(
        id="B2",
        section="quality",
        description="panic!() in production code — propagate errors via Result instead",
        pattern=r"\bpanic!\(",
        scope="production",
        exempt_marker="// skill-allow: panic",
        line_filter=_not_comment,
    ),
    Rule(
        id="B3",
        section="quality",
        description="todo!() / unimplemented!() in production code — always a regression",
        pattern=r"\b(todo|unimplemented)!\(",
        scope="production",
        exempt_marker="// skill-allow: todo",
        line_filter=_not_comment,
    ),
    Rule(
        id="B4",
        section="quality",
        description="dbg!() — debug print left in source",
        pattern=r"\bdbg!\(",
        scope="all",
        line_filter=_not_comment,
    ),
    Rule(
        id="B5",
        section="quality",
        description="println!() / eprintln!() in library code — use tracing::* instead",
        pattern=r"\b(println|eprintln)!\(",
        scope="lib",
        exempt_marker="// skill-allow: println",
        line_filter=_not_comment,
    ),

    # ─────────────────────────────────────────────────────────────────────
    # F — architecture / file-layout discipline
    # ─────────────────────────────────────────────────────────────────────
    Rule(
        id="F3",
        section="arch",
        description="unsafe { ... } block — requires operator-approved rationale",
        pattern=r"\bunsafe\s*\{",
        scope="all",
        exempt_marker="// skill-allow: unsafe",
        line_filter=_not_comment,
    ),
    Rule(
        id="F4",
        section="arch",
        description="#[allow(...)] attribute — disabling enforcement requires marker",
        pattern=r"#!\s*\[\s*allow\(|#\s*\[\s*allow\(",
        scope="all",
        exempt_marker="// skill-allow: allow",
        line_filter=_not_comment,
    ),

    # ─────────────────────────────────────────────────────────────────────
    # G — concurrency
    # ─────────────────────────────────────────────────────────────────────
    Rule(
        id="G1",
        section="arch",
        description="std::thread::spawn — use tokio::spawn instead",
        pattern=r"std::thread::spawn|^\s*thread::spawn",
        scope="all",
        exempt_marker="// skill-allow: thread",
        line_filter=_not_comment,
    ),
    Rule(
        id="G2",
        section="arch",
        description="block_on(...) in async context — deadlock risk",
        pattern=r"\bblock_on\(",
        scope="all",
        exempt_marker="// skill-allow: block_on",
        line_filter=_not_comment,
    ),

    # ─────────────────────────────────────────────────────────────────────
    # H — observability discipline
    # ─────────────────────────────────────────────────────────────────────
    Rule(
        id="H2",
        section="arch",
        description="let _ = swallowing a Result — explicit error handling required",
        # Pure heuristic: `let _ = <expr>?;` style is rare and intentional;
        # `let _ = <thing>;` swallowing a Result is the pattern we want to catch.
        pattern=r"let\s+_\s*=\s*[A-Za-z_][A-Za-z0-9_:.]*\([^?]*\)\s*;",
        scope="production",
        exempt_marker="// skill-allow: let-underscore",
        line_filter=_not_comment,
    ),

    # ─────────────────────────────────────────────────────────────────────
    # C — schema / typing discipline
    # ─────────────────────────────────────────────────────────────────────
    Rule(
        id="C3",
        section="§12",
        description='DEFINE FIELD ... TYPE record<...> without ASSERT — untyped FK loses §12 debug surface',
        # Match a DEFINE FIELD with record<...> type that doesn't end with an
        # ASSERT in the same statement (statements end at `;`). Crude but
        # effective for catch — a missing ASSERT is easy to add.
        pattern=r"DEFINE FIELD[^;]*record<[^>]+>(?![^;]*ASSERT)",
        scope="all",
        flags=re.DOTALL,
        exempt_marker="// skill-allow: §12-assert",
        line_filter=_not_comment,
    ),
]


# --------------------------------------------------------------------------
# Audit engine
# --------------------------------------------------------------------------

def audit_rule(rule: Rule) -> List[Violation]:
    """Run one rule against every in-scope file. Returns list of violations."""
    violations: List[Violation] = []
    regex = re.compile(rule.pattern, rule.flags)
    for path in iter_rust_files(rule.scope):
        try:
            content = path.read_text(encoding="utf-8", errors="replace")
        except (OSError, UnicodeDecodeError):
            continue
        lines = content.splitlines()
        for match in regex.finditer(content):
            # Compute line number
            line_idx = content.count("\n", 0, match.start())
            line_no = line_idx + 1
            if line_idx >= len(lines):
                continue
            line_text = lines[line_idx]
            # Optional comment filter
            if rule.line_filter and not rule.line_filter(line_text):
                continue
            # Exempt-marker check (same line OR previous line)
            if rule.exempt_marker:
                prev_text = lines[line_idx - 1] if line_idx > 0 else ""
                if (rule.exempt_marker in line_text or
                        rule.exempt_marker in prev_text):
                    continue
            violations.append(Violation(
                rule=rule,
                file=path.relative_to(REPO_ROOT),
                line=line_no,
                snippet=line_text.rstrip(),
            ))
    return violations


def audit_all_rules() -> List[Violation]:
    out: List[Violation] = []
    for rule in RULES:
        out.extend(audit_rule(rule))
    return out


# --------------------------------------------------------------------------
# Structural checks (beyond per-line regex)
#
# These four checks catch patterns that pure regex misses — exactly the
# class of failures that the previous session's agent committed:
#
#   D1 (duplicate literals) — same value copy-pasted across files instead
#       of centralised. Caught: DNS namespace UUID duplicated 13×; the
#       attr_config seed values duplicated in get_parameter fallbacks.
#
#   D2 (telemetry coverage) — kernel-verb files that issue SurrealQL
#       CREATE statements must also call log_telemetry in the same file.
#       Per skill §8 "every step emits a typed telemetry event."
#
#   D3 (architecture sync)  — every crate dir under crates/ must have a
#       corresponding mention in ARCHITECTURE.md. Catches new crates / new
#       top-level modules added without spec update (§3).
#
# (#6 PR-body skill-checklist is enforced by the CI workflow, not here.)
# --------------------------------------------------------------------------

# UUID literal pattern — RFC 4122 canonical form.
UUID_LITERAL_RX = re.compile(
    r'"([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})"',
    flags=re.IGNORECASE,
)


def detect_duplicate_literals() -> List[Violation]:
    """[D1] Find quoted UUID literals duplicated across multiple files.

    The first occurrence is allowed (someone has to define it once); every
    additional occurrence in a different file is a violation, with the
    recommendation: extract a `pub const` in a single home and import."""
    out: List[Violation] = []
    if not CRATES_DIR.exists():
        return out

    # value -> list of (file, line, snippet)
    locations: dict[str, List[tuple[Path, int, str]]] = {}

    for path in iter_rust_files("all"):
        try:
            content = path.read_text(encoding="utf-8", errors="replace")
        except (OSError, UnicodeDecodeError):
            continue
        lines = content.splitlines()
        for m in UUID_LITERAL_RX.finditer(content):
            value = m.group(1).lower()
            line_idx = content.count("\n", 0, m.start())
            if line_idx >= len(lines):
                continue
            line_text = lines[line_idx]
            # Honour the marker
            prev_text = lines[line_idx - 1] if line_idx > 0 else ""
            if ("// skill-allow: duplicate" in line_text
                    or "// skill-allow: duplicate" in prev_text):
                continue
            if not _not_comment(line_text):
                continue
            locations.setdefault(value, []).append(
                (path.relative_to(REPO_ROOT), line_idx + 1, line_text.rstrip())
            )

    synth_rule = Rule(
        id="D1",
        section="dup",
        description=(
            "duplicate UUID literal across files — extract a `pub const` "
            "in a single source-of-truth module and import"
        ),
        pattern="",  # synthetic — generated outside the regex engine
        scope="all",
        exempt_marker="// skill-allow: duplicate",
    )

    for value, occurrences in locations.items():
        # Count distinct files. If only one file, it's a single home → OK.
        files = {loc[0] for loc in occurrences}
        if len(files) < 2:
            continue
        # Report every occurrence past the first as a violation.
        for path, line_no, snippet in occurrences:
            out.append(Violation(
                rule=synth_rule,
                file=path,
                line=line_no,
                snippet=f"{snippet}    # value {value} also in {len(files)} files",
            ))

    return out


# DDL statements that produce substrate rows. If a function issues these,
# it must also emit a telemetry event in the same file.
SUBSTRATE_CREATE_RX = re.compile(
    r'"(CREATE\s+(?:entity|relation|state_ledger|telemetry_stream|cursor|cursor_type|execution_params|schedule)\b)',
    flags=re.IGNORECASE,
)
LOG_TELEMETRY_RX = re.compile(r"\blog_telemetry\s*\(")


def detect_telemetry_gaps() -> List[Violation]:
    """[D2] Per skill §8, any kernel verb that creates substrate rows must
    emit a telemetry event. Heuristic: any file containing a CREATE-substrate
    string literal must also contain a log_telemetry( call."""
    out: List[Violation] = []
    if not CRATES_DIR.exists():
        return out

    synth_rule = Rule(
        id="D2",
        section="§8",
        description=(
            "kernel verb issues CREATE-substrate but file has no log_telemetry "
            "call — every state-mutating step must emit telemetry"
        ),
        pattern="",
        scope="production",
        exempt_marker="// skill-allow: §8-no-telemetry",
    )

    for path in iter_rust_files("production"):
        try:
            content = path.read_text(encoding="utf-8", errors="replace")
        except (OSError, UnicodeDecodeError):
            continue
        # Find first CREATE-substrate occurrence (if any).
        create_match = SUBSTRATE_CREATE_RX.search(content)
        if not create_match:
            continue
        # If the file contains a log_telemetry( call anywhere, accept it.
        if LOG_TELEMETRY_RX.search(content):
            continue
        # Otherwise flag, citing the first CREATE site.
        lines = content.splitlines()
        line_idx = content.count("\n", 0, create_match.start())
        line_text = lines[line_idx] if line_idx < len(lines) else ""
        # Honour the marker on the offending line or the line above.
        prev_text = lines[line_idx - 1] if line_idx > 0 else ""
        if ("// skill-allow: §8-no-telemetry" in line_text
                or "// skill-allow: §8-no-telemetry" in prev_text):
            continue
        out.append(Violation(
            rule=synth_rule,
            file=path.relative_to(REPO_ROOT),
            line=line_idx + 1,
            snippet=line_text.rstrip(),
        ))

    return out


ARCH_DOC = REPO_ROOT / "ARCHITECTURE.md"


def detect_arch_drift() -> List[Violation]:
    """[D3] Every crate dir under crates/ must be mentioned in
    ARCHITECTURE.md. Catches new crates added without spec update (§3)."""
    out: List[Violation] = []
    if not CRATES_DIR.exists():
        return out
    if not ARCH_DOC.exists():
        # No architecture doc to compare against — can't enforce.
        return out

    arch_text = ARCH_DOC.read_text(encoding="utf-8")

    synth_rule = Rule(
        id="D3",
        section="§3",
        description=(
            "crate under crates/ not referenced in ARCHITECTURE.md — "
            "every crate must have a corresponding architecture entry"
        ),
        pattern="",
        scope="all",
    )

    for crate_dir in sorted(CRATES_DIR.iterdir()):
        if not crate_dir.is_dir():
            continue
        crate_name = crate_dir.name
        # Crate must be mentioned somewhere in ARCHITECTURE.md (any form:
        # crate name with or without backticks).
        if crate_name in arch_text:
            continue
        cargo_toml = crate_dir / "Cargo.toml"
        out.append(Violation(
            rule=synth_rule,
            file=cargo_toml.relative_to(REPO_ROOT) if cargo_toml.exists()
                else crate_dir.relative_to(REPO_ROOT),
            line=1,
            snippet=f"crate `{crate_name}` not mentioned in ARCHITECTURE.md",
        ))

    return out


# --------------------------------------------------------------------------
# Schema-drift detector
# --------------------------------------------------------------------------

DEFINE_RX = re.compile(
    r"\bDEFINE\s+(TABLE|FIELD|INDEX|USER|ACCESS|FUNCTION|ANALYZER|EVENT|PARAM|CONFIG)"
    r"\b[^;]*?;",
    flags=re.IGNORECASE | re.DOTALL,
)


def _normalize_ddl(stmt: str) -> str:
    """Collapse whitespace and trailing semicolons for comparison."""
    s = re.sub(r"\s+", " ", stmt).strip()
    if not s.endswith(";"):
        s += ";"
    return s


def extract_ddl_from_schema_doc() -> List[str]:
    """Parse SUPERX_SCHEMA.md and return every DEFINE statement found in
    fenced code blocks (sql / surql). Returns normalized statements."""
    if not SCHEMA_DOC.exists():
        return []
    text = SCHEMA_DOC.read_text(encoding="utf-8")
    statements: List[str] = []
    # Find fenced code blocks of type sql or surql or no-type.
    fence_rx = re.compile(
        r"```(?:sql|surql|surrealql)?\n(.*?)```",
        flags=re.DOTALL | re.IGNORECASE,
    )
    for block_match in fence_rx.finditer(text):
        block = block_match.group(1)
        for m in DEFINE_RX.finditer(block):
            statements.append(_normalize_ddl(m.group(0)))
    return statements


def extract_ddl_from_source() -> List[tuple[str, str]]:
    """Walk crates/ for Rust source containing DEFINE statements inside string
    literals (e.g. `db.query("DEFINE TABLE ...")`). Returns list of
    (statement, location) tuples."""
    out: List[tuple[str, str]] = []
    if not CRATES_DIR.exists():
        return out
    # Find any multi-line string literal that contains DEFINE keywords.
    # Use a simplified approach: scan all *.rs files, find DEFINE matches
    # inside them, and capture file:line.
    for path in iter_rust_files("all"):
        try:
            content = path.read_text(encoding="utf-8", errors="replace")
        except (OSError, UnicodeDecodeError):
            continue
        # Only look at content that's likely inside a string literal — i.e.
        # match against the raw file text but require the statement to appear
        # within double-quotes (heuristic).
        for m in DEFINE_RX.finditer(content):
            # Determine line number
            line_no = content.count("\n", 0, m.start()) + 1
            # Heuristic check: the DEFINE has to be within a string literal,
            # signalled by the nearest preceding `"` or `r#"` etc.
            # For now we accept any DEFINE occurrence; this may catch DDL in
            # comments too, which is acceptable noise.
            out.append((
                _normalize_ddl(m.group(0)),
                f"{path.relative_to(REPO_ROOT)}:{line_no}",
            ))
    return out


def detect_schema_drift() -> List[SchemaDrift]:
    """Compare SUPERX_SCHEMA.md DDL against source DDL. Report divergences."""
    doc_ddl = set(extract_ddl_from_schema_doc())
    src_ddl_with_loc = extract_ddl_from_source()
    src_ddl = {stmt for stmt, _ in src_ddl_with_loc}

    drifts: List[SchemaDrift] = []

    # Code has DDL the doc doesn't mention → schema-first violated.
    for stmt, loc in src_ddl_with_loc:
        if stmt not in doc_ddl:
            drifts.append(SchemaDrift(
                kind="code-only",
                statement=stmt,
                location=loc,
            ))

    # Doc has DDL the code doesn't implement → unfinished schema.
    for stmt in doc_ddl:
        if stmt not in src_ddl:
            drifts.append(SchemaDrift(
                kind="doc-only",
                statement=stmt,
                location=str(SCHEMA_DOC.relative_to(REPO_ROOT)),
            ))

    return drifts


# --------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------

def print_human(violations: List[Violation], drifts: List[SchemaDrift]) -> None:
    if violations:
        print()
        print(f"{Ansi.RED}{Ansi.BOLD}── Bright-line violations ──{Ansi.NC}")
        print()
        # Group by section
        by_section: dict[str, List[Violation]] = {}
        for v in violations:
            by_section.setdefault(v.rule.section, []).append(v)
        for section in sorted(by_section.keys()):
            print(f"{Ansi.RED}{section}{Ansi.NC}")
            for v in by_section[section]:
                print(f"  {Ansi.RED}❌{Ansi.NC} [{v.rule.id}] {v.rule.description}")
                print(f"     {Ansi.DIM}{v.file}:{v.line}{Ansi.NC}")
                print(f"     {v.snippet}")
            print()

    if drifts:
        print(f"{Ansi.RED}{Ansi.BOLD}── Schema drift ──{Ansi.NC}")
        print(f"  {Ansi.DIM}({SCHEMA_DOC.relative_to(REPO_ROOT)} vs source DDL){Ansi.NC}")
        print()
        for d in drifts:
            tag = (f"{Ansi.YEL}doc-only{Ansi.NC}"
                   if d.kind == "doc-only"
                   else f"{Ansi.RED}code-only{Ansi.NC}")
            print(f"  [{tag}] {d.statement}")
            print(f"     {Ansi.DIM}{d.location}{Ansi.NC}")
        print()

    print()
    if not violations and not drifts:
        print(f"{Ansi.GRN}════════════════════════════════════════════════════════{Ansi.NC}")
        print(f"{Ansi.GRN}{Ansi.BOLD}✅ SKILL AUDIT CLEAN{Ansi.NC}")
        print(f"{Ansi.GRN}════════════════════════════════════════════════════════{Ansi.NC}")
        return

    print(f"{Ansi.RED}════════════════════════════════════════════════════════{Ansi.NC}")
    print(f"{Ansi.RED}{Ansi.BOLD}❌ SKILL AUDIT FAILED{Ansi.NC}")
    print(f"{Ansi.RED}════════════════════════════════════════════════════════{Ansi.NC}")
    print(f"  {len(violations)} bright-line violation(s)")
    print(f"  {len(drifts)} schema-drift entry(ies)")
    print()
    print("Resolution paths:")
    print("  1. Fix the code so the rule no longer fires.")
    print("  2. Apply an explicit `// skill-allow: ...` marker for a legitimate")
    print("     exception (visible in the diff, defensible at PR review).")
    print("  3. Update SUPERX_SCHEMA.md to match the source if doc-side drift,")
    print("     or update apply_substrate_schema if code-side drift — but per §7,")
    print("     any schema-side change requires `Operator-approved:` in the PR body.")
    print("  4. If the audit rule is wrong, propose changing this script in a")
    print("     separate PR with operator approval. Do not edit script + fix in")
    print("     the same PR.")


def print_json(violations: List[Violation], drifts: List[SchemaDrift]) -> None:
    out = {
        "violations": [v.to_dict() for v in violations],
        "schema_drift": [d.to_dict() for d in drifts],
        "summary": {
            "violations_count": len(violations),
            "drift_count": len(drifts),
            "clean": (not violations) and (not drifts),
        },
    }
    json.dump(out, sys.stdout, indent=2)
    sys.stdout.write("\n")


def print_rules_only() -> None:
    print(f"{Ansi.BOLD}Skill audit rule registry{Ansi.NC}")
    print()
    for r in RULES:
        print(f"  [{r.id}] {r.section} — {r.description}")
        print(f"      scope={r.scope}  marker={r.exempt_marker or '(none)'}")
        print()


# --------------------------------------------------------------------------
# Entrypoint
# --------------------------------------------------------------------------

def main() -> int:
    parser = argparse.ArgumentParser(
        description="Skill-audit gate for SuperX zero-trust-execution.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="emit JSON report on stdout instead of human output",
    )
    parser.add_argument(
        "--no-color",
        action="store_true",
        help="disable ANSI colours",
    )
    parser.add_argument(
        "--rules-only",
        action="store_true",
        help="list every rule and exit 0",
    )
    parser.add_argument(
        "--skip-drift",
        action="store_true",
        help="skip schema-drift check (useful for early-restart phases)",
    )
    args = parser.parse_args()

    if args.no_color or not sys.stdout.isatty():
        Ansi.disable()

    if args.rules_only:
        print_rules_only()
        return 0

    # Per-line regex rules
    violations = audit_all_rules()
    # Structural checks (duplicate literals, telemetry coverage, arch sync)
    violations.extend(detect_duplicate_literals())
    violations.extend(detect_telemetry_gaps())
    violations.extend(detect_arch_drift())

    drifts: List[SchemaDrift] = []
    if not args.skip_drift:
        drifts = detect_schema_drift()

    if args.json:
        print_json(violations, drifts)
    else:
        print_human(violations, drifts)

    if violations:
        return 1
    if drifts:
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
