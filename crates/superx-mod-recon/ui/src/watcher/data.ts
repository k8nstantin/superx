import type { AlertKind, Found, Hidden, Split, TimeRow, WatchAlert, WatchSessionRow } from '../api'
import { CANCEL, FAIL } from '../EChart'
import { span } from '../time'

// What every Watcher view counts the same way: where an agent's time goes,
// in the one order of its colours; the alerts and the findings by name; the
// operator's days; and the focus a click on any chart sets for every view.

/// Where an agent's time goes, in the one order of the colours: validated on
/// the card surface #2A1235 and on white, every adjacent pair clear under
/// colour-blind and normal vision (dataviz validator, 2026-11-05, seven
/// colours with the API's).
export const TIME: [keyof Split, string, string][] = [
  ['thinking', 'thinking', '#B833E8'],
  ['writing', 'writing', '#199e70'],
  ['tools', 'running tools', '#3987e5'],
  ['jobs', 'waiting on its jobs', '#c98500'],
  ['operator', 'waiting on you', '#d55181'],
  ['approval', 'waiting on your approval', '#008300'],
  ['api', 'waiting on the API', '#6f7ef0'],
]

/// The time the agent spent at work, waiting on its own jobs included.
export const atWork = (s: Split) => s.thinking + s.writing + s.tools + s.jobs
/// The time it was held: waiting on its jobs, on you, on your approval, on
/// the API's answers that never came.
export const heldUp = (s: Split) => s.jobs + s.operator + s.approval + s.api
export const every = (s: Split) => atWork(s) + s.operator + s.approval + s.api

export const zero = (): Split => ({ thinking: 0, writing: 0, tools: 0, jobs: 0, operator: 0, approval: 0, api: 0 })
export function plus(a: Split, b: Split): Split {
  return {
    thinking: a.thinking + b.thinking,
    writing: a.writing + b.writing,
    tools: a.tools + b.tools,
    jobs: a.jobs + b.jobs,
    operator: a.operator + b.operator,
    approval: a.approval + b.approval,
    api: a.api + b.api,
  }
}

export const hours = (secs: number) => (secs / 3600).toFixed(1)
export const pct = (part: number, whole: number) => (whole > 0 ? Math.round((100 * part) / whole) : 0)
export const took = (secs: number) => span(secs * 1000)
export const n = (v: number) => v.toLocaleString('en-US')

export const KIND: Record<AlertKind, [string, string]> = {
  run_again: ['Finished work run again', FAIL],
  long_run: ['A run going long', CANCEL],
  stop_not_followed: ['Your stop not followed', FAIL],
  plan_repeats: ['A plan that repeats full runs', CANCEL],
  endless_task: ['A task that never ends', FAIL],
  task_file_broken: ['Its task file broken', FAIL],
  build_stopped: ['A build stopped short', FAIL],
  unproven_merge: ['A task file merged unproven', FAIL],
  api_stalled: ['No answer from the API', FAIL],
  count_gap: ['Its fix covers less than it found', FAIL],
  acted_alone: ['It acted on its own', CANCEL],
  said_false: ['It said what was not so', CANCEL],
  got_wrong: ['It got it wrong', CANCEL],
}

/// What each finding is, in the words the page uses, and what it shows
/// that nothing else does.
export const HIDDEN: Record<Hidden, [string, string]> = {
  held: ['Held on your approval', 'a call waited past the command limit on a prompt of yours'],
  slept: ['Asleep in the foreground', 'a command that slept, often polling its own job, deaf to you'],
  thrash: ['Edited over and over', 'one file edited again and again within the hour'],
  refused: ['Refused', 'a call refused by you, auto mode, a hook, a rule or the worktree, and tried again'],
  protected: ['A protected place changed', 'schema, kernel, infrastructure, skills: changed when nothing you said named it'],
  compacted: ['Context lost', 'the context compacted, and the files read again after it'],
  reread: ['Read again', 'the same part of a file read again with nothing changed in it'],
  flood: ['Flooded', "a result of tens of thousands of characters poured into the agent's context"],
  interrupted: ['You stopped it', 'your interrupt, and the call it was in'],
  asked: ['You asked how it stands', 'a status ask: the agent had not told you'],
  corrected: ['You corrected it', "your correction; its cause is the agent's turn before it"],
  undone: ['Work undone', 'a revert, a hard reset, files put back, a history rewritten'],
}

export const HIDDEN_ORDER: Hidden[] = ['held', 'slept', 'thrash', 'refused', 'protected', 'undone', 'compacted', 'reread', 'flood', 'corrected', 'asked', 'interrupted']

const two = (v: number) => String(v).padStart(2, '0')
/// The operator's own day of a time, as the routes count days: `2026-11-04`.
export function dayOf(iso: string): string {
  const d = new Date(iso)
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}`
}
/// `2026-11-04` as `11-04`.
export const short = (day: string) => day.slice(5)

/// What a click on a chart narrows every view to.
export type Focus = { model?: string; day?: string; session?: string }
export const focusWords = (f: Focus, sessions: Map<string, WatchSessionRow>) =>
  [
    f.model ? `model ${f.model}` : null,
    f.day ? `day ${f.day}` : null,
    f.session ? `session ${sessions.get(f.session)?.name ?? f.session}` : null,
  ].filter(Boolean) as string[]

/// Whether a session, a day and a model are in the focus.
export function within(f: Focus, session: string, day: string | null, model: string | null | undefined): boolean {
  return (!f.session || f.session === session) && (!f.day || f.day === day) && (!f.model || f.model === (model ?? 'unknown'))
}

export const keepRow = (f: Focus) => (r: TimeRow) => within(f, r.session, r.day, r.model)
export const keepFound = (f: Focus, models: Map<string, string | null>) => (x: Found) => within(f, x.session, dayOf(x.at), models.get(x.session))

/// Rows summed by a key.
export function sumBy<T>(rows: T[], key: (r: T) => string, split: (r: T) => Split): Map<string, Split> {
  const out = new Map<string, Split>()
  for (const r of rows) {
    const k = key(r)
    out.set(k, plus(out.get(k) ?? zero(), split(r)))
  }
  return out
}

/// A file's folder, `depth` folders deep from the root.
export function folderOf(path: string, depth: number): string {
  const parts = path.split('/').filter(Boolean)
  return '/' + parts.slice(0, Math.min(depth, Math.max(parts.length - 1, 0))).join('/')
}

/// The tools that read or write a file by its path.
export const FILE_TOOLS = new Set(['Read', 'Edit', 'Write', 'MultiEdit', 'NotebookEdit'])

/// Time no call held, as the tool it is counted under.
export const unheld = (tool: string) => tool.startsWith('(')

/// Everything the watcher knows of: its alerts and its findings.
export type Kind = AlertKind | Hidden

/// A kind's name in the words the page uses.
export const kindName = (k: Kind) => (k in KIND ? KIND[k as AlertKind][0] : HIDDEN[k as Hidden][0])

/// What the agents do, in families the operator's own words name, the
/// worst first (2026-11-06: "redo redo redo", "constantly lies", "took
/// upon itself", "blows away instruction"). The first six are what an agent
/// did wrong, each in its own colour; the rest are what it cost.
export type Family = { key: string; name: string; did: string; about: string; kinds: Kind[]; colour: string | null }

/// The six colours of what an agent did wrong, in the one order of them
/// that passes every check: validated on the card surface #2A1235 and on
/// white, every adjacent pair clear under colour-blind and normal vision
/// (dataviz validator, 2026-11-06, worst ΔE 9.4 deutan).
export const WRONG_COLOURS = ['#B833E8', '#c98500', '#d55181', '#3987e5', '#d95926', '#199e70']

export const FAMILIES: Family[] = [
  { key: 'redone', name: 'Work redone', did: 'redid work', about: 'a merged task reopened, a finished job run again, a plan that runs one job twice, work thrown away', kinds: ['endless_task', 'run_again', 'plan_repeats', 'undone'], colour: WRONG_COLOURS[0] },
  { key: 'false', name: 'Said what was not so', did: 'said what was not so', about: 'a fix that covers less than it found, and its own word that what it said was not so', kinds: ['count_gap', 'said_false'], colour: WRONG_COLOURS[1] },
  { key: 'alone', name: 'Acted on its own', did: 'acted on their own', about: 'its own word that it acted without you, and a protected place changed when nothing you said named it', kinds: ['acted_alone', 'protected'], colour: WRONG_COLOURS[2] },
  { key: 'orders', name: 'Broke your orders', did: 'broke your orders', about: 'your stop not followed, and its task file broken', kinds: ['stop_not_followed', 'task_file_broken'], colour: WRONG_COLOURS[3] },
  { key: 'unproven', name: 'Merged unproven', did: 'had a task file merged unproven', about: 'a task file merged, or asked to be, with no spec gate passed since it changed', kinds: ['unproven_merge'], colour: WRONG_COLOURS[4] },
  { key: 'stopped', name: 'Stopped short', did: 'stopped short', about: 'a build that handed you its turn before it wrote what it builds', kinds: ['build_stopped'], colour: WRONG_COLOURS[5] },
  { key: 'wrong', name: 'Got it wrong', did: 'got it wrong', about: 'its own word that it got something wrong', kinds: ['got_wrong'], colour: null },
  { key: 'you', name: 'You stepped in', did: 'had you step in', about: 'your corrections, your stops, your asks for how it stands', kinds: ['corrected', 'interrupted', 'asked'], colour: null },
  { key: 'held', name: 'Held up', did: 'were held up', about: 'no answer from the API, runs gone long, held on your approval, asleep in the foreground', kinds: ['api_stalled', 'long_run', 'held', 'slept'], colour: null },
  { key: 'waste', name: 'Effort wasted', did: 'wasted effort', about: 'edited over and over, read again, refused and tried again, flooded, context lost', kinds: ['thrash', 'reread', 'refused', 'flood', 'compacted'], colour: null },
]

/// The families of what an agent did wrong.
export const WRONG = FAMILIES.filter((f) => f.colour)
export const familyOf = (k: Kind) => FAMILIES.find((f) => f.kinds.includes(k))

/// One case of anything the watcher knows: an alert or a finding, as one row.
export type Case = { id: string; kind: Kind; name: string; session: string; at: string; from: string; what: string; call: string | null; cost: string; alert: WatchAlert | null; found: Found | null }

/// What a finding cost, in its own measure.
export function costOf(f: Found): string {
  if (f.secs != null) return took(f.secs)
  if (f.chars != null) return `${n(f.chars)} chars`
  if (f.times > 1) return `${f.times} times`
  return '—'
}

/// Every alert and every finding as cases.
export const casesOf = (alerts: WatchAlert[], found: Found[]): Case[] => [
  ...alerts.map((a) => ({ id: a.id, kind: a.kind, name: a.name, session: a.session, at: a.at, from: a.from, what: a.what, call: a.call, cost: a.cost_secs != null ? took(a.cost_secs) : '—', alert: a, found: null })),
  ...found.map((f) => ({ id: f.id, kind: f.kind, name: f.name, session: f.session, at: f.at, from: f.from, what: f.what, call: f.call, cost: costOf(f), alert: null, found: f })),
]

/// The day after a day, both as `2026-11-04`.
function next(day: string): string {
  const d = new Date(`${day}T12:00:00`)
  d.setDate(d.getDate() + 1)
  return dayOf(d.toISOString())
}

/// Every day from `first` to `last`, quiet ones included: a trend reads
/// nothing into a day with no case only if the day is there.
export function daysFrom(first: string, last: string): string[] {
  const out: string[] = []
  for (let d = first; d <= last && out.length < 4000; d = next(d)) out.push(d)
  return out
}

/// The Monday of a day's week, as `2026-11-02`.
export function mondayOf(day: string): string {
  const d = new Date(`${day}T12:00:00`)
  d.setDate(d.getDate() - ((d.getDay() + 6) % 7))
  return dayOf(d.toISOString())
}

/// A week by its first and last day, so a week's bar is never read as its
/// Monday alone: `11-02–08`, `10-26–11-01`; the week still going ends today,
/// `11-02–06 so far`.
export function weekName(monday: string, today: string): string {
  const d = new Date(`${monday}T12:00:00`)
  d.setDate(d.getDate() + 6)
  const sunday = dayOf(d.toISOString())
  const end = sunday > today ? today : sunday
  const tail = end.slice(5, 7) === monday.slice(5, 7) ? end.slice(8) : end.slice(5)
  return `${monday.slice(5)}–${tail}${sunday > today ? ' so far' : ''}`
}
