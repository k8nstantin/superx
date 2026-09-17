import { Box, SimpleGrid, Table, Text, Title } from '@mantine/core'
import { AXIS, EChart, GRID_LINE, INK, INK_MUTED, TOOLTIP } from '../../EChart'
import { Panel, fmtCompact, n } from './parts'
import type { CompareSummary } from '../../generated/CompareSummary'
import type { Deviation } from '../../generated/Deviation'

/// The same row with every i64 as a plain number. ts-rs maps i64 to
/// bigint, which cannot be added to a number or fed to a chart, so the
/// conversion happens once here rather than at forty call sites.
type Row = { [K in keyof Deviation]: Deviation[K] extends bigint ? number : Deviation[K] }

function toRow(d: Deviation): Row {
  const out = {} as Record<string, unknown>
  for (const [k, val] of Object.entries(d)) {
    out[k] = typeof val === 'bigint' ? Number(val) : val
  }
  return out as Row
}

// Model comparison (#406).
//
// One question: which model should the work go to, and what does
// choosing the other one cost. Everything here is read from git and
// from uncapped per-model runs, in ONE walk producing ONE set of
// numbers — two walks computing "survived %" separately is how two
// sections came to disagree with each other on screen.
//
// The charts are small multiples on purpose. Twenty of them, each one
// measure, each labelled with which direction is good, so the pattern
// is read across the grid rather than argued in prose.

const KEPT = '#199e70'
const THROWN = '#e66767'
const IDENT = ['#B833E8', '#c98500', '#3987e5']
const MIN_ADDED = 5000

/// The model FAMILY. Point releases of one model are the same choice
/// from the operator's side — you reach for Fable or you reach for
/// Opus — and splitting them dilutes the comparison and lets a thin
/// recent point release distort a rate. The full table keeps the exact
/// versions; the charts compare families.
function family(model: string): string {
  const m = model.replace(/^claude-/, '')
  const hit = ['fable', 'opus', 'sonnet', 'haiku'].find((f) => m.startsWith(f))
  return hit ?? m
}

/// Fold versions into families. Every rate is recomputed from the
/// SUMMED numerator and denominator — averaging two rates would weigh
/// a 900-line stint the same as a 40,000-line one.
function byFamily(rows: Row[]): Row[] {
  const acc = new Map<string, Row>()
  for (const r of rows) {
    const k = family(r.model)
    const p = acc.get(k)
    if (!p) {
      acc.set(k, { ...r, model: k })
      continue
    }
    p.commits = n(p.commits) + n(r.commits)
    p.added = n(p.added) + n(r.added)
    p.alive = n(p.alive) + n(r.alive)
    p.removed = n(p.removed) + n(r.removed)
    p.thrown = n(p.thrown) + n(r.thrown)
    p.out_tokens = n(p.out_tokens) + n(r.out_tokens)
    p.messages = n(p.messages) + n(r.messages)
    p.runs = n(p.runs) + n(r.runs)
    p.minutes = n(p.minutes) + n(r.minutes)
    p.minutes_thrown = n(p.minutes_thrown) + n(r.minutes_thrown)
    p.tokens_thrown = n(p.tokens_thrown) + n(r.tokens_thrown)
    p.rework_commits = n(p.rework_commits) + n(r.rework_commits)
    p.thrash_files = n(p.thrash_files) + n(r.thrash_files)
    p.operator_turns = n(p.operator_turns) + n(r.operator_turns)
    p.corrections = n(p.corrections) + n(r.corrections)
    p.context_peak = Math.max(n(p.context_peak), n(r.context_peak))
    // Context is per turn, so it weights by messages.
    p.context_avg =
      n(p.messages) > 0
        ? Math.round(
            (n(p.context_avg) * (n(p.messages) - n(r.messages)) + n(r.context_avg) * n(r.messages)) /
              n(p.messages),
          )
        : n(p.context_avg)
    p.median_age_days = Math.max(n(p.median_age_days), n(r.median_age_days))
  }
  // Recompute every rate from the totals now that they are summed.
  return [...acc.values()].map((p) => ({
    ...p,
    survived_pct: n(p.added) > 0 ? Math.round((100 * n(p.alive)) / n(p.added)) : 0,
    removed_per_100_added: n(p.added) > 0 ? Math.round((100 * n(p.removed)) / n(p.added)) : 0,
    rework_pct: n(p.commits) > 0 ? Math.round((100 * n(p.rework_commits)) / n(p.commits)) : 0,
    thrash_per_100_commits:
      n(p.commits) > 0 ? Math.round((100 * n(p.thrash_files)) / n(p.commits)) : 0,
    corrections_per_100:
      n(p.operator_turns) > 0 ? Math.round((100 * n(p.corrections)) / n(p.operator_turns)) : 0,
    tokens_per_line_landed: n(p.added) > 0 ? Math.round(n(p.out_tokens) / n(p.added)) : 0,
    tokens_per_line_kept: n(p.alive) > 0 ? Math.round(n(p.out_tokens) / n(p.alive)) : 0,
    alive_per_mtok:
      n(p.out_tokens) > 0 ? Math.round((n(p.alive) * 1_000_000) / n(p.out_tokens)) : 0,
    alive_per_hour: n(p.minutes) > 0 ? Math.round((n(p.alive) * 60) / n(p.minutes)) : 0,
  }))
}

type Metric = {
  title: string
  note: string
  pick: (d: Row) => number
  good: 'high' | 'low'
  fmt?: (v: number) => string
}

const per = (a: number, b: number, mult = 1) => (b > 0 ? Math.round((a * mult) / b) : 0)

/// The twenty measures, in reading order: what you GET, what you SPEND
/// per unit, what was WASTED, how the work WENT, and the totals that
/// put the rates in context.
function metrics(): Metric[] {
  return [
    // what you get
    { title: 'Surviving lines per 1M tokens', note: 'the headline', good: 'high', pick: (d) => n(d.alive_per_mtok) },
    { title: 'Surviving lines per hour', note: 'productivity, in time', good: 'high', pick: (d) => n(d.alive_per_hour) },
    { title: 'Landed lines per 1M tokens', note: 'before survival is counted', good: 'high', pick: (d) => per(n(d.added), n(d.out_tokens), 1_000_000) },
    { title: 'Survived', note: 'of what it landed', good: 'high', fmt: (v) => `${v}%`, pick: (d) => n(d.survived_pct) },
    // what you spend per unit
    { title: 'Tokens per surviving line', note: 'the bill actually paid', good: 'low', pick: (d) => n(d.tokens_per_line_kept) },
    { title: 'Tokens per landed line', note: 'the bill before rework', good: 'low', pick: (d) => n(d.tokens_per_line_landed) },
    { title: 'Hours per 1,000 surviving lines', note: 'your time, per unit kept', good: 'low', pick: (d) => per(n(d.minutes), n(d.alive) * 60, 1000) },
    { title: 'Context carried per turn', note: 'paid again every turn', good: 'low', fmt: fmtCompact, pick: (d) => n(d.context_avg) },
    // what was wasted
    { title: 'Tokens thrown away', note: 'spent on lines now gone', good: 'low', fmt: fmtCompact, pick: (d) => n(d.tokens_thrown) },
    { title: 'Hours thrown away', note: 'time spent on lines now gone', good: 'low', pick: (d) => Math.round(n(d.minutes_thrown) / 60) },
    { title: 'Lines thrown away', note: 'landed, then removed', good: 'low', fmt: fmtCompact, pick: (d) => n(d.thrown) },
    { title: 'Lines removed per 100 added', note: 'churn while working', good: 'low', pick: (d) => n(d.removed_per_100_added) },
    // how the work went
    { title: 'Commits that say fix or revert', note: "the agent's own word for it", good: 'low', fmt: (v) => `${v}%`, pick: (d) => n(d.rework_pct) },
    { title: 'Files returned to 3+ times', note: 'per 100 commits', good: 'low', pick: (d) => n(d.thrash_per_100_commits) },
    { title: 'Times you put it back on course', note: 'per 100 of your turns', good: 'low', pick: (d) => n(d.corrections_per_100) },
    { title: 'Largest prompt it ever carried', note: 'peak context', good: 'low', fmt: fmtCompact, pick: (d) => n(d.context_peak) },
    // the totals the rates came from
    { title: 'Output tokens, total', note: 'what was spent', good: 'low', fmt: fmtCompact, pick: (d) => n(d.out_tokens) },
    { title: 'Hours, total', note: 'wall clock across every stint', good: 'low', pick: (d) => Math.round(n(d.minutes) / 60) },
    { title: 'Lines still in the tree', note: 'the actual output', good: 'high', fmt: fmtCompact, pick: (d) => n(d.alive) },
    { title: 'Age of the work', note: 'days — the confounder, check it', good: 'high', fmt: (v) => `${v}d`, pick: (d) => n(d.median_age_days) },
  ]
}

/// One small multiple: a bar per model, sorted so the best is on top
/// once ECharts flips the category axis, and tinted by whether being
/// high here is good or bad.
function mini(rows: Row[], m: Metric) {
  const sorted = [...rows].sort((a, b) => (m.good === 'high' ? m.pick(a) - m.pick(b) : m.pick(b) - m.pick(a)))
  const best = m.good === 'high' ? Math.max(...rows.map(m.pick)) : Math.min(...rows.map(m.pick))
  return {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    grid: { left: 108, right: 62, top: 6, bottom: 6, containLabel: false },
    xAxis: { ...AXIS, type: 'value', axisLabel: { show: false }, splitLine: { show: false }, axisLine: { show: false } },
    yAxis: {
      type: 'category',
      data: sorted.map((r) => r.model.replace('claude-', '')),
      axisLabel: { color: INK_MUTED, fontSize: 10 },
      axisLine: { lineStyle: { color: GRID_LINE } },
      axisTick: { show: false },
    },
    series: [
      {
        type: 'bar',
        barWidth: 11,
        itemStyle: {
          color: (p: { value: number }) => (p.value === best ? KEPT : THROWN),
          borderRadius: [0, 3, 3, 0],
        },
        label: {
          show: true,
          position: 'right',
          color: INK,
          fontSize: 10,
          formatter: (p: { value: number }) => (m.fmt ? m.fmt(p.value) : `${p.value}`),
        },
        data: sorted.map(m.pick),
      },
    ],
  }
}

function stacked(rows: Row[], kept: (d: Row) => number, gone: (d: Row) => number, unit: string, keptName: string, goneName: string) {
  return {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    legend: { data: [keptName, goneName], textStyle: { color: INK_MUTED }, top: 0, right: 0 },
    grid: { left: 150, right: 84, top: 32, bottom: 42 },
    xAxis: {
      ...AXIS,
      type: 'value',
      name: unit,
      nameLocation: 'middle',
      nameGap: 24,
      nameTextStyle: { color: INK_MUTED },
      axisLabel: { color: INK_MUTED, formatter: (x: number) => fmtCompact(x) },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      data: rows.map((r) => r.model),
      axisLabel: { color: INK },
      axisLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [
      {
        name: keptName,
        type: 'bar',
        stack: 's',
        barWidth: 16,
        itemStyle: { color: KEPT, borderColor: 'transparent', borderWidth: 1 },
        label: { show: true, color: '#fff', fontSize: 10, formatter: (p: { value: number }) => (p.value > 0 ? fmtCompact(p.value) : '') },
        data: rows.map(kept),
      },
      {
        name: goneName,
        type: 'bar',
        stack: 's',
        barWidth: 16,
        itemStyle: { color: THROWN, borderColor: 'transparent', borderWidth: 1 },
        label: { show: true, position: 'right', color: THROWN, fontSize: 10, formatter: (p: { value: number }) => (p.value > 0 ? fmtCompact(p.value) : '') },
        data: rows.map(gone),
      },
    ],
  }
}

function verdict(rows: Row[]) {
  const ok = rows.filter((r) => n(r.added) >= MIN_ADDED)
  if (ok.length < 2) return null
  const best = ok.reduce((a, b) => (n(a.alive_per_mtok) >= n(b.alive_per_mtok) ? a : b))
  // Compare the best producer against where the budget ACTUALLY GOES,
  // not against whichever model happens to score lowest. The point of
  // the comparison is the money being spent, and that is the model
  // carrying the largest token bill — pairing the best against a model
  // nobody is spending on makes a true statement about nothing.
  const worst = ok.reduce((a, b) => (n(a.out_tokens) >= n(b.out_tokens) ? a : b))
  if (best.model === worst.model) return null
  const tokX = n(worst.alive_per_mtok) > 0 ? n(best.alive_per_mtok) / n(worst.alive_per_mtok) : 0
  const hrX = n(worst.alive_per_hour) > 0 ? n(best.alive_per_hour) / n(worst.alive_per_hour) : 0
  return { best, worst, tokX, hrX }
}

export function ModelComparison({ c }: { c: CompareSummary | undefined }) {
  if (!c) {
    return (
      <Panel title="Model comparison" scope="all" range={null}>
        <Text size="xs" c="dimmed">reading the repositories — git says what landed, blame says what is left.</Text>
      </Panel>
    )
  }
  const rows = (c.deviations ?? []).map(toRow)
  if (rows.length === 0) {
    return (
      <Panel title="Model comparison" scope="all" range={null}>
        <Text size="xs" c="dimmed">no commit could be credited to a single model yet.</Text>
      </Panel>
    )
  }
  const fams = byFamily(rows)
  const v = verdict(fams)
  const hand = (c.handoffs ?? []).filter((h) => n(h.commits) > 0)
  const ms = metrics()

  const switchChart = {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    grid: { left: 190, right: 90, top: 10, bottom: 42 },
    xAxis: {
      ...AXIS, type: 'value', max: 100,
      name: 'of what it wrote just after taking over, % still there',
      nameLocation: 'middle', nameGap: 24, nameTextStyle: { color: INK_MUTED },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      data: hand.map((h) => `${h.from.replace('claude-', '')} → ${h.to.replace('claude-', '')}`),
      axisLabel: { color: INK, fontSize: 10 },
      axisLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [{
      type: 'bar', barWidth: 16,
      itemStyle: { color: IDENT[0], borderRadius: [0, 4, 4, 0] },
      label: {
        show: true, position: 'right', color: INK, fontSize: 11,
        formatter: (p: { value: number; dataIndex: number }) => `${p.value}%  ·  ${fmtCompact(n(hand[p.dataIndex]?.added))} lines`,
      },
      data: hand.map((h) => n(h.survived_pct)),
    }],
  }

  const repos = (c.repos ?? []).filter((r) => n(r.added) >= 1000).slice(0, 12)
  const repoChart = {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    grid: { left: 210, right: 80, top: 10, bottom: 42 },
    xAxis: {
      ...AXIS, type: 'value', max: 100,
      name: '% of what it landed there that is still in the tree',
      nameLocation: 'middle', nameGap: 24, nameTextStyle: { color: INK_MUTED },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      data: repos.map((r) => `${r.repo} · ${r.model.replace('claude-', '')}`),
      axisLabel: { color: INK, fontSize: 10 },
      axisLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [{
      type: 'bar', barWidth: 12,
      itemStyle: { color: (p: { value: number }) => (p.value >= 50 ? KEPT : THROWN), borderRadius: [0, 3, 3, 0] },
      label: {
        show: true, position: 'right', color: INK, fontSize: 10,
        formatter: (p: { value: number; dataIndex: number }) => `${p.value}%  ·  ${fmtCompact(n(repos[p.dataIndex]?.added))} lines`,
      },
      data: repos.map((r) => n(r.survived_pct)),
    }],
  }

  return (
    <>
      <Panel title="Which model to reach for" scope="all" range={null} note="one walk of git, one set of numbers" mb="md">
        {v && (
          <Box mb="md">
            <Title order={3} style={{ lineHeight: 1.25 }}>
              {v.best.model} produces {v.tokX.toFixed(1)}× more lasting code per token than {v.worst.model}, and {v.hrX.toFixed(1)}× more per hour.
            </Title>
            <Text size="sm" c="dimmed" mt={6}>
              {fmtCompact(n(v.best.alive_per_mtok))} surviving lines per million output tokens against{' '}
              {fmtCompact(n(v.worst.alive_per_mtok))}, and {n(v.best.alive_per_hour)} per hour against{' '}
              {n(v.worst.alive_per_hour)}. That is the whole argument: the cheaper token buys less
              code that lasts, so it is the dearer choice. {v.worst.model} spent{' '}
              {fmtCompact(n(v.worst.out_tokens))} tokens and {Math.round(n(v.worst.minutes) / 60)} hours
              to leave {fmtCompact(n(v.worst.alive))} lines standing; {v.best.model} spent{' '}
              {fmtCompact(n(v.best.out_tokens))} and {Math.round(n(v.best.minutes) / 60)} hours to leave{' '}
              {fmtCompact(n(v.best.alive))}.
            </Text>
          </Box>
        )}
        <SimpleGrid cols={{ base: 1, md: 2, xl: 4 }} spacing="xs">
          {ms.map((m) => (
            <Box key={m.title}>
              <Text fz={11} fw={600} c={INK}>
                {m.title}
              </Text>
              <Text fz={9} c="dimmed" mb={2}>
                {m.note} · {m.good === 'high' ? 'higher is better' : 'lower is better'}
              </Text>
              <EChart option={mini(fams, m)} height={26 + rows.length * 22} />
            </Box>
          ))}
        </SimpleGrid>
        <Text size="xs" c="dimmed" mt="sm">
          Green is the better family on that measure. Point releases are folded together — you reach for Fable or you reach for Opus — and every rate is recomputed from the summed totals rather than averaged.{' '} Twenty measures, one walk of git: rates are shown
          beside the totals they came from, so a thin sample cannot hide behind a ratio.
        </Text>
      </Panel>

      <SimpleGrid cols={{ base: 1, lg: 2 }} mb="md">
        <Panel title="Tokens: what was kept, what was thrown away" scope="all" range={null} note="the money">
          <EChart
            option={stacked(fams, (d) => Math.max(0, n(d.out_tokens) - n(d.tokens_thrown)), (d) => n(d.tokens_thrown), 'output tokens', 'bought code still there', 'thrown away')}
            height={70 + rows.length * 44}
          />
        </Panel>
        <Panel title="Hours: what was kept, what was thrown away" scope="all" range={null} note="the time, which is the one you cannot get back">
          <EChart
            option={stacked(fams, (d) => Math.max(0, Math.round((n(d.minutes) - n(d.minutes_thrown)) / 60)), (d) => Math.round(n(d.minutes_thrown) / 60), 'hours', 'hours that lasted', 'hours thrown away')}
            height={70 + rows.length * 44}
          />
        </Panel>
      </SimpleGrid>

      {repos.length > 0 && (
        <Panel title="Where it happened, repository by repository" scope="all" range={null} note="so one bad checkout is visible rather than averaged away" mb="md">
          <EChart option={repoChart} height={50 + repos.length * 26} />
        </Panel>
      )}

      {hand.length > 0 && (
        <Panel title="What happens right after the model is switched" scope="all" range={null} note={`${hand.length} switch directions that produced commits within three hours`} mb="md">
          <EChart option={switchChart} height={40 + hand.length * 42} />
          <Text size="xs" c="dimmed" mt={4}>
            The account being tested is that a budget runs out, another model takes over, and what it
            does then gets thrown away. That is not what this shows: code written in the first hours
            after a takeover survives ABOVE each model&apos;s own average. The damage is in long
            unbroken runs, which makes run length the thing to control, not who picks the work up.
          </Text>
        </Panel>
      )}

      <Panel title="The comparison, in full" scope="all" range={null} note="by exact version, so a point release can still be inspected">
        <Table striped withTableBorder fz="xs" horizontalSpacing="xs">
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Model</Table.Th>
              <Table.Th ta="right">Tokens</Table.Th>
              <Table.Th ta="right">Hours</Table.Th>
              <Table.Th ta="right">Landed</Table.Th>
              <Table.Th ta="right">Still there</Table.Th>
              <Table.Th ta="right">Survived</Table.Th>
              <Table.Th ta="right">Alive /Mtok</Table.Th>
              <Table.Th ta="right">Alive /hour</Table.Th>
              <Table.Th ta="right">Tok /kept</Table.Th>
              <Table.Th ta="right">Age</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {rows.map((r) => {
              const thin = n(r.added) < MIN_ADDED
              return (
                <Table.Tr key={r.model} opacity={thin ? 0.55 : 1}>
                  <Table.Td>{r.model}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.out_tokens))}</Table.Td>
                  <Table.Td ta="right">{Math.round(n(r.minutes) / 60)}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.added))}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.alive))}</Table.Td>
                  <Table.Td ta="right" c={n(r.survived_pct) >= 50 ? KEPT : THROWN}>{n(r.survived_pct)}%</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.alive_per_mtok))}</Table.Td>
                  <Table.Td ta="right">{n(r.alive_per_hour)}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.tokens_per_line_kept))}</Table.Td>
                  <Table.Td ta="right">{n(r.median_age_days)}d</Table.Td>
                </Table.Tr>
              )
            })}
          </Table.Tbody>
        </Table>
        <Text size="xs" c="dimmed" mt="xs">
          Two measures do NOT separate these models and are in the grid so that can be seen rather
          than assumed: how often you have to correct, and how often a file is returned to. What
          separates them is what the work is worth afterwards, and how much token and clock time it
          took to get there.
        </Text>
      </Panel>
    </>
  )
}
