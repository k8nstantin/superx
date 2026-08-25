import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import {
  Badge,
  Card,
  CloseButton,
  Group,
  ScrollArea,
  Switch,
  Text,
  TextInput,
  Tooltip,
} from '@mantine/core'
import type { SseEvent } from './generated/SseEvent'
import type { SessionView } from './generated/SessionView'
import type { AgentView } from './generated/AgentView'

// THE feed (issue #187): Activity and the session view render the
// same exact continuous feed — same rows, same chips, same pause,
// same pinned-to-bottom auto-scroll. Only the scope differs.

export const MAX_FEED_ROWS = 500

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
  return [...seen.values()]
    .sort((a, b) => (a.valid_from < b.valid_from ? -1 : a.valid_from > b.valid_from ? 1 : 0))
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
        `${match.agent}/${match.session_id.slice(0, 8)}` +
        (match.model ? ` · ${match.model}` : '') +
        (match.effort ? ` · ${match.effort}` : ''),
      key: match.session_id,
      model: match.model ?? undefined,
      effort: match.effort ?? undefined,
    }
  if (e.kind === 'message' && e.session_id)
    return { label: e.session_id.slice(0, 8), key: e.session_id }
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
  onLoadOlder?: (before: string | undefined) => void
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
  const labels = [...new Set(rows.map(rowLabel))].sort()
  const active = label === 'all' || labels.includes(label) ? label : 'all'
  const visible = rows.filter((r) => active === 'all' || rowLabel(r) === active)

  const directory = useMemo(() => buildDirectory(sessions, agents), [sessions, agents])

  // Pinned-to-bottom auto-scroll: a sentinel below the last row is
  // scrolled into view whenever the LAST ROW changes (keying on count
  // dies once the feed hits its cap — the length stops changing);
  // the user scrolling up releases the pin, returning near the bottom
  // restores it.
  const viewport = useRef<HTMLDivElement>(null)
  const bottom = useRef<HTMLDivElement>(null)
  const following = useRef(true)
  const lastId = visible.length > 0 ? visible[visible.length - 1].id : ''
  useEffect(() => {
    if (following.current) {
      bottom.current?.scrollIntoView({ block: 'end' })
    }
  }, [lastId, loading])

  // Scrolling back (issue #241). Reaching the top asks for the page
  // before the oldest row; when those rows land the viewport is
  // rebased so the row you were reading stays under your eyes instead
  // of being shoved down by everything prepended above it.
  const firstId = visible.length > 0 ? visible[0].id : ''
  const anchor = useRef<{ height: number; top: number } | null>(null)
  const askOlder = () => {
    const v = viewport.current
    if (!v || !onLoadOlder || loadingOlder || exhausted) return
    if (v.scrollHeight <= v.clientHeight + 40) return // nothing to scroll yet
    anchor.current = { height: v.scrollHeight, top: v.scrollTop }
    onLoadOlder(visible[0]?.valid_from)
  }
  useLayoutEffect(() => {
    const v = viewport.current
    const a = anchor.current
    if (v && a) {
      v.scrollTop = v.scrollHeight - a.height + a.top
      anchor.current = null
    }
  }, [firstId])

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
        h={580}
        viewportRef={viewport}
        onScrollPositionChange={({ y }) => {
          const v = viewport.current
          if (!v) return
          following.current = y + v.clientHeight >= v.scrollHeight - 80
          if (y < 120) askOlder()
        }}
      >
        {loading && (
          <Text c="dimmed" size="sm">
            loading captured activity…
          </Text>
        )}
        {!loading && onLoadOlder && visible.length > 0 && (
          <Text c="dimmed" size="xs" ta="center" mb={4}>
            {loadingOlder
              ? 'loading older…'
              : exhausted
                ? '— the beginning of what the OS captured —'
                : 'scroll up for more history'}
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
        {visible.map((r) => {
          const identity = identityOf(r, directory)
          return (
            <Text
              key={r.id}
              size="sm"
              ff="monospace"
              mb={2}
              c={r.kind === 'action' ? 'dimmed' : undefined}
            >
              <Badge
                size="xs"
                mr={4}
                variant="filled"
                autoContrast
                color={labelColor(rowLabel(r))}
              >
                {rowLabel(r)}
              </Badge>
              <Tooltip label={identity ? 'session' : 'no session — OS-level event'} withArrow>
                <Badge
                  size="xs"
                  mr={6}
                  variant="filled"
                  autoContrast
                  color={identity ? sessionColor(identity.key) : 'yellow'}
                >
                  {identity?.label ?? 'system'}
                </Badge>
              </Tooltip>
              {r.rendered}
            </Text>
          )
        })}
        <div ref={bottom} />
      </ScrollArea>
    </Card>
  )
}
