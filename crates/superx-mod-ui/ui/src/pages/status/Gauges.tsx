import { SimpleGrid } from '@mantine/core'
import type { InsightsSummary } from '../../generated/InsightsSummary'
import type { StatsSummary } from '../../generated/StatsSummary'
import { OK, PELICAN } from '../../EChart'
import { BANDS, CANCEL, FAIL, Gauge, HIGH_GOOD, LOW_GOOD, fmtCompact, n, pct } from './parts'

// The six-pack (#367): the primary instruments, as dials with banded
// arcs. Attitude is churn, heading is who directed the rewrites, and
// the rest say whether the work holds, the tools work, the cache pays
// and the machine is being flown around the clock.

export function Gauges({ s, i, range }: { s: StatsSummary | undefined; i: InsightsSummary | undefined; range: string | null }) {
  const added = n(s?.lines_added)
  const replaced = n(s?.lines_removed)
  // Nothing replaced on the line but edits of unknown replaced size
  // (#383): the needle has nothing true to point at.
  const unknown = n(s?.replaced_unknown)
  const transcriptChurn = replaced === 0 && unknown > 0 ? null : pct(replaced, added + replaced)
  // When the transcript cannot say what was replaced, the repository
  // can: what landed on main, read with git (#386).
  const landedAdded = n(s?.landed?.added)
  const landedRemoved = n(s?.landed?.removed)
  const landedChurn = pct(landedRemoved, landedAdded + landedRemoved)
  const churn = transcriptChurn ?? landedChurn

  const directed = n(s?.churn_directed)
  const self = n(s?.churn_self)
  const onCourse = pct(directed, directed + self)

  const passed = n(s?.tests_passed)
  const failedTests = n(s?.tests_failed)
  const pass = pct(passed, passed + failedTests)

  const outcomes = s?.tool_outcomes ?? []
  const scored = outcomes.reduce((a, t) => a + n(t.ok) + n(t.failed) + n(t.cancelled), 0)
  const failed = outcomes.reduce((a, t) => a + n(t.failed), 0)
  const toolsOk = scored > 0 ? Math.round(((scored - failed) * 100) / scored) : null

  const tok = i?.tokens
  const promptTotal = tok ? n(tok.input) + n(tok.cache_read) + n(tok.cache_write) : 0
  const cacheHit = tok && promptTotal > 0 ? Math.round((n(tok.cache_read) * 100) / promptTotal) : null

  const coverage = s ? Math.round((n(s.active_hours_24h) * 100) / 24) : null

  const rangeNote = range === 'window' ? 'this window' : range ? `last ${range}` : ''

  return (
    <SimpleGrid cols={{ base: 2, sm: 3, lg: 6 }} spacing="xs" mb="md">
      <Gauge
        label="Churn"
        value={s ? churn : null}
        bands={LOW_GOOD}
        sub={
          s && transcriptChurn != null
            ? `${fmtCompact(added)} added · ${fmtCompact(replaced)} replaced${unknown > 0 ? ` · ${unknown} of unknown size` : ''}`
            : s && landedChurn != null
              ? `landed on main: ${fmtCompact(landedAdded)} added · ${fmtCompact(landedRemoved)} removed`
              : unknown > 0
                ? `${fmtCompact(added)} added · ${unknown} edits of unknown size`
                : 'no code moved'
        }
        tip={`replaced ÷ (added + replaced) over ${rangeNote}. 0% is all new code; past ${BANDS.churnBad}% the window spent itself rewriting. From the transcript when it can see what was replaced, else from what landed on main as git reports it.`}
      />
      <Gauge
        label="On course"
        value={s ? onCourse : null}
        bands={[[BANDS.directedBad / 100, FAIL], [BANDS.directedOk / 100, CANCEL], [1, OK]]}
        sub={s && onCourse != null ? `${fmtCompact(directed)} directed · ${fmtCompact(self)} self` : unknown > 0 ? `${unknown} rewrites of unknown size` : 'nothing rewritten'}
        tip="share of replaced lines that followed a human instruction. Low means the agents are rewriting themselves with nobody steering."
      />
      <Gauge
        label="Tests green"
        value={s ? pass : null}
        bands={HIGH_GOOD}
        sub={s && pass != null ? `${fmtCompact(passed)} passed · ${fmtCompact(failedTests)} failed` : 'no tally read'}
        tip="pass rate read out of what the test runners printed"
      />
      <Gauge
        label="Tools OK"
        value={s ? toolsOk : null}
        bands={HIGH_GOOD}
        sub={s && toolsOk != null ? `${failed} of ${scored} calls failed` : 'no calls scored'}
        tip="tool calls that did not come back an error"
      />
      <Gauge
        label="Cache hit"
        value={i ? cacheHit : null}
        bands={HIGH_GOOD}
        sub={tok ? `${fmtCompact(tok.cache_read)} served from cache` : ''}
        tip="cache reads ÷ (cache reads + fresh input + cache writes), across every captured message"
      />
      <Gauge
        label="Coverage"
        value={coverage}
        // A literal: the canvas cannot resolve a CSS variable.
        bands={[[1, PELICAN]]}
        sub={s ? `${s.active_hours_24h} of the last 24 hours` : ''}
        tip="hours of the last day that saw any work — a 24×7 operator's instrument"
      />
    </SimpleGrid>
  )
}
