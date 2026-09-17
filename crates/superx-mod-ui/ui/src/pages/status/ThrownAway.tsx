import { Box, Group, SimpleGrid, Table, Text, Title } from '@mantine/core'
import { AXIS, EChart, GRID_LINE, INK, INK_MUTED, TOOLTIP } from '../../EChart'
import { Panel, fmtCompact, n } from './parts'
import type { ThrownSummary } from '../../generated/ThrownSummary'
import type { ThrownAway as Row } from '../../generated/ThrownAway'

// What the work threw away (#406).
//
// This section replaces one that could not make its case. That one
// drew four charts on a capped read path, credited one model's
// surviving lines to another, and argued the reverse of the truth.
// The lesson is in the shape of this file: the CONCLUSION is a
// sentence, computed from the same numbers the charts draw, and it
// refuses to rank when the data does not separate the models. Charts
// support the sentence; they are not asked to replace it.
//
// Forms by job:
//  · part-to-whole of one model's spend  → stacked bar, both segments
//    directly labelled (the green/red pair sits in the 6–8 CVD band,
//    which is legal only with secondary encoding)
//  · one number per model                → plain bar, directly labelled
//  · the full accounting with caveats    → a table, not a picture
// Nothing here carries two y-scales.

const KEPT = '#199e70'
const THROWN = '#e66767'

/// A model only counts in the verdict when it landed enough for the
/// ratio to mean anything. Below this the page shows the row and
/// leaves it out of the ranking.
const MIN_LANDED = 2000

/// The verdict, in words, or an honest refusal.
///
/// Two models separate only when one keeps a clearly larger share of
/// what it wrote AND the age of its work does not explain it. Older
/// code has had longer to be deleted, so a winner whose code is OLDER
/// is a stronger result; a winner whose code is much younger is not a
/// result at all, and this says so instead of ranking.
function verdict(rows: Row[]): { lead: string; body: string; ranked: boolean } {
  const ok = rows.filter((r) => n(r.landed) >= MIN_LANDED)
  if (ok.length < 2) {
    return {
      lead: 'Not enough landed work to compare models yet.',
      body: `A model needs ${fmtCompact(MIN_LANDED)} landed lines before its survival rate means anything. The table below shows what there is.`,
      ranked: false,
    }
  }
  const worstFirst = ok.reduce((a, b) => (n(a.survived_pct) <= n(b.survived_pct) ? a : b))
  // Which comparison to lead with. The highest survival rate is not
  // automatically the strongest claim: if the winner's code is much
  // younger it has simply had less time to be deleted, and age is
  // doing the work. So prefer a winner whose code is at least as OLD
  // as the loser's — that comparison survives the obvious objection,
  // and it is the one a reader can be asked to act on. Only when no
  // such winner exists does this fall back to the raw leader, and say
  // plainly that age may explain the gap.
  const beats = ok.filter((r) => n(r.survived_pct) > n(worstFirst.survived_pct))
  const robust = beats.filter((r) => n(r.median_age_days) >= n(worstFirst.median_age_days))
  const pool = robust.length > 0 ? robust : beats
  const best =
    pool.length > 0
      ? pool.reduce((a, b) => (n(a.survived_pct) >= n(b.survived_pct) ? a : b))
      : worstFirst
  const worst = worstFirst
  const gap = n(best.survived_pct) - n(worst.survived_pct)
  if (gap < 5) {
    return {
      lead: 'These models do not separate on survival.',
      body: `The best and worst are ${gap} points apart, which is too close to call a difference. Tool failure and test pass rates put them within noise of each other too.`,
      ranked: false,
    }
  }
  const younger = n(best.median_age_days) < n(worst.median_age_days)
  const ratio =
    n(worst.tokens_per_line_kept) > 0 && n(best.tokens_per_line_kept) > 0
      ? n(worst.tokens_per_line_kept) / n(best.tokens_per_line_kept)
      : null
  const price =
    ratio && ratio >= 1.2
      ? ` A line that is still in the tree cost ${fmtCompact(n(worst.tokens_per_line_kept))} tokens from ${worst.model} and ${fmtCompact(n(best.tokens_per_line_kept))} from ${best.model}, ${ratio.toFixed(1)} times the price.`
      : ''
  const age = younger
    ? ` Read this one carefully: ${best.model}'s work is the YOUNGER of the two, at ${n(best.median_age_days)} days against ${n(worst.median_age_days)}, and younger code has had less time to be deleted. Some of the gap is age rather than quality.`
    : ` The confounder runs the wrong way to explain it away. ${best.model}'s work is the OLDER of the two, at ${n(best.median_age_days)} days against ${n(worst.median_age_days)}, so it has had MORE time to be deleted, and it survived anyway.`
  return {
    lead: `${worst.model} threw away ${fmtCompact(n(worst.tokens_thrown))} tokens of work.`,
    body: `Of the lines it landed, ${100 - n(worst.survived_pct)}% are no longer in the tree, against ${100 - n(best.survived_pct)}% for ${best.model}.${price}${age}`,
    ranked: true,
  }
}

export function ThrownAwaySection({ t }: { t: ThrownSummary | undefined }) {
  const rows = t?.models ?? []
  if (!t) {
    return (
      <Panel title="What the work threw away" scope="all" range={null}>
        <Text size="xs" c="dimmed">
          reading the repositories — git says what landed, blame says what is left.
        </Text>
      </Panel>
    )
  }
  if (rows.length === 0) {
    return (
      <Panel title="What the work threw away" scope="all" range={null}>
        <Text size="xs" c="dimmed">
          no commit could be credited to a model. A commit is credited only when exactly one
          model was at work in that checkout as it landed.
        </Text>
      </Panel>
    )
  }

  const v = verdict(rows)
  const names = rows.map((r) => r.model)
  // The price chart is a ranking, so it is ordered by price rather
  // than by the spend chart's order. Cheapest at the top once ECharts
  // flips the category axis.
  const byPrice = [...rows].sort((a, b) => n(b.tokens_per_line_kept) - n(a.tokens_per_line_kept))

  // The headline: what each model's tokens bought, split by whether it
  // is still there. Part-to-whole, so a stacked bar, and both segments
  // carry their own label.
  const spend = {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    legend: {
      data: ['bought code that is still there', 'thrown away'],
      textStyle: { color: INK_MUTED },
      top: 0,
      right: 0,
    },
    grid: { left: 150, right: 90, top: 34, bottom: 34 },
    xAxis: {
      ...AXIS,
      type: 'value',
      name: 'output tokens',
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
        name: 'bought code that is still there',
        type: 'bar',
        stack: 'spend',
        barWidth: 16,
        itemStyle: { color: KEPT, borderColor: 'transparent', borderWidth: 1 },
        label: {
          show: true,
          color: '#fff',
          fontSize: 10,
          formatter: (p: { value: number }) => (p.value > 0 ? fmtCompact(p.value) : ''),
        },
        data: rows.map((r) => Math.max(0, n(r.out_tokens) - n(r.tokens_thrown))),
      },
      {
        name: 'thrown away',
        type: 'bar',
        stack: 'spend',
        barWidth: 16,
        itemStyle: { color: THROWN, borderColor: 'transparent', borderWidth: 1 },
        label: {
          show: true,
          position: 'right',
          color: THROWN,
          fontSize: 10,
          formatter: (p: { value: number }) => (p.value > 0 ? fmtCompact(p.value) : ''),
        },
        data: rows.map((r) => n(r.tokens_thrown)),
      },
    ],
  }

  // The unit price: what one surviving line cost. One measure, one bar
  // per model, labelled at its end.
  const price = {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    grid: { left: 150, right: 70, top: 14, bottom: 40 },
    xAxis: {
      ...AXIS,
      type: 'value',
      name: 'output tokens per line that is STILL THERE',
      nameLocation: 'middle',
      nameGap: 24,
      nameTextStyle: { color: INK_MUTED },
      axisLabel: { color: INK_MUTED },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      data: byPrice.map((r) => r.model),
      axisLabel: { color: INK },
      axisLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [
      {
        type: 'bar',
        barWidth: 16,
        itemStyle: { color: '#B833E8', borderRadius: [0, 4, 4, 0] },
        label: {
          show: true,
          position: 'right',
          color: INK,
          fontSize: 11,
          formatter: (p: { value: number }) => `${p.value}`,
        },
        data: byPrice.map((r) => n(r.tokens_per_line_kept)),
      },
    ],
  }

  return (
    <>
      <Panel
        title="What the work threw away"
        scope="all"
        range={null}
        note="git says what landed, blame says what is left"
        mb="md"
      >
        <Box mb="md">
          <Title order={3} style={{ lineHeight: 1.25 }}>
            {v.lead}
          </Title>
          {v.body && (
            <Text size="sm" c="dimmed" mt={6}>
              {v.body}
            </Text>
          )}
        </Box>
        <EChart option={spend} height={60 + rows.length * 44} />
        <Text size="xs" c="dimmed" mt={4}>
          Thrown away is the model&apos;s own price per landed line, charged on the lines that
          are no longer in the tree. It is the column no price list carries.
        </Text>
      </Panel>

      <SimpleGrid cols={{ base: 1, lg: 2 }} mb="md">
        <Panel
          title="What one surviving line cost"
          scope="all"
          range={null}
          note="the bill actually paid"
        >
          <EChart option={price} height={40 + rows.length * 44} />
        </Panel>

        <Panel
          title="The accounting"
          scope="all"
          range={null}
          note={`${t.sessions} sessions · ${t.repos} checkouts`}
        >
          <Table striped withTableBorder fz="xs" horizontalSpacing="xs">
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Model</Table.Th>
                <Table.Th ta="right">Landed</Table.Th>
                <Table.Th ta="right">Still there</Table.Th>
                <Table.Th ta="right">Survived</Table.Th>
                <Table.Th ta="right">Tok / kept</Table.Th>
                <Table.Th ta="right">Age</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {rows.map((r) => {
                const thin = n(r.landed) < MIN_LANDED
                return (
                  <Table.Tr key={r.model} opacity={thin ? 0.55 : 1}>
                    <Table.Td>
                      <Group gap={6} wrap="nowrap">
                        <Text fz="xs">{r.model}</Text>
                        {thin && (
                          <Text fz={9} c="dimmed">
                            thin
                          </Text>
                        )}
                      </Group>
                    </Table.Td>
                    <Table.Td ta="right">{fmtCompact(n(r.landed))}</Table.Td>
                    <Table.Td ta="right">{fmtCompact(n(r.alive))}</Table.Td>
                    <Table.Td ta="right" c={n(r.survived_pct) >= 50 ? KEPT : THROWN}>
                      {n(r.survived_pct)}%
                    </Table.Td>
                    <Table.Td ta="right">{fmtCompact(n(r.tokens_per_line_kept))}</Table.Td>
                    <Table.Td ta="right">{n(r.median_age_days)}d</Table.Td>
                  </Table.Tr>
                )
              })}
            </Table.Tbody>
          </Table>
          <Text size="xs" c="dimmed" mt="xs">
            Age is here so it can be checked before anything is ranked: older code has had more
            time to be deleted.{' '}
            {n(t.uncredited_commits) > 0 && (
              <>
                {fmtCompact(n(t.uncredited_commits))} commits carrying{' '}
                {fmtCompact(n(t.uncredited_lines))} lines are left uncredited, because no single
                model was at work in that checkout when they landed.
              </>
            )}
          </Text>
        </Panel>
      </SimpleGrid>
      {!v.ranked && rows.length > 1 && (
        <Text size="xs" c="dimmed">
          The page is not ranking these models. It will say one is worse only when the survival
          gap is wide enough to mean something.
        </Text>
      )}
    </>
  )
}
