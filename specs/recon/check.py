"""The spec check: python3 specs/recon/check.py, from the repository root. Exit 0 when every spec file passes.

What it proves, and nothing more:
  - every .xml under specs/recon/ (rules.xml, root.xml, tasks/*.xml) is well-formed;
  - every <quote> whose nearest ancestor carries a `path` or `from` is verbatim in that file, runs of white space read
    as one (for a JSON file, in one of its string values);
  - a quoted sentence in a rule's from is verbatim in the file that from names before it; the operator's own words are
    a ruling, not a file, and are not checked;
  - no text or attribute writes a count (a number of two or more digits outside a date, a version, an issue number,
    a mandate's §, a table's number, a gate's id or a fallback attribute), a credential (a token's shape), a path of the
    operator's own home, or the word reader;
  - every file a spec names in a reference attribute (path, file, from, reflects, builds, where) exists, unless a task
    of the sequence builds or removes it, and none is a prose document (a README, CLAUDE.md, a page, docs/), the
    authority files BLUEPRINT.md, SUPERX_SCHEMA.md and the skill's SKILL.md apart;
  - no spec carries its history: no changelog, no version-by-version header; a task file's header, branch and PR title
    carry its version attribute;
  - every rule has its from, text and breach; every decision is confirmed, with its date and where, or open, with the
    task of the sequence before whose file it closes, and no file of that task or of a later one exists while it is
    open;
  - the root's read step leaves what is no source to authority not_sources; what a PR holds is listed in the root's
    method pr step alone; the root's specs and repository elements name every entry the repository tracks there;
  - every task file has the shape every task file has: start, piece, before, after, reflects, reads, build with
    modules and numbered steps and a tests element whose every prove names a step the file has, agent-decides, gates
    each with id, kind (local or live), proves, cmd and expect, owns, forbid, done; the root's task of the same id
    names the file and the same issue; a task's comparison attribute is yes or absent.
A spec file is committed only when this check passes on it. specs/recon/mutate_check.py plants one defect of each
class above and proves this check names it.
"""
import json
import os
import re
import subprocess
import sys
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
SPECS = os.path.relpath(HERE, REPO)

TASK_SHAPE = ("start", "piece", "before", "after", "reflects", "reads", "build", "agent-decides", "gates", "owns", "forbid", "done")
TASK_ATTRS = ("id", "name", "issue", "version", "date", "reflects", "builds")
GATE_KINDS = ("local", "live")

NUMBER = re.compile(r"(?<![\w.\-/:#§])\d[\d,]*\d(?![\w.\-/:])")   # two or more digits: a 0 or a 1 is a relation, not a count
EXEMPT = (
    re.compile(r"\b\d{4}-\d\d-\d\d\b"),                 # a date
    re.compile(r"\bv?\d+(?:\.\d+)+\b"),                 # a version
    re.compile(r"#\d+"),                                # an issue
    re.compile(r"§\d+"),                                # a mandate of the skill
    re.compile(r"\b[A-Z]\d+\b"),                        # an id: a gate, a phase, a decision of the blueprint
    re.compile(r"\btables? \d+(?:\s*(?:,|–|-|to|and)\s*\d+)*"),   # a table of the structure, by its number
    re.compile(r"\bsteps? \d+(?:\s*(?:,|–|-|to|and)\s*\d+)*"),    # a step of a task file, by its number
    re.compile(r"\b(?:mode|chmod) 0?[0-7]{3}\b"),          # a file's mode
    re.compile(r"\bHTTP [1-5]\d\d\b"),                    # an HTTP status
)
CREDENTIAL = re.compile(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{16,}|\bgithub_pat_[A-Za-z0-9_]{16,}|\bATATT[A-Za-z0-9_\-=]{16,}|\bxox[abprs]-[A-Za-z0-9-]{10,}")
HOME_PATH = re.compile(r"/(?:Users|home)/[A-Za-z0-9._-]+")
READER = re.compile(r"(?<![\w-])readers?\b(?!-)", re.I)
BANS_READER = "crawler-is-the-reader"   # the rule and the decision that ban the word say it
PATH_TOKEN = re.compile(r"(?<![\w.<-])((?:[\w.-]+/)*[\w<>*-]+(?:\.[\w-]+)*\.(?:xml|json|py|rs|surql|toml|md|html|tsx|ts)|(?:[\w.-]+/)+)(?![\w.])")
REF_ATTRS = ("path", "file", "from", "reflects", "builds", "where")
PROSE = re.compile(r"\.html$|(?:^|/)README|CLAUDE\.md$|^docs/", re.I)
AUTHORITY_PROSE = ("BLUEPRINT.md", "SUPERX_SCHEMA.md", ".claude/skills/zero-trust-execution/SKILL.md")
PR_PHRASES = ("PR body", "into the PR", "in the PR")


def norm(s):
    return " ".join(s.split())


def json_strings(obj):
    if isinstance(obj, dict):
        for v in obj.values():
            yield from json_strings(v)
    elif isinstance(obj, list):
        for v in obj:
            yield from json_strings(v)
    elif isinstance(obj, str):
        yield obj


def holds(path, quote):
    """None when the file at path holds the quote, verbatim, runs of white space read as one; else why not."""
    full = os.path.join(REPO, path)
    if not os.path.isfile(full):
        return f"{path} does not exist"
    text = open(full, encoding="utf-8").read()
    q = norm(quote)
    if path.endswith(".json"):
        return None if any(q in norm(s) for s in json_strings(json.loads(text))) else f"{path} holds no string containing: {q[:60]}"
    return None if q in norm(text) else f"{path} does not contain: {q[:60]}"


def scan(text, where, problems, reader=True):
    stripped = text
    for rx in EXEMPT:
        stripped = rx.sub(" ", stripped)
    for m in NUMBER.finditer(stripped):
        problems.append(f"{where}: a count written down: {m.group(0)}")
    for m in CREDENTIAL.finditer(text):
        problems.append(f"{where}: a credential written down: {m.group(0)[:8]}…")
    for m in HOME_PATH.finditer(text):
        problems.append(f"{where}: a path of the operator's own home: {m.group(0)}")
    for m in (READER.finditer(re.sub(r"`[^`]*`", "", text)) if reader else ()):   # a word in backquotes is named, not used
        problems.append(f"{where}: the word {m.group(0)}: the crawler is the reader (rules crawler-is-the-reader)")


def tracked(prefix):
    """The paths the repository tracks under prefix, so a local leftover is not counted."""
    try:
        return subprocess.run(["git", "ls-files", prefix], cwd=REPO, capture_output=True, text=True, check=True).stdout.split()
    except (OSError, subprocess.CalledProcessError):
        return []


def built(root, task_roots):
    """What the tasks build or remove: paths that may be absent, before their task is merged or after it."""
    out = set()
    for t in (list(root.iter("task")) if root is not None else []) + task_roots:
        for p in (t.get("builds") or "").split() + (t.get("removes") or "").split():
            out.add(p)
        if t.get("file"):
            out.add(t.get("file"))
    return out


def exists(token):
    import glob
    pattern = re.sub(r"<[^>]*>", "*", token)
    full = os.path.join(REPO, pattern)
    return bool(glob.glob(full)) or os.path.exists(full)


def references(rel, root, built_paths, problems):
    """Every file a spec names in a reference attribute exists, unless a task builds or removes it, and none is prose."""
    for el in root.iter():
        for attr in REF_ATTRS:
            for token in PATH_TOKEN.findall(el.get(attr) or ""):
                where = f"{rel} <{el.tag} {attr}>"
                if PROSE.search(token) and token not in AUTHORITY_PROSE:
                    problems.append(f"{where}: a prose document as a source: {token}")
                elif not exists(token) and not any(token == b or token.startswith(b) for b in built_paths):
                    problems.append(f"{where}: names {token}, which does not exist")


def rules_from(rel, root, problems):
    """A quoted sentence in a rule's from is verbatim in the file named before it; the operator's words are not checked."""
    for rule in root.iter("rule"):
        text = norm(rule.findtext("from") or "")
        markers = sorted([(m.start(), m.group(1)) for m in PATH_TOKEN.finditer(text) if not m.group(1).endswith("/")]
                         + [(m.start(), None) for m in re.finditer(r"(?i)\bthe operator\b", text)])
        for m in re.finditer(r'"([^"]+)"', text):
            before = [src for pos, src in markers if pos < m.start()]
            source = before[-1] if before else "?"
            if source is None:
                continue
            if source == "?" or holds(source, m.group(1)):
                problems.append(f"{rel} rule {rule.get('id')}: from quotes a sentence {source} does not hold: {m.group(1)[:60]}")


def rules_whole(rel, root, problems):
    for rule in root.iter("rule"):
        for part in ("from", "text", "breach"):
            if not norm(rule.findtext(part) or ""):
                problems.append(f"{rel} rule {rule.get('id')}: no {part}")


def history(rel, raw, root, problems):
    if root.find("changelog") is not None:
        problems.append(f"{rel}: carries a changelog; the history of a spec file is its git log")
    for m in re.finditer(r"<!--(.*?)-->", raw, re.S):
        if re.search(r"\bVersion \d+ \(", m.group(1)):
            problems.append(f"{rel}: its header comment carries a version-by-version history; the history is the git log")


def decisions(root, problems):
    tasks = root.find("sequence").findall("task")
    order = [t.get("id") for t in tasks]
    for d in root.iter("decision"):
        status = d.get("status")
        if status == "confirmed":
            if not d.get("date") or not d.get("where"):
                problems.append(f"{SPECS}/root.xml decision {d.get('id')}: confirmed, with no date or no where")
        elif status == "open":
            task = d.get("closes-before")
            if task not in order:
                problems.append(f"{SPECS}/root.xml decision {d.get('id')}: open, and closes-before names no task of the sequence")
                continue
            for later in tasks[order.index(task):]:
                if later.get("file") and os.path.exists(os.path.join(REPO, later.get("file"))):
                    problems.append(f"{SPECS}/root.xml decision {d.get('id')}: open, and closes before {task}, while {later.get('file')} exists")
                    break
        else:
            problems.append(f"{SPECS}/root.xml decision {d.get('id')}: status is {status}; it is confirmed or open")


def read_step(root, problems):
    read = norm(root.findtext("method/read") or "")
    if "not_sources" not in read:
        problems.append(f"{SPECS}/root.xml <read>: does not leave what is no source to authority not_sources")
    for phrase in ("earlier spec", "other branch", "conversation", "outside the repository", "memory"):
        if phrase in read:
            problems.append(f"{SPECS}/root.xml <read>: repeats not_sources ({phrase}); what is no source is listed in not_sources alone")


def pr_once(root, problems):
    pr = root.find("method/pr")
    for el in root.iter():
        for phrase in PR_PHRASES:
            if el is not pr and phrase in (el.text or ""):
                problems.append(f"{SPECS}/root.xml <{el.tag}>: says what goes {phrase}; what a PR holds is listed in method pr alone")


def coverage(root, problems):
    """The root's specs and repository elements name every entry the repository tracks there."""
    for tag in ("specs", "repository"):
        el = root.find(f"identity/{tag}")
        if el is None or not el.get("path"):
            continue
        base = el.get("path").rstrip("/")
        entries = set()
        for p in tracked(base + "/"):
            head, sep, _ = p[len(base) + 1:].partition("/")
            entries.add(head + sep)
        for e in sorted(entries):
            if e.startswith(".") or e == "__pycache__/":
                continue
            if not re.search(r"(?<![\w./-])" + re.escape(e) + ("" if e.endswith("/") else r"(?![\w.-])"), el.text or ""):
                problems.append(f"{SPECS}/root.xml <{tag}>: does not name {base}/{e}, which the repository holds")


def shape(rel, raw, root, sequence, problems):
    """A task file's shape, its version in its header, branch and title, its gates and its tests, its place in the sequence."""
    for tag in TASK_SHAPE:
        if root.find(tag) is None:
            problems.append(f"{rel}: no <{tag}>")
    for attr in TASK_ATTRS:
        if not root.get(attr):
            problems.append(f"{rel}: no {attr} attribute")
    if root.get("comparison") not in (None, "yes"):
        problems.append(f"{rel}: comparison is {root.get('comparison')}; it is yes or absent")
    v = root.get("version")
    start = root.findtext("start") or ""
    if f"-v{v}-" not in start or f" v{v} · " not in start:
        problems.append(f"{rel}: the start element's branch or PR title does not carry version {v}")
    if f"File version {v} " not in raw:
        problems.append(f"{rel}: the header does not say file version {v}")
    modules = root.findall("build/module")
    if not modules:
        problems.append(f"{rel}: no <module> under <build>")
    steps = {s.get("n") for s in root.iter("step") if s.get("n")}
    for m in modules:
        if not list(m.iter("step")):
            problems.append(f"{rel}: module {m.get('path')} has no <step>")
    tests = root.find("build/tests")
    if tests is None or not tests.findall("prove"):
        problems.append(f"{rel}: no <tests> with <prove> items under <build>")
    else:
        for p in tests.findall("prove"):
            if p.get("step") not in steps | {"platform", "prints"}:
                problems.append(f"{rel}: a prove names step {p.get('step')}, which the file does not have")
    gates = root.findall("gates/gate")
    if not gates:
        problems.append(f"{rel}: no <gate>")
    for g in gates:
        for attr in ("id", "kind", "proves"):
            if not g.get(attr):
                problems.append(f"{rel}: gate {g.get('id') or '?'} has no {attr}")
        if g.get("kind") not in GATE_KINDS:
            problems.append(f"{rel}: gate {g.get('id')} kind is not local or live")
        if g.find("cmd") is None or g.find("expect") is None:
            problems.append(f"{rel}: gate {g.get('id')} has no cmd or no expect")
    task = sequence.get(root.get("id"))
    if task is None:
        problems.append(f"{rel}: the root's sequence has no task {root.get('id')}")
    else:
        if task.get("issue") != root.get("issue"):
            problems.append(f"{rel}: issue {root.get('issue')} but the root says {task.get('issue')}")
        if os.path.basename(task.get("file", "")) != os.path.basename(rel):
            problems.append(f"{rel}: the root's task {root.get('id')} names the file {task.get('file')}")


def main():
    problems = []
    files = sorted(p for d in (HERE, os.path.join(HERE, "tasks")) if os.path.isdir(d)
                   for p in (os.path.join(d, f) for f in os.listdir(d)) if p.endswith(".xml"))
    root_xml, sequence = None, {}
    try:
        root_xml = ET.parse(os.path.join(HERE, "root.xml")).getroot()
        sequence = {t.get("id"): t for t in root_xml.find("sequence").findall("task")}
    except Exception as e:
        problems.append(f"{SPECS}/root.xml: no sequence to check the task files against: {e}")
    task_roots = []
    for f in files:
        try:
            r = ET.parse(f).getroot()
            if r.tag == "task":
                task_roots.append(r)
        except ET.ParseError:
            pass
    built_paths = built(root_xml, task_roots)
    quotes = 0
    for f in files:
        rel = os.path.relpath(f, REPO)
        try:
            root = ET.parse(f).getroot()
        except ET.ParseError as e:
            problems.append(f"{rel}: not well-formed: {e}")
            continue
        raw = open(f, encoding="utf-8").read()
        if root.tag == "task":
            shape(rel, raw, root, sequence, problems)
        if root.tag == "rules":
            rules_from(rel, root, problems)
            rules_whole(rel, root, problems)
        if root.tag == "root":
            decisions(root, problems)
            read_step(root, problems)
            pr_once(root, problems)
            coverage(root, problems)
        references(rel, root, built_paths, problems)
        history(rel, raw, root, problems)
        for m in re.finditer(r"<!--(.*?)-->", raw, re.S):
            scan(m.group(1), f"{rel} <!-- header -->", problems)
        parents = {c: p for p in root.iter() for c in p}

        def bans_reader(el):
            while el is not None:
                if el.get("id") == BANS_READER:
                    return True
                el = parents.get(el)
            return False

        for el in root.iter():
            where = f"{rel} <{el.tag}>"
            attrs = " ".join(v for k, v in el.attrib.items() if k not in ("path", "from", "fallback", "epic", "issue", "version", "date", "n", "step"))
            scan((el.text or "") + " " + attrs, where, problems, reader=not bans_reader(el))
            if el.tag == "quote":
                quotes += 1
                p = el
                while p is not None and not (p.get("path") or p.get("from")):
                    p = parents.get(p)
                if p is None:
                    problems.append(f"{where}: a quote with no file to check against")
                    continue
                err = holds(p.get("path") or p.get("from"), el.text or "")
                if err:
                    problems.append(f"{where}: {err}")
    if problems:
        print("\n".join(problems))
        print(f"check: {len(problems)} problem(s) in {len(files)} file(s)")
        return 1
    print(f"check ok: {len(files)} spec file(s), {quotes} quote(s) verbatim; no count, credential, home path or reader "
          "written down; every reference exists and none is prose; no history; every rule whole; every decision dated "
          "or before its task; the read step, the PR and the coverage of the root; every task file whole")
    return 0


if __name__ == "__main__":
    sys.exit(main())
