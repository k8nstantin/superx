import { useCallback, useEffect, useMemo, useState } from 'react'
import { keepPreviousData, useQuery } from '@tanstack/react-query'
import { Alert, Box, Button, Group, Loader, Text } from '@mantine/core'
import { fetchInsights, fetchStats, fetchStatus } from '../../api'
import { useBreadcrumb } from '../../Breadcrumbs'
import { Annunciator } from './Annunciator'
import { CodeSection } from './CodeSection'
import { CostSection } from './CostSection'
import { FleetSection } from './FleetSection'
import { FlightDeck } from './FlightDeck'
import { Gauges } from './Gauges'
import { HistorySection } from './HistorySection'
import { QualitySection } from './QualitySection'
import { Section, rangeLabel } from './parts'
import { SystemsSection } from './SystemsSection'

// The cockpit (#367). One sticky bar carries the sections and the
// range; below it, nine bands in the order a pilot reads them: what is
// wrong, who is in the air, the primary instruments, then the code,
// its quality, the fleet, the cost, the long view and the machine.

const SECTIONS = [
  ['warnings', 'Warnings'],
  ['deck', 'Flight deck'],
  ['gauges', 'Gauges'],
  ['code', 'Code'],
  ['quality', 'Quality'],
  ['fleet', 'Fleet'],
  ['cost', 'Cost'],
  ['history', 'History'],
  ['systems', 'Systems'],
] as const

const RANGES: [string, string][] = [
  ['window', 'newest'],
  ['1h', '1h'],
  ['6h', '6h'],
  ['24h', '24h'],
  ['7d', '7d'],
  ['30d', '30d'],
  ['all', 'all'],
]

/// The range lives in the URL hash so a reload — or a switch to another
/// page and back — lands where you were.
function hashRange(): string | null {
  const m = /range=([a-z0-9]+)/.exec(window.location.hash)
  const r = m?.[1] ?? null
  return r && RANGES.some(([k]) => k === r) ? r : null
}

export default function StatusPage() {
  useBreadcrumb([{ label: 'Status' }])
  const status = useQuery({ queryKey: ['status'], queryFn: fetchStatus, refetchInterval: 10000 })
  // The landing range is the substrate's decision (attr_ui_default_range);
  // the hash overrides it once a pilot has chosen.
  const [chosen, setChosen] = useState<string | null>(hashRange)
  // Back, forward, or a hand-edited hash all land on the range they name.
  useEffect(() => {
    const sync = () => setChosen(hashRange())
    window.addEventListener('hashchange', sync)
    return () => window.removeEventListener('hashchange', sync)
  }, [])
  const range = chosen ?? status.data?.default_range ?? null
  const setRange = useCallback((r: string) => {
    setChosen(r)
    window.history.replaceState(null, '', `#range=${r}`)
  }, [])
  const stats = useQuery({
    queryKey: ['stats', range],
    queryFn: () => fetchStats(range ?? undefined),
    enabled: range != null,
    refetchInterval: 15000,
    // A failed range keeps the last good reading on the instruments
    // and says so, instead of blanking every panel to a dash.
    placeholderData: keepPreviousData,
    // A 400 from the substrate is deterministic, and TanStack pauses
    // retries while the tab is hidden — a retried failure never reached
    // the error state in a background tab. The interval is the retry.
    retry: false,
  })
  const insights = useQuery({ queryKey: ['insights'], queryFn: fetchInsights, refetchInterval: 60000 })

  const s = stats.data
  const i = insights.data
  const stale = s != null && s.range !== range

  // The sticky bar tracks which section is in view.
  const [active, setActive] = useState<string>('warnings')
  useEffect(() => {
    const els = SECTIONS.map(([id]) => document.getElementById(id)).filter((e): e is HTMLElement => e != null)
    if (els.length === 0) return
    const obs = new IntersectionObserver(
      (entries) => {
        const hit = entries.filter((e) => e.isIntersecting).sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)[0]
        if (hit) setActive(hit.target.id)
      },
      { rootMargin: '-120px 0px -60% 0px', threshold: 0 },
    )
    els.forEach((e) => obs.observe(e))
    return () => obs.disconnect()
  }, [])
  const jump = useCallback((id: string) => {
    document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }, [])

  const note = useMemo(() => {
    if (!s) return ''
    const base = rangeLabel(s.range, s.window_messages)
    return s.truncated ? `${base} · sampled, row cap reached` : base
  }, [s])

  return (
    <>
      <Box
        style={{
          position: 'sticky',
          top: 'var(--app-shell-header-offset, 52px)',
          zIndex: 5,
          background: 'var(--mantine-color-body)',
          margin: '-16px -16px 16px',
          padding: '10px 16px',
          borderBottom: '1px solid var(--mantine-color-dark-5)',
        }}
      >
        <Group justify="space-between" wrap="wrap" gap="sm">
          <Group gap={4} wrap="wrap">
            {SECTIONS.map(([id, label]) => (
              <Button key={id} size="compact-xs" variant={active === id ? 'light' : 'subtle'} color={active === id ? 'pelican' : 'gray'} onClick={() => jump(id)}>
                {label}
              </Button>
            ))}
          </Group>
          <Group gap="sm" wrap="nowrap">
            <Text size="xs" c="dimmed" visibleFrom="md" style={{ whiteSpace: 'nowrap' }}>
              {stats.isFetching && range ? `reading ${range}…` : note}
            </Text>
            <Group gap={4} wrap="nowrap">
              {RANGES.map(([key, label]) => (
                <Button key={key} size="compact-xs" variant={range === key ? 'filled' : 'default'} onClick={() => setRange(key)} disabled={range == null}>
                  {label}
                </Button>
              ))}
            </Group>
          </Group>
        </Group>
      </Box>

      {stats.isError && (
        <Alert color="red" variant="light" mb="md" title={`The ${range} range could not be read`}>
          <Text size="sm">{String(stats.error)}</Text>
          <Text size="xs" c="dimmed" mt={4}>
            {stale ? `The instruments below still show the last good reading (${rangeLabel(s?.range ?? null, s?.window_messages)}).` : 'Pick another range, or check the substrate.'}
          </Text>
        </Alert>
      )}
      {insights.isError && (
        <Alert color="orange" variant="light" mb="md" title="The all-history aggregates could not be read">
          <Text size="sm">{String(insights.error)}</Text>
        </Alert>
      )}
      {!s && !stats.isError && (
        <Group gap="xs" mb="md">
          <Loader size="xs" />
          <Text size="sm" c="dimmed">
            {range ? `Reading the ${range} range from the substrate…` : 'Asking the OS which range to land on…'}
          </Text>
        </Group>
      )}

      <Section id="warnings" title="Warnings" blurb="dark is good — a lamp lights when something needs a pilot">
        <Annunciator s={stale ? undefined : s} i={i} status={status.data} jump={jump} />
      </Section>
      <Section id="deck" title="Flight deck" blurb="who is in the air right now, and how each one is flying">
        <FlightDeck s={s} i={i} loading={stats.isFetching && !s} />
      </Section>
      <Section id="gauges" title="Gauges" blurb="the primary instruments — attitude, heading, whether the work holds">
        <Gauges s={stale ? undefined : s} i={i} range={range} />
      </Section>
      <Section id="code" title="Code" blurb="what got built, out of what, and whether it stayed built">
        <CodeSection s={s} range={s?.range ?? range} />
      </Section>
      <Section id="quality" title="Quality" blurb="what the commands reported, when it went wrong, what the agents waited on">
        <QualitySection s={s} range={s?.range ?? range} />
      </Section>
      <Section id="fleet" title="Fleet" blurb="agents, reasoning levels, models and branches compared on outcome, not volume">
        <FleetSection s={s} range={s?.range ?? range} />
      </Section>
      <Section id="cost" title="Cost" blurb="what the tokens bought, and what left this machine">
        <CostSection s={s} i={i} range={s?.range ?? range} />
      </Section>
      <Section id="history" title="History" blurb="the long view — every captured message, regardless of range">
        <HistorySection s={s} i={i} range={s?.range ?? range} />
      </Section>
      <Section id="systems" title="Systems" blurb="the OS itself — modules, capture, the substrate">
        <SystemsSection s={s} i={i} status={status.data} range={s?.range ?? range} />
      </Section>
    </>
  )
}
