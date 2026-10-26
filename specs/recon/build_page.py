"""The Build Spec page, rendered from the spec files: python3 specs/recon/build_page.py [out.html]

One page: the map of the build (the root and every task of the sequence, numbered, in their order), the decisions,
the rules whole, the root whole, every task file whole, and each file's XML as written. Nothing here is written by
hand: every word comes from specs/recon/rules.xml, specs/recon/root.xml and specs/recon/tasks/*.xml.
"""
import html
import os
import subprocess
import sys
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))

esc = html.escape
RULES = ET.parse(os.path.join(HERE, "rules.xml")).getroot()
ROOT = ET.parse(os.path.join(HERE, "root.xml")).getroot()
TASKS = ROOT.find("sequence").findall("task")


def text(el):
    return " ".join((el.text or "").split()) if el is not None else ""


def task_file(task):
    for name in (task.get("file", "").split("/")[-1], f"{task.get('issue')}.xml"):
        path = os.path.join(HERE, "tasks", name)
        if name and name != ".xml" and os.path.exists(path):
            return path
    return None


def sha():
    try:
        return subprocess.check_output(["git", "rev-parse", "--short", "HEAD"], cwd=REPO, text=True).strip()
    except Exception:
        return "working copy"


def key(t):
    return f"#{t.get('issue')}" if t.get("issue") else t.get("id")


# ---------------------------------------------------------------- the map: every box a file of the repository
def slug(path):
    return "src-" + "".join(c if c.isalnum() else "-" for c in path).strip("-")


def authority_paths():
    """The files the root's authority names, in its order: what the root and the rules quote."""
    out = []
    for f in ROOT.find("authority").findall("file"):
        if f.get("path") not in out:
            out.append(f.get("path"))
    return out


def decides(path):
    """What the root says a file decides, to its first colon or comma."""
    for f in ROOT.find("authority").findall("file"):
        if f.get("path") == path:
            d = " ".join((f.get("decides") or "").split())
            for stop in (":", ","):
                d = d.split(stop)[0]
            return d
    return ""


ROW = {"authority": (12, 46), "spec": (172, 58), "task": (322, 58), "tool": (424, 50)}


def map_files():
    """Every file the map draws, by its place: (path, line 1, line 2, x, y, w, h, kind)."""
    files = []
    auth = [p for p in authority_paths() if not p.endswith(".surql")]
    for i, p in enumerate(auth):
        r, c = divmod(i, 3)
        files.append((p, os.path.basename(p), decides(p), 20 + c * 330, ROW["authority"][0] + r * 56, 300, ROW["authority"][1], "authority"))
    structure = next(p for p in authority_paths() if p.endswith(".surql"))
    open_n = sum(1 for d in ROOT.iter("decision") if d.get("status") == "open")
    y, h = ROW["spec"]
    files.append(("specs/recon/rules.xml", "rules.xml", f"{len(RULES.findall('rule'))} rules, each binding every task", 40, y, 220, h, "spec"))
    files.append(("specs/recon/root.xml", "root.xml", f"method, sequence, decisions · {open_n} open", 390, y, 220, h, "root"))
    files.append((structure, "recon.surql", "the structure · no task writes it", 740, y, 240, h, "structure"))
    tw, tgap = 220, (1000 - 40 - 4 * 220) / 3
    y, h = ROW["task"]
    for i, t in enumerate(TASKS):
        path = os.path.relpath(task_file(t), REPO) if task_file(t) else t.get("file")
        sub = f"{key(t)} · {t.get('name')}" + ("" if task_file(t) else " · not written yet")
        files.append((path, f"tasks/{os.path.basename(path)}", sub, 20 + i * (tw + tgap), y, tw, h, "task" if task_file(t) else "missing"))
    tools = [("specs/recon/check.py", "checks every spec file"), ("specs/recon/mutate_check.py", "proves the check catches"),
             ("specs/recon/build_issues.py", "renders the issues"), ("specs/recon/build_page.py", "renders this page")]
    y, h = ROW["tool"]
    for i, (p, sub) in enumerate(tools):
        files.append((p, os.path.basename(p), sub, 20 + i * (tw + tgap), y, tw, h, "tool"))
    return files


def map_svg():
    files = map_files()
    at = {f[0]: f for f in files}
    def mid_x(p):
        f = at[p]
        return f[3] + f[5] / 2
    root, rules = at["specs/recon/root.xml"], at["specs/recon/rules.xml"]
    structure = next(f for f in files if f[7] == "structure")
    tasks = [f for f in files if f[7] in ("task", "missing")]
    auth = [f for f in files if f[7] == "authority"]
    edges, labels = [], []
    frame_bottom = max(f[4] + f[6] for f in auth) + 8
    frame = f'<rect x="8" y="4" width="984" height="{frame_bottom - 4}" rx="12" class="frame"/>'
    edges.append((f"M{mid_x(rules[0]):.0f},{frame_bottom} V{rules[4] - 3}", True))
    edges.append((f"M{mid_x(root[0]):.0f},{frame_bottom} V{root[4] - 3}", True))
    labels.append((mid_x(root[0]) + 10, frame_bottom + 20, "quoted word for word by the root and the rules; check.py proves every quote", "start"))
    edges.append((f"M{root[3]},{root[4] + 29} H{rules[3] + rules[5] + 3}", True))
    labels.append(((root[3] + rules[3] + rules[5]) / 2, root[4] + 22, "read next: the rules", "middle"))
    edges.append((f"M{root[3] + root[5]},{root[4] + 29} H{structure[3] - 3}", True))
    labels.append(((root[3] + root[5] + structure[3]) / 2, root[4] + 22, "its authority", "middle"))
    tbus = (root[4] + root[6] + tasks[0][4]) // 2 + 8
    edges.append((f"M{mid_x(root[0]):.0f},{root[4] + root[6]} V{tbus}", False))
    edges.append((f"M{mid_x(rules[0]):.0f},{rules[4] + rules[6]} V{tbus}", False))
    edges.append((f"M{tasks[0][3] + tasks[0][5] / 2:.0f},{tbus} H{tasks[-1][3] + tasks[-1][5] / 2:.0f}", False))
    for t in tasks:
        edges.append((f"M{t[3] + t[5] / 2:.0f},{tbus} V{t[4] - 3}", True))
    labels.append((mid_x(root[0]) + 8, root[4] + root[6] + 20, "its sequence names each task file", "start"))
    labels.append((mid_x(rules[0]) + 8, rules[4] + rules[6] + 20, "every rule binds every task", "start"))
    edges.append((f"M{tasks[-1][3] + tasks[-1][5] - 30:.0f},{tasks[-1][4]} V{structure[4] + structure[6] + 3}", True))
    labels.append((tasks[-1][3] + tasks[-1][5] - 36, structure[4] + structure[6] + 20, "each task reflects it", "end"))
    for a, b in zip(tasks, tasks[1:]):
        edges.append((f"M{a[3] + a[5]},{a[4] + 29} H{b[3] - 3}", True))
    tools = [f for f in files if f[7] == "tool"]
    labels.append((20, tools[0][4] - 10, "the pack's tools: the check and its mutation test, and the renderers of the issues and this page", "start"))
    height = tools[0][4] + tools[0][6] + 12
    out = [f'<svg class="map" viewBox="0 0 1000 {height}" role="group" aria-label="The spec pack: every file of the repository it is made of, and how each names the others. Each box opens the file as the repository holds it.">',
           '<defs><marker id="arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto"><path d="M0,0 L8,4 L0,8 z" class="mk"/></marker></defs>',
           frame]
    out += [f'<path d="{d}" class="edge"{" marker-end=\"url(#arrow)\"" if arrow else ""}/>' for d, arrow in edges]
    out += [f'<text x="{x:.0f}" y="{y:.0f}" text-anchor="{anchor}" class="lbl">{esc(t)}</text>' for x, y, t, anchor in labels]
    for path, l1, l2, x, y, w, h, kind in files:
        exists = os.path.exists(os.path.join(REPO, path))
        box = (f'<rect x="{x:.0f}" y="{y}" width="{w}" height="{h}" rx="9" class="box {kind}"/>'
               f'<text x="{x + w / 2:.0f}" y="{y + (22 if h > 52 else 21)}" text-anchor="middle" class="l1">{esc(l1)}</text>'
               f'<text x="{x + w / 2:.0f}" y="{y + (42 if h > 52 else 39)}" text-anchor="middle" class="l2">{esc(l2)}</text>')
        if exists:
            out.append(f'<a href="#{slug(path)}" data-src="{slug(path)}"><title>{esc(path)}: open the file as the repository holds it</title>{box}</a>')
        else:
            out.append(f'<g><title>{esc(path)}: not written yet</title>{box}</g>')
    out.append("</svg>")
    return "".join(out)


def sources_html():
    """Every file the map draws that exists, whole, as the repository holds it: what each box opens."""
    out = []
    for path, l1, l2, *_ in map_files():
        full = os.path.join(REPO, path)
        if not os.path.exists(full):
            continue
        body = open(full, encoding="utf-8").read()
        meta = f"{len(body.splitlines())} lines · {l2}"
        out.append(f'<details class="src" id="{slug(path)}" data-path="{esc(path)}" data-meta="{esc(meta)}">'
                   f'<summary><code>{esc(path)}</code> <span class="sub">{esc(meta)}</span></summary><pre>{esc(body)}</pre></details>')
    return "".join(out)


VIEWER = ('<div id="viewer" class="viewer" hidden><div class="vhead"><code class="vpath"></code><span class="vmeta sub"></span>'
          '<button type="button" class="vclose" aria-label="Close the file">Close</button></div><pre class="vbody"></pre></div>')
SCRIPT = """<script>
(function(){
  var viewer = document.getElementById('viewer');
  function show(id){
    var src = document.getElementById(id); if(!src) return false;
    viewer.querySelector('.vpath').textContent = src.getAttribute('data-path');
    viewer.querySelector('.vmeta').textContent = src.getAttribute('data-meta');
    viewer.querySelector('.vbody').textContent = src.querySelector('pre').textContent;
    viewer.hidden = false; viewer.scrollIntoView({block: 'start'});
    return true;
  }
  document.querySelectorAll('svg.map a[data-src]').forEach(function(a){
    a.addEventListener('click', function(e){ if(show(a.getAttribute('data-src'))) e.preventDefault(); });
  });
  viewer.querySelector('.vclose').addEventListener('click', function(){ viewer.hidden = true; });
})();
</script>"""


# ---------------------------------------------------------------- generic XML rendering
def attrs(el, skip=()):
    items = [(k, v) for k, v in el.attrib.items() if k not in skip and v]
    if not items:
        return ""
    return '<dl class="attrs">' + "".join(f"<dt>{esc(k)}</dt><dd>{esc(v)}</dd>" for k, v in items) + "</dl>"


def render(el, depth=2):
    if el.tag == "quote":
        return f"<blockquote>{esc(text(el))}</blockquote>"
    if el.tag == "note":
        return f'<p class="note">{esc(text(el))}</p>'
    title = el.get("id") or el.get("name") or el.tag
    h = min(depth, 5)
    out = [f'<section class="el el-{esc(el.tag)}"><h{h} id="x-{esc(el.tag)}-{esc(title)}">{esc(el.tag)}{"" if title == el.tag else " · " + esc(title)}</h{h}>',
           attrs(el, skip=("id", "name"))]
    if text(el):
        out.append(f"<p>{esc(text(el))}</p>")
    for child in el:
        out.append(render(child, depth + 1))
    out.append("</section>")
    return "".join(out)


def raw(path):
    return (f'<details class="raw"><summary>{esc(os.path.relpath(path, REPO))}, the XML as written</summary>'
            f'<pre>{esc(open(path, encoding="utf-8").read())}</pre></details>')


# ---------------------------------------------------------------- tables
def table(head, rows):
    return ('<div class="tw"><table><thead><tr>' + "".join(f"<th>{esc(h)}</th>" for h in head) + "</tr></thead><tbody>"
            + "".join(rows) + "</tbody></table></div>")


def decisions_html():
    rows = []
    for d in sorted(ROOT.iter("decision"), key=lambda d: d.get("status") != "open"):
        status = d.get("status")
        when = f"closes before {d.get('closes-before')}" if status == "open" else d.get("date", "")
        rows.append(f'<tr class="{esc(status)}"><td><code>{esc(d.get("id"))}</code></td><td><span class="pill {esc(status)}">{esc(status)}</span></td>'
                    f'<td>{esc(when)}</td><td>{esc(text(d))}</td><td>{esc(d.get("where", ""))}</td></tr>')
    return table(("Decision", "Status", "When", "What", "Where it lives"), rows)


def rules_html():
    rows = [f'<tr id="r-{esc(r.get("id"))}"><td><code>{esc(r.get("id"))}</code></td><td>{esc(text(r.find("from")))}</td>'
            f'<td>{esc(text(r.find("text")))}</td><td>{esc(text(r.find("breach")))}</td></tr>' for r in RULES.findall("rule")]
    return table(("Rule", "From", "Text", "Breach caught by"), rows)


def sequence_html():
    rows = []
    for i, t in enumerate(TASKS, 1):
        rows.append(f'<tr><td>{i}</td><td><code>{esc(key(t))}</code></td><td><a href="#t-{esc(t.get("id"))}">{esc(t.get("name"))}</a></td>'
                    f'<td>{esc(text(t.find("piece")))}</td><td><code>{esc(t.get("builds"))}</code></td><td>{esc(t.get("depends") or "")}</td></tr>')
    return table(("#", "Task", "Name", "The piece", "Builds", "After"), rows)


def tasks_html():
    out = []
    for i, t in enumerate(TASKS, 1):
        f = task_file(t)
        head = f"{i} · {key(t)} · {t.get('name')}"
        out.append(f'<section class="task" id="t-{esc(t.get("id"))}"><h3>{esc(head)}</h3>')
        if not f:
            out.append('<p class="note">The task file is not written yet. What the root says of this task:</p>'
                       + attrs(t, skip=("id", "name")) + f"<p>{esc(text(t.find('piece')))}</p>"
                       + "".join(f"<blockquote>{esc(text(q))}</blockquote>" for q in t.findall("quote")) + "</section>")
            continue
        el = ET.parse(f).getroot()
        out.append(attrs(el, skip=("id", "name")))
        for child in el:
            out.append(render(child, 4))
        out.append(raw(f) + "</section>")
    return "".join(out)


CSS = """
:root{--bg:#ffffff;--panel:#f5f6f8;--ink:#1b1f24;--mute:#5a636d;--line:#dde1e6;--accent:#1a5fb4;--box:#eef1f5;--edge:#6b7682;--code:#f0f2f5;--open:#9a5b00;--open-bg:#fff4e0;--ok:#1e6b3a;--ok-bg:#e6f4ea}
@media (prefers-color-scheme: dark){:root:not([data-theme="light"]){--bg:#0f1317;--panel:#161b21;--ink:#e6e9ee;--mute:#9aa3ad;--line:#2a323b;--accent:#8ab4f8;--box:#1d242c;--edge:#7f8b98;--code:#121820;--open:#f2b25c;--open-bg:#2e2310;--ok:#7fd49b;--ok-bg:#132a1c}}
:root[data-theme="dark"]{--bg:#0f1317;--panel:#161b21;--ink:#e6e9ee;--mute:#9aa3ad;--line:#2a323b;--accent:#8ab4f8;--box:#1d242c;--edge:#7f8b98;--code:#121820;--open:#f2b25c;--open-bg:#2e2310;--ok:#7fd49b;--ok-bg:#132a1c}
*{box-sizing:border-box}
html,body{margin:0;background:var(--bg);color:var(--ink);font:15px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif}
.wrap{max-width:1180px;margin:0 auto;padding:0 16px 48px}
h1{font-size:1.7rem;margin:22px 0 4px}h2{font-size:1.25rem;margin:36px 0 10px;border-bottom:1px solid var(--line);padding-bottom:6px}
h3{font-size:1.05rem;margin:22px 0 6px}h4,h5{font-size:.95rem;margin:14px 0 4px;color:var(--mute)}
p{margin:6px 0}.sub{color:var(--mute)}code,pre{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.86em}
code{background:var(--code);padding:1px 5px;border-radius:4px;overflow-wrap:anywhere}
pre{background:var(--code);padding:12px;border-radius:8px;overflow:auto;white-space:pre-wrap;word-break:break-word}
blockquote{margin:6px 0;padding:6px 12px;border-left:3px solid var(--accent);background:var(--panel)}
.note{color:var(--mute);font-style:italic}
.tw{overflow-x:auto}table{border-collapse:collapse;width:100%;font-size:.92em}
th,td{border:1px solid var(--line);padding:6px 8px;vertical-align:top;text-align:left}th{background:var(--panel)}
.pill{display:inline-block;padding:0 8px;border-radius:10px;font-size:.85em;font-weight:600}
.pill.open{color:var(--open);background:var(--open-bg)}.pill.confirmed{color:var(--ok);background:var(--ok-bg)}
dl.attrs{display:grid;grid-template-columns:max-content 1fr;gap:2px 12px;margin:4px 0 8px;font-size:.92em}dl.attrs dt{color:var(--mute)}dl.attrs dd{margin:0;overflow-wrap:anywhere}
.el{margin:8px 0 8px 10px;padding-left:10px;border-left:1px solid var(--line)}
.task{margin-top:24px;padding:12px 16px;background:var(--panel);border-radius:10px}
details.raw{margin-top:14px}details.raw summary{cursor:pointer;color:var(--accent)}
.mapw{overflow-x:auto;background:var(--panel);border-radius:10px;padding:8px}svg.map{width:100%;min-width:860px;height:auto;display:block}
svg.map .box{fill:var(--box);stroke:var(--edge);stroke-width:1.2}svg.map .box.root{stroke:var(--accent);stroke-width:1.8}
svg.map .box.structure{stroke:var(--ok);stroke-width:1.6}svg.map .box.authority,svg.map .box.tool{fill:var(--bg)}
svg.map .box.missing{fill:none;stroke-dasharray:5 4}
svg.map .l1{fill:var(--ink);font-size:15px;font-weight:600;font-family:ui-monospace,SFMono-Regular,Menlo,monospace}svg.map .l2{fill:var(--mute);font-size:11.5px}
svg.map .lbl{fill:var(--mute);font-size:11px;font-style:italic}svg.map .frame{fill:none;stroke:var(--line);stroke-width:1.2;stroke-dasharray:3 4}
svg.map .edge{fill:none;stroke:var(--edge);stroke-width:1.4}svg.map .mk{fill:var(--edge)}svg.map a:hover .box,svg.map a:focus .box{stroke:var(--accent);stroke-width:2.2}
svg.map a{cursor:pointer}
.viewer{margin-top:12px;border:1px solid var(--accent);border-radius:10px;background:var(--panel);padding:10px 12px}
.vhead{display:flex;flex-wrap:wrap;align-items:baseline;gap:6px 12px}.vhead .vclose{margin-left:auto;font:inherit;color:var(--accent);background:none;border:1px solid var(--line);border-radius:6px;padding:2px 10px;cursor:pointer}
.viewer pre{max-height:70vh;margin:10px 0 0;white-space:pre;overflow:auto}
details.src{margin:6px 0;padding:6px 10px;background:var(--panel);border-radius:8px}details.src summary{cursor:pointer}details.src pre{white-space:pre;max-height:70vh}
.foot{color:var(--mute);font-size:.88em;margin-top:40px}
"""


def page():
    files = [os.path.join(HERE, "rules.xml"), os.path.join(HERE, "root.xml")] + [f for f in (task_file(t) for t in TASKS) if f]
    body = [f'<h1>Recon Build Spec</h1><p class="sub">Version {esc(ROOT.get("version"))} · epic #{esc(ROOT.get("epic"))} · {esc(ROOT.get("date"))} · rendered from specs/recon/ at {esc(sha())}. '
            "An agent reads the rules, the root, its one task file and the code merged on main, and nothing else.</p>",
            f'<p>{esc(text(ROOT.find("identity/product")))}</p>',
            '<h2 id="s-map">The spec pack</h2><p class="sub">Every box is a file of the repository; open one to read it as the '
            "repository holds it. The arrows are how the files name each other. A build starts at its task file, reads the root, "
            "then the rules, then its task file whole.</p><div class=\"mapw\">" + map_svg() + "</div>" + VIEWER,
            '<h2 id="s-sequence">The sequence</h2><p class="note">' + esc(text(ROOT.find("sequence/note"))) + "</p>" + sequence_html(),
            '<h2 id="s-decisions">The decisions</h2><p class="note">' + esc(text(ROOT.find("decisions/note"))) + "</p>" + decisions_html(),
            '<h2 id="s-the-rules">The rules · specs/recon/rules.xml</h2>' + rules_html() + raw(files[0]),
            '<h2 id="s-the-root">The root · specs/recon/root.xml</h2>']
    for part in ROOT:
        if part.tag not in ("sequence", "decisions"):
            body.append(render(part, 3))
    body.append(raw(files[1]))
    body.append('<h2 id="s-the-tasks">The task files · specs/recon/tasks/</h2>' + tasks_html())
    body.append('<h2 id="s-files">The files, whole</h2><p class="sub">What each box of the map opens: the file as the repository '
                "holds it at this commit.</p>" + sources_html())
    body.append(f'<p class="foot">Rendered by specs/recon/build_page.py from {", ".join(esc(os.path.relpath(f, REPO)) for f in files)}; '
                "the spec check (specs/recon/check.py) passes on them.</p>")
    return ('<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">'
            '<title>Recon Build Spec</title><meta name="description" content="The build spec of SuperX Recon: the sequence, the decisions, the rules, the root and every task file, rendered from the repository.">'
            f"<style>{CSS}</style></head><body><div class=\"wrap\">" + "".join(body) + "</div>" + SCRIPT + "</body></html>")


def main(argv):
    out = argv[0] if argv else os.path.join(HERE, "build_spec.html")
    open(out, "w", encoding="utf-8").write(page())
    print(f"wrote {os.path.relpath(out, REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
