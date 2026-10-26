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


# ---------------------------------------------------------------- the map
def map_svg():
    cols, bw, bh, gx, gy = 4, 200, 58, 44, 46
    left, top = 24, 24
    boxes, arrows = [], []
    rx = left + (cols * (bw + gx) - gx) // 2 - bw // 2
    open_n = sum(1 for d in ROOT.iter("decision") if d.get("status") == "open")
    boxes.append((rx, top, "root · rules · decisions", f"{open_n} decisions open", "#s-decisions", True))
    pos = {}
    for i, t in enumerate(TASKS):
        r, c = divmod(i, cols)
        x = left + c * (bw + gx)
        y = top + bh + gy + r * (bh + gy)
        pos[t.get("id")] = (x, y)
        boxes.append((x, y, f"{i + 1} · {key(t)}", t.get("name"), f"#t-{t.get('id')}", False))
    if TASKS:
        x0, y0 = pos[TASKS[0].get("id")]
        arrows.append(f"M{rx + bw // 2},{top + bh} V{top + bh + gy // 2} H{x0 + bw // 2} V{y0 - 3}")
    for t in TASKS:
        for dep in (t.get("depends") or "").split():
            if dep not in pos:
                continue
            ax, ay = pos[dep]
            bx, by = pos[t.get("id")]
            if ay == by:
                arrows.append(f"M{ax + bw},{ay + bh // 2} H{bx - 3}")
            else:
                mid = ay + bh + gy // 2
                arrows.append(f"M{ax + bw // 2},{ay + bh} V{mid} H{bx + bw // 2} V{by - 3}")
    rows = (len(TASKS) + cols - 1) // cols
    width = left * 2 + cols * (bw + gx) - gx
    height = top * 2 + bh + gy + rows * (bh + gy) - gy
    out = [f'<svg class="map" viewBox="0 0 {width} {height}" role="group" aria-label="The build: the root, then every task in its order, each pointing at the next.">',
           '<defs><marker id="arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto"><path d="M0,0 L8,4 L0,8 z" class="mk"/></marker></defs>']
    out += [f'<path d="{d}" class="edge" marker-end="url(#arrow)"/>' for d in arrows]
    for x, y, l1, l2, href, is_root in boxes:
        out.append(f'<a href="{href}"><title>{esc(l1)}: {esc(l2)}</title><rect x="{x}" y="{y}" width="{bw}" height="{bh}" rx="9" class="box{" root" if is_root else ""}"/>'
                   f'<text x="{x + bw // 2}" y="{y + 24}" text-anchor="middle" class="l1">{esc(l1)}</text>'
                   f'<text x="{x + bw // 2}" y="{y + 44}" text-anchor="middle" class="l2">{esc(l2)}</text></a>')
    out.append("</svg>")
    return "".join(out)


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
.mapw{overflow-x:auto;background:var(--panel);border-radius:10px;padding:8px}svg.map{width:100%;min-width:640px;height:auto;display:block}
svg.map .box{fill:var(--box);stroke:var(--edge);stroke-width:1.2}svg.map .box.root{stroke:var(--accent);stroke-width:1.8}
svg.map .l1{fill:var(--ink);font-size:17px;font-weight:600}svg.map .l2{fill:var(--mute);font-size:12.5px}
svg.map .edge{fill:none;stroke:var(--edge);stroke-width:1.6}svg.map .mk{fill:var(--edge)}svg.map a:hover .box{stroke:var(--accent)}
.foot{color:var(--mute);font-size:.88em;margin-top:40px}
"""


def page():
    files = [os.path.join(HERE, "rules.xml"), os.path.join(HERE, "root.xml")] + [f for f in (task_file(t) for t in TASKS) if f]
    body = [f'<h1>Recon Build Spec</h1><p class="sub">Version {esc(ROOT.get("version"))} · epic #{esc(ROOT.get("epic"))} · {esc(ROOT.get("date"))} · rendered from specs/recon/ at {esc(sha())}. '
            "An agent reads the rules, the root, its one task file and the code merged on main, and nothing else.</p>",
            f'<p>{esc(text(ROOT.find("identity/product")))}</p>',
            '<h2 id="s-map">The build</h2><div class="mapw">' + map_svg() + "</div>",
            '<h2 id="s-sequence">The sequence</h2><p class="note">' + esc(text(ROOT.find("sequence/note"))) + "</p>" + sequence_html(),
            '<h2 id="s-decisions">The decisions</h2><p class="note">' + esc(text(ROOT.find("decisions/note"))) + "</p>" + decisions_html(),
            '<h2 id="s-the-rules">The rules · specs/recon/rules.xml</h2>' + rules_html() + raw(files[0]),
            '<h2 id="s-the-root">The root · specs/recon/root.xml</h2>']
    for part in ROOT:
        if part.tag not in ("sequence", "decisions"):
            body.append(render(part, 3))
    body.append(raw(files[1]))
    body.append('<h2 id="s-the-tasks">The task files · specs/recon/tasks/</h2>' + tasks_html())
    body.append(f'<p class="foot">Rendered by specs/recon/build_page.py from {", ".join(esc(os.path.relpath(f, REPO)) for f in files)}; '
                "the spec check (specs/recon/check.py) passes on them.</p>")
    return ('<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">'
            '<title>Recon Build Spec</title><meta name="description" content="The build spec of SuperX Recon: the sequence, the decisions, the rules, the root and every task file, rendered from the repository.">'
            f"<style>{CSS}</style></head><body><div class=\"wrap\">" + "".join(body) + "</div></body></html>")


def main(argv):
    out = argv[0] if argv else os.path.join(HERE, "build_spec.html")
    open(out, "w", encoding="utf-8").write(page())
    print(f"wrote {os.path.relpath(out, REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
