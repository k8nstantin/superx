"""The GitHub issues of the build, rendered from the spec: python3 specs/recon/build_issues.py [task id ...] | --epic

The repository is the source (rules specs-are-the-truth): one issue per task of the root's sequence, its text from the
root and, when the task's file exists, from that file. Nothing flows back from GitHub but the number, which the
operator writes into the task's issue="" and the file's name. The output is JSON, one object per task:
{id, issue, title, body}; the body is Markdown, as gh issue create --body-file takes it.
"""
import json
import os
import sys
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = ET.parse(os.path.join(HERE, "root.xml")).getroot()
EPIC = ROOT.get("epic")
VERSION = ROOT.get("version")


def text(el):
    return " ".join((el.text or "").split()) if el is not None else ""


def task_file(task):
    """The task's own file, when it exists: by the file attribute, else by its issue number."""
    for name in (task.get("file", "").split("/")[-1], f"{task.get('issue')}.xml"):
        path = os.path.join(HERE, "tasks", name)
        if name and name != ".xml" and os.path.exists(path):
            return ET.parse(path).getroot(), os.path.relpath(path, os.path.join(HERE, "..", ".."))
    return None, None


def render(task, order):
    spec, path = task_file(task)
    key = f"#{task.get('issue')}" if task.get("issue") else task.get("id")
    version = spec.get("version") if spec is not None else "0"
    name = task.get("name")
    lines = [f"## {order} · {name}", "",
             f"**Runs as:** `{task.get('command')}`  ",
             f"**Reflects:** `{task.get('reflects')}`  ",
             f"**Builds:** `{task.get('builds')}`  ",
             f"**After:** {task.get('depends') or 'nothing: the first task'}  ",
             f"**Spec:** version {VERSION}; {'`' + path + '`' if path else 'the task file is not written yet'}"
             + ("; a comparison across models" if spec is not None and spec.get("comparison") == "yes" else ""), "",
             text(task.find("piece")), ""]
    issue = task.get("issue") or "<issue>"
    lines += [f"**Kickoff:** point a model at `specs/recon/tasks/{issue}.xml`, nothing else; it writes its own label "
              f"`<model>-<effort>` and reads the root, the rules and the merged code itself. Branch "
              f"`feat/{issue}-{name.replace(' ', '-')}-v{version}-<model>-<effort>` from main; PR into main titled "
              f"`#{issue} {name} v{version} · <model>-<effort>`; every commit starts with `#{issue}`.", ""]
    lines += [f"Under epic #{EPIC}.", ""]
    lines += ["> " + text(q) for q in task.findall("quote")]
    if spec is not None:
        gates = spec.findall(".//gate")
        if gates:
            lines += ["", "### Gates", ""] + [f"- **{g.get('id')}** ({g.get('kind')}): {text(g.find('expect'))}" for g in gates]
    lines += ["", "---", "Generated from the repository by specs/recon/build_issues.py. The task's XML file under "
              "specs/recon/tasks/ is the spec; an agent builds from it, specs/recon/root.xml, specs/recon/rules.xml and "
              "the code merged on main, nothing else. The branch and the PR carry this issue's number."]
    return {"id": task.get("id"), "issue": task.get("issue") or None, "title": f"recon {order} · {name}", "body": "\n".join(lines)}


def epic():
    """The epic's text: the product, the spec's place, the sequence with its issues; nothing else."""
    tasks = ROOT.find("sequence").findall("task")
    lines = [f"## SuperX Recon · build spec version {VERSION}", "", text(ROOT.find("identity/product")), "",
             "The build spec lives in the repository under `specs/recon/`: `rules.xml`, `root.xml` and one XML file per "
             "task under `tasks/`, named by its issue. An agent builds one task from its file, the root, the rules and the "
             "code merged on main, nothing else.", "", "### The sequence", ""]
    lines += [f"{i}. {('#' + t.get('issue')) if t.get('issue') else t.get('id')} · {t.get('name')}" for i, t in enumerate(tasks, 1)]
    lines += ["", "---", "Generated from the repository by specs/recon/build_issues.py (--epic)."]
    return {"issue": EPIC, "body": "\n".join(lines)}


def main(argv):
    if argv == ["--epic"]:
        print(json.dumps(epic(), indent=1, ensure_ascii=False))
        return 0
    tasks = ROOT.find("sequence").findall("task")
    wanted = set(argv) or {t.get("id") for t in tasks}
    print(json.dumps([render(t, i) for i, t in enumerate(tasks, 1) if t.get("id") in wanted], indent=1, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
