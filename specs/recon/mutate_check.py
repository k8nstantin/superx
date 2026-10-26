"""The mutation test of the spec check: python3 specs/recon/mutate_check.py [checkout], from the repository root.

It plants one defect of each class specs/recon/check.py guards, one at a time, runs the check, proves the check fails
naming it, and restores the file before the next; it ends by running the check on the restored files. It rewrites
files in place while it runs, so it runs in a worktree of its own, never in a build folder. Exit 0 when every defect
is caught.
"""
import os
import subprocess
import sys

W = sys.argv[1] if len(sys.argv) > 1 else os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
ROOT, RULES, T01 = "specs/recon/root.xml", "specs/recon/rules.xml", "specs/recon/tasks/536.xml"
PROBE = "specs/recon/tasks/t09-probe.xml"
BARE_TASK = '<?xml version="1.0" encoding="UTF-8"?>\n<task id="t09" name="probe" issue="" version="1" date="2026-10-26">\n  <piece>x</piece>\n</task>\n'

# (label, file, old, new, what the check must say); a case whose old is None creates the file with new as its text.
cases = [
    ("authority quote altered", ROOT, "<quote>Nothing here is judged.</quote>", "<quote>Nothing here is ever judged.</quote>", "does not contain: Nothing here is ever judged."),
    ("rules quote altered", RULES, '"Facts go in columns; judgments do not."', '"Facts go in columns; judgments never do."', "does not hold: Facts go in columns; judgments never do."),
    ("quote with no file", ROOT, "<product>The models", "<product><quote>loose</quote>The models", "a quote with no file to check against"),
    ("count written", ROOT, "Four tasks, four PRs at most", "Four tasks, the 42 PRs at most", "a count written down: 42"),
    ("count in an attribute", ROOT, 'command="superx-recon run" builds="crates/superx-mod-recon/src/crawler/lines/"', 'command="superx-recon run 15" builds="crates/superx-mod-recon/src/crawler/lines/"', "a count written down: 15"),
    ("credential", ROOT, "<not_sources>Earlier", "<not_sources>ghp_abcdefghijklmnopqrstuvwx Earlier", "a credential written down"),
    ("home path", ROOT, "the folder its executables are installed in.", "the folder its executables are installed in, /Users/someone/bin.", "a path of the operator's own home: /Users/someone"),
    ("reader", ROOT, "One crawler crawls every line", "One reader crawls every line", "the word reader"),
    ("stale reference", ROOT, '<file path="SUPERX_SCHEMA.md"', '<file path="SUPERX_SCHEMA_V3.md"', "names SUPERX_SCHEMA_V3.md, which does not exist"),
    ("prose source", ROOT, '<file path="BLUEPRINT.md"', '<file path="README.md"', "a prose document as a source: README.md"),
    ("changelog", ROOT, "  </decisions>\n</root>", '  </decisions>\n  <changelog><entry date="2026-10-26" version="0">x</entry></changelog>\n</root>', "carries a changelog"),
    ("header history", ROOT, "  Parts: identity, authority, method, sequence, decisions.", "  Parts: identity, authority, method, sequence, decisions.\n  Version 1 (2026-10-27): what changed.", "version-by-version history"),
    ("rule without breach", RULES, '    <breach>review</breach>\n  </rule>\n\n  <rule id="specs-are-the-truth">', '  </rule>\n\n  <rule id="specs-are-the-truth">', "rule trunk: no breach"),
    ("decision undated", ROOT, '<decision id="recon" status="confirmed" date="2026-10-26"', '<decision id="recon" status="confirmed"', "decision recon: confirmed, with no date or no where"),
    ("decision status", ROOT, 'status="open" closes-before="t04"', 'status="pending" closes-before="t04"', "status is pending"),
    ("open decision with no task", ROOT, 'status="open" closes-before="t04"', 'status="open" closes-before="t09"', "closes-before names no task of the sequence"),
    ("read repeats not_sources", ROOT, "Nothing else: nothing authority not_sources names.", "Nothing else: no conversation, nothing authority not_sources names.", "repeats not_sources (conversation)"),
    ("PR content outside pr", ROOT, "<deliverable>A task delivers code, tests and the PR with its gate table.", "<deliverable>A task delivers code, tests and the PR with its gate table, and the PR body says so.", "says what goes PR body"),
    ("task file shape", PROBE, None, BARE_TASK, "no <start>"),
    ("task outside the sequence", PROBE, None, BARE_TASK, "the root's sequence has no task t09"),
    ("stale version in start", T01, "feat/536-structure-v1-", "feat/536-structure-v9-", "does not carry version 1"),
    ("stale version in header", T01, "File version 1 (2026-10-26)", "File version 9 (2026-10-26)", "the header does not say file version 1"),
    ("gate of no known kind", T01, '<gate id="G6" kind="live"', '<gate id="G6" kind="remote"', "gate G6 kind is not local or live"),
    ("prove naming a step the file lacks", T01, '<prove step="11">', '<prove step="12">', "a prove names step 12"),
    ("issue unlike the root's", T01, 'issue="536" version="1"', 'issue="537" version="1"', "issue 537 but the root says 536"),
    ("count in a task file", T01, "<piece>The structure task.", "<piece>The structure task, its 42 tests.", "a count written down: 42"),
    ("reader in a task file", T01, "takes a name other than `reader`.", "takes a name other than reader.", "the word reader"),
    ("not well-formed", ROOT, "</root>", "</rooot>", "not well-formed"),
]

failed = 0
for label, rel, old, new, expect in cases:
    path = os.path.join(W, rel)
    original, made_dir = None, False
    if old is None:
        if not os.path.isdir(os.path.dirname(path)):
            os.makedirs(os.path.dirname(path))
            made_dir = True
        open(path, "w", encoding="utf-8").write(new)
    else:
        original = open(path, encoding="utf-8").read()
        assert original.count(old) == 1, (label, original.count(old))
        open(path, "w", encoding="utf-8").write(original.replace(old, new))
    try:
        r = subprocess.run([sys.executable, "specs/recon/check.py"], cwd=W, capture_output=True, text=True)
    finally:
        if original is None:
            os.remove(path)
            if made_dir:
                os.rmdir(os.path.dirname(path))
        else:
            open(path, "w", encoding="utf-8").write(original)
    caught = r.returncode == 1 and expect in r.stdout
    failed += not caught
    print(f"{'CAUGHT' if caught else 'MISSED'}  {label}" + ("" if caught else f"\n   exit {r.returncode}: {r.stdout[-300:]}"))
r = subprocess.run([sys.executable, "specs/recon/check.py"], cwd=W, capture_output=True, text=True)
print("after restore:", r.stdout.strip()[:60], "| exit", r.returncode)
sys.exit(1 if failed or r.returncode else 0)
