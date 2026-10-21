import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import {
  Badge,
  Box,
  Card,
  CloseButton,
  Group,
  ScrollArea,
  Switch,
  Text,
  TextInput,
  Tooltip,
} from '@mantine/core'
import { Virtuoso } from 'react-virtuoso'
import type { FeedCursor } from './api'
import type { SseEvent } from './generated/SseEvent'
import type { SessionView } from './generated/SessionView'
import type { AgentView } from './generated/AgentView'

// THE feed (issue #187): Activity and the session view render the
// same exact continuous feed — same rows, same chips, same pause,
// same pinned-to-bottom auto-scroll. Only the scope differs.

export const MAX_FEED_ROWS = 500

// The feed box's height is the reader's to set (#442): dragged from the
// bar under it, remembered per viewer, the same in every feed.
const FEED_HEIGHT_KEY = 'superx.feed.height'
const FEED_HEIGHT_DEFAULT = 580
const FEED_HEIGHT_MIN = 160
const FEED_HEIGHT_STEP = 40

function storedFeedHeight(): number {
  try {
    const v = Number(localStorage.getItem(FEED_HEIGHT_KEY))
    return Number.isFinite(v) && v >= FEED_HEIGHT_MIN ? v : FEED_HEIGHT_DEFAULT
  } catch {
    return FEED_HEIGHT_DEFAULT
  }
}

function storeFeedHeight(h: number) {
  try {
    localStorage.setItem(FEED_HEIGHT_KEY, String(Math.round(h)))
  } catch {
    // A blocked store only loses the remembered height.
  }
}

/// The bar under the feed box: drag it to resize, double-click it to fit
/// the box to the window, or focus it and use the arrow keys.
function ResizeBar({ height, onHeight, box }: { height: number; onHeight: (h: number) => void; box: React.RefObject<HTMLDivElement | null> }) {
  const set = (h: number) => onHeight(Math.max(FEED_HEIGHT_MIN, Math.round(h)))
  const drag = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault()
    const from = e.clientY
    const start = height
    let last = start
    const move = (ev: PointerEvent) => {
      last = Math.max(FEED_HEIGHT_MIN, Math.round(start + ev.clientY - from))
      onHeight(last)
    }
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      storeFeedHeight(last)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }
  const fit = () => {
    const top = box.current?.getBoundingClientRect().top ?? 0
    const h = Math.max(FEED_HEIGHT_MIN, window.innerHeight - Math.max(top, 0) - 48)
    set(h)
    storeFeedHeight(h)
  }
  const key = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const d = e.key === 'ArrowDown' ? FEED_HEIGHT_STEP : e.key === 'ArrowUp' ? -FEED_HEIGHT_STEP : 0
    if (!d) return
    e.preventDefault()
    const h = Math.max(FEED_HEIGHT_MIN, height + d)
    set(h)
    storeFeedHeight(h)
  }
  return (
    <Tooltip label="drag to resize · double-click to fit the window" withArrow openDelay={400}>
      <Box
        role="separator"
        aria-orientation="horizontal"
        aria-label="resize the feed"
        aria-valuenow={height}
        tabIndex={0}
        onPointerDown={drag}
        onDoubleClick={fit}
        onKeyDown={key}
        style={{
          height: 12,
          marginTop: 4,
          cursor: 'ns-resize',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          borderTop: '1px solid var(--mantine-color-dark-4)',
          touchAction: 'none',
        }}
      >
        <Box style={{ width: 48, height: 4, borderRadius: 2, background: 'var(--mantine-color-dark-3)' }} />
      </Box>
    </Tooltip>
  )
}

// Merge backlog + live rows: dedupe by the event's REAL id (every row
// carries its UUIDv7 — no heuristics), backlog copy wins, then sort
// by capture time (SSE emits each tick's actions before its messages,
// so arrival order alone is not chronological), capped to the newest
// rows. Code-point compare: RFC3339 UTC strings order correctly by
// code point; localeCompare does not (ICU collates '.' before '+').
//
// The cap bounds what the LIVE stream may accumulate on its own. Pages
// the reader deliberately fetched raise it (issue #241) — scrolling
// back must not drop the rows you scrolled back to.
export function mergeFeed(backlog: SseEvent[], live: SseEvent[], cap = MAX_FEED_ROWS): SseEvent[] {
  const seen = new Map<string, SseEvent>()
  // Backlog LAST: on a duplicate id the backlog copy wins.
  for (const r of [...live, ...backlog]) seen.set(r.id, r)
  // Rows of one instant order by id, as the server pages them (#273).
  const order = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0)
  return [...seen.values()]
    .sort((a, b) => order(a.valid_from, b.valid_from) || order(a.id, b.id))
    .slice(-cap)
}

// O(1) lookup structures for per-row session attribution. `bySrc` is
// keyed by BOTH the bare source key and the legacy "<agent>/<key>"
// form — session_discovered rows captured before the bare-key fix
// carry the full name in payload.session.
type Directory = {
  bySessionId: Map<string, SessionView>
  bySrc: Map<string, SessionView[]>
  agentName: Map<string, string>
}

function buildDirectory(sessions: SessionView[], agents: AgentView[]): Directory {
  const bySessionId = new Map(sessions.map((s) => [s.session_id, s]))
  const bySrc = new Map<string, SessionView[]>()
  for (const s of sessions) {
    for (const key of [s.src, `${s.agent}/${s.src}`]) {
      const arr = bySrc.get(key)
      if (arr) arr.push(s)
      else bySrc.set(key, [s])
    }
  }
  const agentName = new Map(agents.map((a) => [a.agent_id, a.name]))
  return { bySessionId, bySrc, agentName }
}

// Every session gets its own stable color (issue #200): hash the
// session id into the palette, the same session is the same color
// everywhere. The hues are the validated dark-surface categorical
// ramp (dataviz method, issue #207) — script-validated against this
// dashboard's surface, no fluorescent steps.
const SESSION_COLORS = [
  '#3987e5', // blue
  '#d95926', // orange
  '#199e70', // aqua
  '#c98500', // ochre
  '#d55181', // magenta
  '#008300', // green
  '#CC66FF', // violet — brand accent-bright
  '#e66767', // red
] as const

export function sessionColor(sessionKey: string): string {
  let h = 0
  for (let i = 0; i < sessionKey.length; i++) h = (h * 31 + sessionKey.charCodeAt(i)) >>> 0
  return SESSION_COLORS[h % SESSION_COLORS.length]
}

// Who is doing what: every row is attributed to its session —
// messages by session entity id, actions by source-session key. When
// two agents' sessions share a source key, the event's agent_id
// picks the right one. `key` is the stable identity the color hash
// runs on (the session entity uuid when resolved).
function identityOf(
  e: SseEvent,
  d: Directory,
): { label: string; key: string; model?: string; effort?: string } | null {
  let match: SessionView | undefined
  if (e.kind === 'message') {
    match = e.session_id ? d.bySessionId.get(e.session_id) : undefined
  } else if (e.session_src) {
    const candidates = d.bySrc.get(e.session_src) ?? []
    if (candidates.length > 1 && e.agent_id) {
      const name = d.agentName.get(e.agent_id)
      match = candidates.find((s) => s.agent === name) ?? candidates[0]
    } else {
      match = candidates[0]
    }
  }
  if (match)
    return {
      // The model is part of the session's identity (issue #241): the
      // session outlives the model choice, so it is the CURRENT model,
      // read live rather than stamped at the session's birth. Effort
      // rides along the same way when the agent reports it — the same
      // work at a different effort is different work.
      label:
        `${match.agent}/${match.short_id}` +
        (match.model ? ` · ${match.model}` : '') +
        (match.effort ? ` · ${match.effort}` : ''),
      key: match.session_id,
      model: match.model ?? undefined,
      effort: match.effort ?? undefined,
    }
  if (e.kind === 'message' && e.session_id)
    // Not in the directory yet: the server's floor for a short id
    // (SHORT_ID_MIN), the whole millisecond of the UUIDv7 (#426).
    return { label: e.session_id.slice(0, 13), key: e.session_id }
  if (e.session_src)
    return {
      // Legacy pre-#204 rows carry the literal fallback key; label
      // them honestly instead of a truncated "unknown-".
      label: e.session_src === 'unknown-session' ? 'unattributed' : e.session_src.slice(0, 8),
      key: e.session_src,
    }
  return null // a global OS event — no session
}

// Does a LIVE row belong in a searched feed? The backlog is filtered
// in the engine, over the captured text; a row arriving over SSE is
// already rendered, so it is matched against what the reader sees.
// Same intent, the only check available at that point.
/// The stamp a row leads with, on the viewer's clock (#415 QA). The line
/// comes from the renderer the CLI shares — UTC, `[hh:mm:ss]` for an event
/// and `[yyyy-mm-dd hh:mm:ss]` for a message — while every other time on
/// the page is the viewer's. The same instant, local, in one shape; the
/// date only when it is not today.
export function localStamp(e: SseEvent, now: Date = new Date()): string {
  const m = e.rendered.match(/^\[(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}|\d{2}:\d{2}:\d{2})\]/)
  if (!m) return e.rendered
  // A message's stamp is its whole UTC moment; an event's carries only
  // the time, and its moment is the row's own `valid_from`.
  const at = m[1].length > 8 ? new Date(`${m[1].replace(' ', 'T')}Z`) : new Date(e.valid_from)
  if (Number.isNaN(at.getTime())) return e.rendered
  const time = at.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false })
  const day = at.toDateString() === now.toDateString() ? '' : `${at.toLocaleDateString([], { month: 'short', day: 'numeric' })} `
  return `[${day}${time}]${e.rendered.slice(m[0].length)}`
}

export function matchesSearch(e: SseEvent, q: string): boolean {
  if (!q) return true
  return e.rendered.toLowerCase().includes(q.toLowerCase())
}

// The label a row wears — the same string its badge shows, and the
// vocabulary of the filter chips (operator directive: filter by the
// labels actually visible — user, assistant, tool, action, …).
const rowLabel = (r: SseEvent) => (r.kind === 'message' ? (r.role ?? 'message') : 'action')

// ONE fixed color per label, solid, identical in every view (issues
// #204/#207). The three high-frequency labels wear the all-pairs
// CVD-validated trio of the dark palette; `user` separates on the
// LUMINANCE channel (near-white chip) because a fourth hue from any
// ordering fails the validator's CVD floors. Unknown roles fall back
// to the visible slate.
const LABEL_COLORS: Record<string, string> = {
  user: '#e8e6df', // near-white — unmistakable under every vision type
  assistant: '#3987e5', // validated trio: blue
  tool: '#d95926', //                  orange
  action: '#199e70', //                aqua — visible at last
  system: '#52514e', // slate — neutral but never invisible
}
const labelColor = (label: string) => LABEL_COLORS[label] ?? '#52514e'

/// One row of the feed: its label, its session, and everything the export
/// writes for it (#479). Drawn only while it is near the screen (#461), so
/// it is kept cheap and memoized. Padding, not a margin, between rows: the
/// list measures each row's box, and a margin falls outside it.
const FeedRow = memo(function FeedRow({
  r,
  directory,
  closes,
  unchanged,
}: {
  r: SseEvent
  directory: Directory
  /// The newest row of its request: the request's line goes under it,
  /// once, as the export writes it (#481).
  closes: boolean
  /// A transcript line that says what the last line under its heading
  /// said: named, not written again, as the export names it (#481).
  unchanged: boolean
}) {
  const identity = identityOf(r, directory)
  const stamped = localStamp(r)
  const text = unchanged ? `${stamped.split('\n', 1)[0]} · unchanged` : stamped
  const request = closes && r.request_line ? `\n    [${r.request_line}]` : ''
  return (
    // A div, not the default <p>: a row holds badges, which are
    // divs, and a div inside a <p> is invalid HTML (#415 QA). Its text is
    // written as the export writes it: every line break kept, and a long
    // line wrapped rather than cut or scrolled.
    <Text
      component="div"
      size="sm"
      ff="monospace"
      pb={2}
      c={r.kind === 'action' ? 'dimmed' : undefined}
      style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}
    >
      <Badge size="xs" mr={4} variant="filled" autoContrast color={labelColor(rowLabel(r))}>
        {rowLabel(r)}
      </Badge>
      <Tooltip label={identity ? 'session' : 'no session — OS-level event'} withArrow>
        <Badge size="xs" mr={6} variant="filled" autoContrast color={identity ? sessionColor(identity.key) : 'yellow'}>
          {identity?.label ?? 'system'}
        </Badge>
      </Tooltip>
      {text}
      {request}
    </Text>
  )
})

/// What the line above the rows says, while there is history to read.
type Edge = { show: boolean; loadingOlder?: boolean; exhausted?: boolean }

function HistoryEdge({ context }: { context?: Edge }) {
  if (!context?.show) return null
  return (
    <Text c="dimmed" size="xs" ta="center" pb={4}>
      {context.loadingOlder
        ? 'loading older…'
        : context.exhausted
          ? '— the beginning of what the OS captured —'
          : 'scroll up for more history'}
    </Text>
  )
}

// The list's index of its first row. Rows read back land above it and lower
// it by as many; rows the live cap drops raise it. A large start keeps it
// positive however far back a session reads.
const FIRST_INDEX = 1_000_000_000

export function Feed({
  header,
  rows,
  sessions,
  agents,
  paused,
  onPausedChange,
  loading,
  error,
  onLoadOlder,
  loadingOlder,
  exhausted,
  search,
  onSearchChange,
}: {
  header: React.ReactNode
  rows: SseEvent[]
  sessions: SessionView[]
  agents: AgentView[]
  paused: boolean
  onPausedChange: (v: boolean) => void
  loading?: boolean
  error?: string | null
  /// Fetch the page before the oldest row on screen (issue #241).
  onLoadOlder?: (before: FeedCursor | undefined) => void
  loadingOlder?: boolean
  exhausted?: boolean
  /// Keyword search — matched in the engine across ALL captured
  /// history, not just the rows on screen (issue #241).
  search?: string
  onSearchChange?: (v: string) => void
}) {
  const [label, setLabel] = useState<string>('all')
  // Chips are the DISTINCT labels present in the feed right now. A
  // selected label whose rows aged out falls back to 'all' — never a
  // silently-filtered empty feed.
  const labels = useMemo(() => [...new Set(rows.map(rowLabel))].sort(), [rows])
  const active = label === 'all' || labels.includes(label) ? label : 'all'
  const visible = useMemo(() => rows.filter((r) => active === 'all' || rowLabel(r) === active), [rows, active])

  const directory = useMemo(() => buildDirectory(sessions, agents), [sessions, agents])

  // As the export writes them (#481): a request's line once, under its
  // newest row, and a transcript line that says what the last line under
  // its heading said named as unchanged. Over the rows held, oldest first.
  const { closes, unchanged } = useMemo(() => {
    const newest = new Map<string, string>()
    const last = new Map<string, string>()
    const unchanged = new Set<string>()
    for (const r of visible) {
      if (r.request) newest.set(r.request, r.id)
      if (r.heading && r.said) {
        if (last.get(r.heading) === r.said) unchanged.add(r.id)
        last.set(r.heading, r.said)
      }
    }
    return { closes: new Set(newest.values()), unchanged }
  }, [visible])

  // Only the rows near the screen are drawn (#461): a long session's feed
  // holds tens of thousands, and drawing them all held the page for
  // seconds at every page read back. The scroll box stays the ScrollArea's
  // viewport, so the feed keeps its box, its bar and its height.
  const [scroller, setScroller] = useState<HTMLDivElement | null>(null)
  const box = useRef<HTMLDivElement>(null)
  // Pinned to the bottom until the reader scrolls up; back near the bottom,
  // pinned again. The bottom is the box's own, never the list's idea of its
  // height: that is an estimate until every row has been measured, and a
  // pin kept by it stopped short of the newest row.
  const following = useRef(true)
  const [height, setHeight] = useState(storedFeedHeight)
  // Only the reader lets go of the bottom. A scroll right after their own
  // input (wheel, touch, keys, the bar) that leaves the bottom ends the pin;
  // the list's own scrolls, which hold rows in place by estimated heights
  // and can stop short of the bottom for a frame, never do. Reaching the
  // bottom, by any means, pins again.
  const lastInput = useRef(0)
  useEffect(() => {
    const root = box.current
    if (!root) return
    const mark = () => {
      lastInput.current = performance.now()
    }
    const inputs = ['wheel', 'touchmove', 'keydown', 'pointerdown'] as const
    for (const e of inputs) root.addEventListener(e, mark, { passive: true, capture: true })
    return () => {
      for (const e of inputs) root.removeEventListener(e, mark, { capture: true })
    }
  }, [])
  const onScroll = (y: number) => {
    if (!scroller) return
    if (y + scroller.clientHeight >= scroller.scrollHeight - 80) following.current = true
    // A scroll still carrying the reader's last input, smooth wheels included.
    else if (performance.now() - lastInput.current < 1000) following.current = false
  }
  // A new newest row, with the cap dropping the oldest: the box holds the
  // same height, the list keeps the rows on screen where they were, and
  // the new row lands below the fold. A feed that was pinned when it came
  // follows it down. "Was pinned" is read now, before the list's own
  // scroll to hold its rows in place can say otherwise, and the pin is
  // kept for the frames the list takes to draw and measure the row.
  const lastId = visible.length > 0 ? visible[visible.length - 1].id : ''
  useLayoutEffect(() => {
    if (!scroller || !following.current) return
    let frames = 0
    let frame = 0
    const pin = () => {
      scroller.scrollTop = scroller.scrollHeight
      if ((frames += 1) < 3) frame = requestAnimationFrame(pin)
    }
    frame = requestAnimationFrame(pin)
    return () => cancelAnimationFrame(frame)
  }, [lastId, scroller])

  // Scrolling back (issue #241): reaching the top asks for the page before
  // the oldest row. Rows that land above, asked for or read back on their
  // own (#442), lower the list's first index by as many, so the row you
  // were reading stays under your eyes; rows the live cap drops from the
  // top raise it. A row that lands anywhere else, such as a live row that
  // committed late, is drawn where it belongs and moves nothing.
  //
  // Which list this is comes from what chose it, the label and the search,
  // never from its rows: read from the rows, a late row looked like a new
  // list, the view remounted empty, and it blinked on live updates (#469).
  // A new list starts over at its newest row, pinned.
  const list = `${active}\u0000${search ?? ''}`
  const anchor = useRef({ list, index: FIRST_INDEX, ids: [] as string[] })
  const firstItemIndex = useMemo(() => {
    const before = anchor.current
    const ids = visible.map((r) => r.id)
    let index = FIRST_INDEX
    if (before.list === list) {
      const added = ids.indexOf(before.ids[0])
      const dropped = added < 0 ? before.ids.indexOf(ids[0]) : -1
      if (added >= 0) index = before.index - added
      else if (dropped >= 0) index = before.index + dropped
      else {
        // None of the rows held is left, so the reader's place is gone:
        // the same list, read again, shows its newest row.
        index = before.index
        following.current = true
      }
    } else following.current = true
    anchor.current = { list, index, ids }
    return index
  }, [visible, list])
  // Whatever the box holds changing size puts a pinned feed back at the
  // bottom: the list's own height, the box, and the rows drawn at the
  // bottom, which, measured for the first time, can come out taller or
  // shorter than the list's estimate and move the bottom without the
  // list's own height changing. A new list (`list`) draws new rows.
  useEffect(() => {
    const content = scroller?.firstElementChild
    if (!scroller || !content) return
    const pin = new ResizeObserver(() => {
      if (following.current) scroller.scrollTop = scroller.scrollHeight
    })
    pin.observe(content)
    pin.observe(scroller)
    const drawn = scroller.querySelector('[data-testid="virtuoso-item-list"]')
    if (drawn) pin.observe(drawn)
    return () => pin.disconnect()
  }, [scroller, list])
  const askOlder = () => {
    if (!onLoadOlder || loadingOlder || exhausted || visible.length === 0) return
    onLoadOlder({ at: visible[0].valid_from, id: visible[0].id })
  }

  return (
    <Card withBorder>
      <Group justify="space-between" mb="xs">
        {header}
        <Group gap="xs">
          {onSearchChange && (
            <TextInput
              size="xs"
              w={230}
              placeholder="search all captured history…"
              value={search ?? ''}
              onChange={(e) => onSearchChange(e.currentTarget.value)}
              rightSection={
                search ? (
                  <CloseButton size="xs" onClick={() => onSearchChange('')} aria-label="clear" />
                ) : null
              }
            />
          )}
          {['all', ...labels].map((k) => (
            <Badge
              key={k}
              variant={active === k ? 'filled' : 'outline'}
              style={{ cursor: 'pointer' }}
              onClick={() => setLabel(k)}
            >
              {k}
            </Badge>
          ))}
          <Switch
            label="pause"
            checked={paused}
            onChange={(e) => onPausedChange(e.currentTarget.checked)}
          />
        </Group>
      </Group>
      <ScrollArea
        ref={box}
        h={height}
        viewportRef={setScroller}
        onScrollPositionChange={({ y }) => onScroll(y)}
      >
        {loading && (
          <Text c="dimmed" size="sm">
            loading captured activity…
          </Text>
        )}
        {error && (
          <Text c="red.4" size="sm">
            could not load activity: {error}
          </Text>
        )}
        {!loading && !error && visible.length === 0 && (
          <Text c="dimmed" size="sm">
            {search
              ? `nothing captured matches “${search}”`
              : 'nothing captured yet — work in any coding agent to see it land here'}
          </Text>
        )}
        {scroller && visible.length > 0 && (
          <Virtuoso
            key={list}
            customScrollParent={scroller}
            data={visible}
            firstItemIndex={firstItemIndex}
            initialTopMostItemIndex={{ index: 'LAST', align: 'end' }}
            computeItemKey={(_, r) => r.id}
            startReached={askOlder}
            // Rows are drawn before they reach the box, above it most: a long
            // message scrolled up into is measured off screen and moves
            // nothing the reader sees (drawn only as it came in, the feed
            // jumped by as much as 225 px).
            increaseViewportBy={{ top: 800, bottom: 400 }}
            context={{ show: !loading && !!onLoadOlder, loadingOlder, exhausted }}
            components={{ Header: HistoryEdge }}
            itemContent={(_, r) => (
              <FeedRow r={r} directory={directory} closes={closes.has(r.id)} unchanged={unchanged.has(r.id)} />
            )}
          />
        )}
      </ScrollArea>
      <ResizeBar height={height} onHeight={setHeight} box={box} />
    </Card>
  )
}
