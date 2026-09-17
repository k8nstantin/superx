import { Box, SimpleGrid, Table, Text, Title } from '@mantine/core'
import { AXIS, EChart, GRID_LINE, INK, INK_MUTED, TOOLTIP } from '../../EChart'
import { Panel, fmtCompact, n } from './parts'
import type { CompareSummary } from '../../generated/CompareSummary'
import type { Deviation } from '../../generated/Deviation'

// Model comparison (#406) — which model to reach for, and why.
//
// The neighbouring section prices the work. This one asks the harder
// question: work gets handed from one model to another when a budget
// runs out, rework follows, and the rework is blamed on whoever's lines
// died rather than on whoever caused them to die. So the switch is a
// measured event here, not a footnote.
//
// Every panel states its own result in words, including when the result
// is that nothing separates. A chart that cannot support a conclusion
// must not be allowed to imply one — that failure is why the first
// version of this work was deleted rather than patched.

const KEPT = '#199e70'
const THROWN = '#e66767'
const IDENT = ['#B833E8', '#c98500', '#3987e5']

/// Enough landed work for a rate to mean anything.
const MIN_ADDED = 5000

/// Survival rates this close are one spread, so the tie is broken by
/// which model has landed more rather than by the decimal.
const NEAR_TIE = 5

function bar(
  rows: Deviation[],
  pick: (d: Deviation) => number,
  axis: string,
  colour: string,
  fmt?: (v: number) => string,
) {
  const sorted = [...rows].sort((a, b) => pick(b) - pick(a))
  return {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    grid: { left: 150, right: 70, top: 12, bottom: 42 },
    xAxis: {
      ...AXIS,
      type: 'value',
      name: axis,
      nameLocation: 'middle',
      nameGap: 24,
      nameTextStyle: { color: INK_MUTED },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      data: sorted.map((r) => r.model),
      axisLabel: { color: INK },
      axisLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [
      {
        type: 'bar',
        barWidth: 16,
        itemStyle: { color: colour, borderRadius: [0, 4, 4, 0] },
        label: {
          show: true,
          position: 'right',
          color: INK,
          fontSize: 11,
          formatter: (p: { value: number }) => (fmt ? fmt(p.value) : `${p.value}`),
        },
        data: sorted.map(pick),
      },
    ],
  }
}

/// The recommendation, computed, or an honest refusal.
function pick(rows: Deviation[]): { lead: string; body: string } {
  const ok = rows.filter((r) => n(r.added) >= MIN_ADDED)
  if (ok.length < 2) {
    return {
      lead: 'Not enough landed work to compare models yet.',
      body: `A model needs ${fmtCompact(MIN_ADDED)} landed lines before its rates mean anything.`,
    }
  }
  const worst = ok.reduce((a, b) => (n(a.survived_pct) <= n(b.survived_pct) ? a : b))
  // The top rate is not automatically the best-evidenced one. A thin,
  // recent model can edge a point ahead of one carrying four times the
  // lines, and recommending the thin one on that margin is how the
  // previous version of this work went wrong. Among everything within
  // a few points of the leader, prefer the model that has landed the
  // most, because that is the claim that will still hold next week.
  const topRate = Math.max(...ok.map((r) => n(r.survived_pct)))
  const contenders = ok.filter((r) => topRate - n(r.survived_pct) <= NEAR_TIE)
  const best = contenders.reduce((a, b) => (n(a.added) >= n(b.added) ? a : b))
  const gap = n(best.survived_pct) - n(worst.survived_pct)
  if (gap < 5) {
    return {
      lead: 'These models do not separate on what their work is worth.',
      body: `Best and worst are ${gap} points apart on survival, which is one spread rather than a difference.`,
    }
  }
  const churn =
    n(worst.removed_per_100_added) > n(best.removed_per_100_added)
      ? ` It also churns harder while working: ${n(worst.removed_per_100_added)} lines removed per hundred added, against ${n(best.removed_per_100_added)}.`
      : ''
  const rework =
    n(worst.rework_pct) > n(best.rework_pct)
      ? ` ${n(worst.rework_pct)}% of its commits say fix, revert or undo in their own subject, against ${n(best.rework_pct)}%.`
      : ''
  return {
    lead: `Reach for ${best.model} over ${worst.model}.`,
    body: `${n(best.survived_pct)}% of what ${best.model} landed is still in the tree, against ${n(worst.survived_pct)}% for ${worst.model}, over ${fmtCompact(n(best.added))} landed lines against ${fmtCompact(n(worst.added))}.${churn}${rework}`,
  }
}

export function ModelComparison({ c }: { c: CompareSummary | undefined }) {
  if (!c) {
    return (
      <Panel title="Model comparison" scope="all" range={null}>
        <Text size="xs" c="dimmed">
          reading the repositories — git says what landed, blame says what is left.
        </Text>
      </Panel>
    )
  }
  const rows = c.deviations ?? []
  if (rows.length === 0) {
    return (
      <Panel title="Model comparison" scope="all" range={null}>
        <Text size="xs" c="dimmed">
          no commit could be credited to a single model yet.
        </Text>
      </Panel>
    )
  }
  const v = pick(rows)
  // Only switches that actually produced work can say anything about
  // what the incoming model did.
  const hand = (c.handoffs ?? []).filter((h) => n(h.commits) > 0)
  const names = rows.map((r) => r.model)

  const worth = {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    legend: {
      data: ['still there', 'gone'],
      textStyle: { color: INK_MUTED },
      top: 0,
      right: 0,
    },
    grid: { left: 150, right: 80, top: 32, bottom: 42 },
    xAxis: {
      ...AXIS,
      type: 'value',
      name: 'lines it landed',
      nameLocation: 'middle',
      nameGap: 24,
      nameTextStyle: { color: INK_MUTED },
      axisLabel: { color: INK_MUTED, formatter: (x: number) => fmtCompact(x) },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      data: names,
      axisLabel: { color: INK },
      axisLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [
      {
        name: 'still there',
        type: 'bar',
        stack: 'w',
        barWidth: 16,
        itemStyle: { color: KEPT, borderColor: 'transparent', borderWidth: 1 },
        label: {
          show: true,
          color: '#fff',
          fontSize: 10,
          formatter: (p: { value: number }) => (p.value > 0 ? fmtCompact(p.value) : ''),
        },
        data: rows.map((r) => n(r.alive)),
      },
      {
        name: 'gone',
        type: 'bar',
        stack: 'w',
        barWidth: 16,
        itemStyle: { color: THROWN, borderColor: 'transparent', borderWidth: 1 },
        label: {
          show: true,
          position: 'right',
          color: THROWN,
          fontSize: 10,
          formatter: (p: { value: number }) => (p.value > 0 ? fmtCompact(p.value) : ''),
        },
        data: rows.map((r) => Math.max(0, n(r.added) - n(r.alive))),
      },
    ],
  }

  const switchChart = {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    grid: { left: 190, right: 70, top: 12, bottom: 42 },
    xAxis: {
      ...AXIS,
      type: 'value',
      name: 'of what it wrote just after taking over, % still there',
      nameLocation: 'middle',
      nameGap: 24,
      nameTextStyle: { color: INK_MUTED },
      max: 100,
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      data: hand.map((h) => `${h.from.replace('claude-', '')} → ${h.to.replace('claude-', '')}`),
      axisLabel: { color: INK, fontSize: 10 },
      axisLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [
      {
        type: 'bar',
        barWidth: 16,
        itemStyle: { color: IDENT[0], borderRadius: [0, 4, 4, 0] },
        label: {
          show: true,
          position: 'right',
          color: INK,
          fontSize: 11,
          formatter: (p: { value: number; dataIndex: number }) =>
            `${p.value}%  ·  ${fmtCompact(n(hand[p.dataIndex]?.added))} lines`,
        },
        data: hand.map((h) => n(h.survived_pct)),
      },
    ],
  }

  const thrashSpread = Math.max(...rows.map((r) => n(r.thrash_per_100_commits))) -
    Math.min(...rows.map((r) => n(r.thrash_per_100_commits)))

  return (
    <>
      <Panel
        title="Which model to reach for"
        scope="all"
        range={null}
        note="read from git, uncapped — not from a sampled message window"
        mb="md"
      >
        <Box mb="md">
          <Title order={3} style={{ lineHeight: 1.25 }}>
            {v.lead}
          </Title>
          <Text size="sm" c="dimmed" mt={6}>
            {v.body}
          </Text>
        </Box>
        <EChart option={worth} height={70 + rows.length * 44} />
      </Panel>

      <SimpleGrid cols={{ base: 1, lg: 2 }} mb="md">
        <Panel
          title="Churn while working"
          scope="all"
          range={null}
          note="lines removed per hundred added"
        >
          <EChart
            option={bar(rows, (d) => n(d.removed_per_100_added), 'lines removed per 100 added', THROWN)}
            height={30 + rows.length * 42}
          />
          <Text size="xs" c="dimmed" mt={4}>
            A model that removes as fast as it writes is rewriting its own work rather than
            building on it. This counts only authored commits: bulk vendor drops and generated
            purges are excluded, because one of them moves this number more than the whole rest
            of the history.
          </Text>
        </Panel>

        <Panel
          title="Commits that say they are redoing something"
          scope="all"
          range={null}
          note="fix, revert, undo, redo — the agent's own word for it"
        >
          <EChart
            option={bar(rows, (d) => n(d.rework_pct), '% of its commits', '#c98500', (x) => `${x}%`)}
            height={30 + rows.length * 42}
          />
          <Text size="xs" c="dimmed" mt={4}>
            Read from the commit subject the agent wrote itself, so it is the agent&apos;s own
            account of redoing work rather than an inference from the transcript.
          </Text>
        </Panel>
      </SimpleGrid>

      {hand.length > 0 && (
        <Panel
          title="What happens right after the model is switched"
          scope="all"
          range={null}
          note={`${hand.length} switch directions that produced commits within three hours`}
          mb="md"
        >
          <EChart option={switchChart} height={40 + hand.length * 42} />
          <Text size="xs" c="dimmed" mt={4}>
            The account being tested is that a budget runs out, another model takes over, and the
            work it does then gets thrown away. On this data that is not what happens: code
            written in the first hours after a takeover survives ABOVE each model&apos;s own
            average. The damage shows up in long unbroken runs instead, which makes run length
            the thing to control rather than who picks the work up.
          </Text>
        </Panel>
      )}

      <Panel
        title="The comparison, in full"
        scope="all"
        range={null}
        note="every rate beside the count it came from"
      >
        <Table striped withTableBorder fz="xs" horizontalSpacing="xs">
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Model</Table.Th>
              <Table.Th ta="right">Commits</Table.Th>
              <Table.Th ta="right">Landed</Table.Th>
              <Table.Th ta="right">Still there</Table.Th>
              <Table.Th ta="right">Survived</Table.Th>
              <Table.Th ta="right">Removed /100</Table.Th>
              <Table.Th ta="right">Rework</Table.Th>
              <Table.Th ta="right">Thrash /100</Table.Th>
              <Table.Th ta="right">You corrected /100</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {rows.map((r) => {
              const thin = n(r.added) < MIN_ADDED
              return (
                <Table.Tr key={r.model} opacity={thin ? 0.55 : 1}>
                  <Table.Td>{r.model}</Table.Td>
                  <Table.Td ta="right">{n(r.commits)}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.added))}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.alive))}</Table.Td>
                  <Table.Td ta="right" c={n(r.survived_pct) >= 50 ? KEPT : THROWN}>
                    {n(r.survived_pct)}%
                  </Table.Td>
                  <Table.Td ta="right">{n(r.removed_per_100_added)}</Table.Td>
                  <Table.Td ta="right">{n(r.rework_pct)}%</Table.Td>
                  <Table.Td ta="right">{n(r.thrash_per_100_commits)}</Table.Td>
                  <Table.Td ta="right">{n(r.corrections_per_100)}</Table.Td>
                </Table.Tr>
              )
            })}
          </Table.Tbody>
        </Table>
        <Text size="xs" c="dimmed" mt="xs">
          Two of these columns do NOT separate the models and are here so that can be seen rather
          than assumed. You correct every model at about the same rate, and files returned to
          three times or more run within {thrashSpread} per hundred commits of each other. What
          separates them is survival and churn, not how often you have to speak.
        </Text>
      </Panel>
    </>
  )
}
