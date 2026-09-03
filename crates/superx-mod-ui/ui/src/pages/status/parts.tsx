import type { ReactNode } from 'react'
import { Badge, Card, Group, SimpleGrid, Text, Title, Tooltip } from '@mantine/core'
import { CANCEL, EChart, FAIL, GRID_LINE, INK, INK_MUTED, MONO, OK, TRACK, UNKNOWN } from '../../EChart'

// The cockpit's parts (#367). Every instrument on the Status page is
// built from these, so a number renders one way, a scope reads one
// way, and a threshold lives in one place.

// ── numbers ────────────────────────────────────────────────────────

/// ts-rs exports i64 as `bigint`, but `api.ts` uses plain `r.json()`,
/// so the values ARRIVE as numbers. `Number()` is what makes `=== 0`
/// comparisons true (#348).
export const n = (v: number | bigint | null | undefined): number => (v == null ? 0 : Number(v))

export function fmtCompact(v: number | bigint | null | undefined): string {
  if (v == null) return '—'
  const x = Number(v)
  if (x >= 1e12) return `${(x / 1e12).toFixed(1)}T`
  if (x >= 1e9) return `${(x / 1e9).toFixed(1)}B`
  if (x >= 1e6) return `${(x / 1e6).toFixed(1)}M`
  if (x >= 1e3) return `${(x / 1e3).toFixed(1)}k`
  return String(x)
}

export function fmtBytes(v: number | bigint | null | undefined): string {
  const x = n(v)
  if (x < 1024) return `${x} B`
  if (x < 1024 * 1024) return `${(x / 1024).toFixed(1)} KB`
  return `${(x / (1024 * 1024)).toFixed(1)} MB`
}

export function fmtAge(secs: number | bigint | null | undefined): string {
  if (secs == null || Number(secs) < 0) return '—'
  const v = Number(secs)
  if (v < 60) return `${v}s`
  if (v < 3600) return `${Math.floor(v / 60)}m`
  if (v < 86400) return `${Math.floor(v / 3600)}h`
  return `${Math.floor(v / 86400)}d`
}

export function fmtMs(ms: number | bigint | null | undefined): string {
  if (ms == null) return '—'
  const v = Number(ms)
  if (v >= 3_600_000) return `${(v / 3_600_000).toFixed(1)}h`
  if (v >= 60_000) return `${Math.round(v / 60_000)}m`
  return `${Math.round(v / 1000)}s`
}

export const pct = (part: number, whole: number): number | null =>
  whole > 0 ? Math.round((part * 100) / whole) : null

export const baseName = (p: string) => p.split('/').slice(-2).join('/')

// ── the bands every gauge and lamp reads from ─────────────────────
// Render-layer thresholds, stated once. The Rust side carries the
// analysis constants (STEERING_MINUTES, DURABLE_MINS, REVISIT_AT);
// these say only where a dial turns amber and red.
export const BANDS = {
  /// replaced ÷ (added + replaced): under this is new code…
  churnOk: 25,
  /// …over this the window spent itself rewriting.
  churnBad: 50,
  /// share of replaced lines a human asked for — below this the
  /// agents are rewriting themselves.
  directedBad: 30,
  directedOk: 60,
  /// test pass rate.
  passOk: 90,
  passBad: 70,
  /// tool calls that came back an error, as a percentage.
  toolFailWarn: 5,
  toolFailBad: 15,
  /// context window in use — the altitude at which compaction looms.
  contextWarn: 70,
  contextBad: 90,
  /// capture lag: amber, then red, in seconds.
  lagWarn: 300,
  lagBad: 1800,
  /// a live row is circling at either of these.
  selfChurnBad: 50,
  revisitedBad: 2,
  /// repos an agent crossed mid-session before it reads as thrash.
  switchesBad: 20,
} as const

export type Tone = 'ok' | 'warn' | 'bad' | 'none'
export const TONE_COLOR: Record<Tone, string | undefined> = {
  ok: OK,
  warn: CANCEL,
  bad: FAIL,
  none: undefined,
}

/// Low is good: churn, failure rates, lag.
export function lowGood(v: number | null, warn: number, bad: number): Tone {
  if (v == null) return 'none'
  return v >= bad ? 'bad' : v >= warn ? 'warn' : 'ok'
}
/// High is good: pass rates, directed share, cache hit.
export function highGood(v: number | null, bad: number, ok: number): Tone {
  if (v == null) return 'none'
  return v < bad ? 'bad' : v < ok ? 'warn' : 'ok'
}

// ── scope: what window a card reads ───────────────────────────────

export type Scope = 'live' | 'range' | '24h' | 'all'

export function rangeLabel(range: string | null, windowMessages?: number | bigint | null): string {
  if (!range) return '…'
  if (range === 'window') return `newest ${fmtCompact(windowMessages)} messages`
  if (range === 'all') return 'all history'
  return `last ${range}`
}

export function ScopeBadge({ scope, range }: { scope: Scope; range: string | null }) {
  const label =
    scope === 'live'
      ? 'live'
      : scope === 'all'
        ? 'all time'
        : scope === '24h'
          ? 'last 24h'
          : range === 'window'
            ? 'window'
            : range === 'all'
              ? 'all history'
              : range ?? '…'
  const tip =
    scope === 'live'
      ? 'a reading of right now — it does not follow the range'
      : scope === 'all'
        ? 'every captured event, regardless of the range'
        : scope === '24h'
          ? 'the last twenty-four hours, regardless of the range'
          : 'follows the range selected above'
  return (
    <Tooltip label={tip} withArrow>
      <Badge
        size="xs"
        variant={scope === 'range' ? 'filled' : 'outline'}
        color={scope === 'live' ? 'teal' : scope === 'range' ? 'pelican' : 'gray'}
        style={{ cursor: 'help' }}
      >
        {label}
      </Badge>
    </Tooltip>
  )
}

// ── layout ─────────────────────────────────────────────────────────

/// One band of the cockpit, with the anchor the sticky nav jumps to.
export function Section({
  id,
  title,
  blurb,
  children,
}: {
  id: string
  title: string
  blurb?: string
  children: ReactNode
}) {
  return (
    <section id={id} style={{ scrollMarginTop: 104, marginBottom: 28 }}>
      <Group gap="sm" align="baseline" mb="sm" wrap="nowrap">
        <Title order={4} style={{ letterSpacing: 0.2 }}>
          {title}
        </Title>
        {blurb && (
          <Text size="xs" c="dimmed" lineClamp={1}>
            {blurb}
          </Text>
        )}
      </Group>
      {children}
    </section>
  )
}

/// A card with a title, its scope, and a one-line note.
export function Panel({
  title,
  scope,
  range,
  note,
  children,
  h,
  mb,
  p,
}: {
  title: string
  scope: Scope
  range: string | null
  note?: string
  children: ReactNode
  h?: string | number
  mb?: string | number
  p?: string | number
}) {
  return (
    <Card withBorder h={h} mb={mb} p={p}>
      <Group justify="space-between" mb="xs" wrap="nowrap" gap="xs">
        <Group gap="xs" wrap="nowrap" style={{ minWidth: 0 }}>
          <Title order={5} style={{ whiteSpace: 'nowrap' }}>
            {title}
          </Title>
          <ScopeBadge scope={scope} range={range} />
        </Group>
        {note && (
          <Text size="xs" c="dimmed" ta="right" lineClamp={1} style={{ minWidth: 0 }}>
            {note}
          </Text>
        )}
      </Group>
      {children}
    </Card>
  )
}

// ── readouts ───────────────────────────────────────────────────────

export function Stat({
  label,
  value,
  sub,
  tip,
  tone,
}: {
  label: string
  value: string
  sub?: string
  tip?: string
  tone?: Tone
}) {
  const card = (
    <Card withBorder padding="sm" h="100%">
      <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
        {label}
      </Text>
      <Title order={3} ff={MONO} c={tone ? TONE_COLOR[tone] : undefined}>
        {value}
      </Title>
      {sub ? (
        <Text size="xs" c="dimmed">
          {sub}
        </Text>
      ) : null}
    </Card>
  )
  return tip ? (
    <Tooltip label={tip} withArrow multiline w={280}>
      {card}
    </Tooltip>
  ) : (
    card
  )
}

/// One small labelled counter inside a card.
export function Counter({
  label,
  value,
  tone,
  tip,
}: {
  label: string
  value: string | number | bigint | null | undefined
  tone?: string
  tip?: string
}) {
  const body = (
    <div>
      <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
        {label}
      </Text>
      <Text fz={22} fw={700} ff={MONO} c={tone}>
        {typeof value === 'string' ? value : fmtCompact(value)}
      </Text>
    </div>
  )
  return tip ? (
    <Tooltip label={tip} withArrow multiline w={260}>
      {body}
    </Tooltip>
  ) : (
    body
  )
}

/// `+added / −removed`, drawn one way everywhere (#348). A dash when
/// both are zero; the size travels with it so a cell cannot out-size
/// its neighbours.
export function Churn({
  added,
  removed,
  size = 'sm',
  fz,
}: {
  added: number | bigint | null | undefined
  removed: number | bigint | null | undefined
  size?: 'xs' | 'sm' | 'md'
  fz?: number
}) {
  const a = n(added)
  const r = n(removed)
  if (a === 0 && r === 0)
    return (
      <Text size={size} c="dimmed" span>
        —
      </Text>
    )
  return (
    <Group gap={6} wrap="nowrap" justify="flex-end" style={{ display: 'inline-flex' }}>
      <Text size={size} fz={fz} fw={fz ? 700 : undefined} ff={MONO} c={OK} span>
        +{fmtCompact(a)}
      </Text>
      <Text size={size} fz={fz} fw={fz ? 700 : undefined} ff={MONO} c={FAIL} span>
        −{fmtCompact(r)}
      </Text>
    </Group>
  )
}

/// A ranked list as proportional bars.
export function BarList({
  rows,
  color = 'var(--mantine-color-pelican-5)',
  mono,
  shorten,
  empty = 'nothing in this range',
}: {
  rows: { name: string; value: number | bigint }[]
  color?: string
  mono?: boolean
  shorten?: (s: string) => string
  empty?: string
}) {
  if (rows.length === 0)
    return (
      <Text size="xs" c="dimmed">
        {empty}
      </Text>
    )
  const max = Math.max(...rows.map((r) => Number(r.value)), 1)
  return (
    <div>
      {rows.map((r) => (
        <Tooltip key={r.name} label={`${r.name} · ${r.value}`} withArrow>
          <Group gap="xs" wrap="nowrap" mb={4}>
            <Text
              size="xs"
              ff={mono ? MONO : undefined}
              style={{ width: 148, flexShrink: 0, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}
            >
              {shorten ? shorten(r.name) : r.name}
            </Text>
            <div style={{ flex: 1, background: TRACK, borderRadius: 3, height: 10 }}>
              <div style={{ width: `${(Number(r.value) * 100) / max}%`, background: color, borderRadius: 3, height: 10 }} />
            </div>
            <Text size="xs" c="dimmed" ff={MONO} style={{ width: 42, textAlign: 'right' }}>
              {fmtCompact(r.value)}
            </Text>
          </Group>
        </Tooltip>
      ))}
    </div>
  )
}

/// A single horizontal meter on the card surface.
export function Meter({ value, color, tip }: { value: number | null; color?: string; tip?: string }) {
  const bar = (
    <div style={{ background: TRACK, borderRadius: 3, height: 10 }}>
      <div style={{ width: `${Math.max(0, Math.min(100, value ?? 0))}%`, background: color ?? UNKNOWN, borderRadius: 3, height: 10 }} />
    </div>
  )
  return tip ? (
    <Tooltip label={tip} withArrow>
      {bar}
    </Tooltip>
  ) : (
    bar
  )
}

// ── instruments ────────────────────────────────────────────────────

/// A dial with banded arcs — the primary flight instruments. `bands`
/// are ECharts axisLine stops: `[[0.25, OK], [0.5, CANCEL], [1, FAIL]]`
/// reads "green to a quarter, amber to a half, red beyond".
export function Gauge({
  label,
  value,
  bands,
  sub,
  tip,
  unit = '%',
  max = 100,
}: {
  label: string
  value: number | null
  bands: [number, string][]
  sub?: string
  tip?: string
  unit?: string
  max?: number
}) {
  const card = (
    <Card withBorder p="xs" h="100%">
      <Text size="xs" c="dimmed" tt="uppercase" ta="center" style={{ letterSpacing: 0.4 }}>
        {label}
      </Text>
      <EChart
        height={128}
        option={{
          series: [
            {
              type: 'gauge',
              startAngle: 210,
              endAngle: -30,
              min: 0,
              max,
              radius: '100%',
              center: ['50%', '62%'],
              axisLine: { lineStyle: { width: 10, color: value == null ? [[1, GRID_LINE]] : bands } },
              pointer:
                value == null
                  ? { show: false }
                  : { length: '58%', width: 4, itemStyle: { color: INK } },
              anchor: { show: value != null, size: 6, itemStyle: { color: INK } },
              axisTick: { show: false },
              splitLine: { show: false },
              axisLabel: { show: false },
              title: { show: false },
              detail: {
                valueAnimation: true,
                fontSize: 22,
                fontWeight: 700,
                fontFamily: MONO,
                color: value == null ? INK_MUTED : INK,
                offsetCenter: [0, '30%'],
                formatter: () => (value == null ? '—' : `${Math.round(value)}${unit}`),
              },
              data: [{ value: value ?? 0 }],
            },
          ],
        }}
      />
      <Text size="xs" c="dimmed" ta="center" lineClamp={1}>
        {sub ?? ' '}
      </Text>
    </Card>
  )
  return tip ? (
    <Tooltip label={tip} withArrow multiline w={280}>
      {card}
    </Tooltip>
  ) : (
    card
  )
}

/// Bands for a dial where low is good.
export const LOW_GOOD: [number, string][] = [
  [BANDS.churnOk / 100, OK],
  [BANDS.churnBad / 100, CANCEL],
  [1, FAIL],
]
/// Bands for a dial where high is good.
export const HIGH_GOOD: [number, string][] = [
  [BANDS.passBad / 100, FAIL],
  [BANDS.passOk / 100, CANCEL],
  [1, OK],
]

/// An annunciator lamp: dark until something is wrong.
export function Lamp({
  label,
  value,
  tone,
  sub,
  tip,
  onClick,
}: {
  label: string
  value: string
  tone: Tone
  sub?: string
  tip?: string
  onClick?: () => void
}) {
  const color = TONE_COLOR[tone]
  const lit = tone === 'warn' || tone === 'bad'
  const card = (
    <Card
      withBorder
      p="xs"
      h="100%"
      onClick={onClick}
      style={{
        cursor: onClick ? 'pointer' : undefined,
        borderColor: lit ? color : undefined,
        background: lit ? `${color}1f` : undefined,
        boxShadow: tone === 'bad' ? `0 0 10px 1px ${color}66` : undefined,
        transition: 'background 200ms, border-color 200ms',
      }}
    >
      <Group gap={6} wrap="nowrap" mb={2}>
        <span
          style={{
            width: 8,
            height: 8,
            borderRadius: '50%',
            flexShrink: 0,
            background: lit ? color : TRACK,
            boxShadow: lit ? `0 0 6px 1px ${color}` : undefined,
          }}
        />
        <Text size="xs" c={lit ? undefined : 'dimmed'} tt="uppercase" lineClamp={1} style={{ letterSpacing: 0.4 }}>
          {label}
        </Text>
      </Group>
      <Text fz={20} fw={700} ff={MONO} c={lit ? color : 'dimmed'} lh={1.1}>
        {value}
      </Text>
      {sub && (
        <Text size="xs" c="dimmed" lineClamp={1}>
          {sub}
        </Text>
      )}
    </Card>
  )
  return tip ? (
    <Tooltip label={tip} withArrow multiline w={280}>
      {card}
    </Tooltip>
  ) : (
    card
  )
}

/// A 24-segment clock: which hours of the last day saw work.
export function CoverageStrip({ hours }: { hours: number | bigint | null | undefined }) {
  const lit = n(hours)
  return (
    <Group gap={2} mt={6} wrap="nowrap">
      {Array.from({ length: 24 }, (_, i) => (
        <div
          key={i}
          style={{ flex: 1, height: 8, borderRadius: 2, background: i < lit ? 'var(--mantine-color-pelican-4)' : TRACK }}
        />
      ))}
    </Group>
  )
}

export function StatGrid({ children, cols }: { children: ReactNode; cols: Record<string, number> }) {
  return (
    <SimpleGrid cols={cols} spacing="xs" mb="md">
      {children}
    </SimpleGrid>
  )
}

export { OK, FAIL, CANCEL, UNKNOWN }
