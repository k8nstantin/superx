import { SimpleGrid } from '@mantine/core'
import type { InsightsSummary } from '../../generated/InsightsSummary'
import type { StatsSummary } from '../../generated/StatsSummary'
import type { StatusResponse } from '../../generated/StatusResponse'
import { BANDS, Lamp, fmtAge, fmtCompact, lowGood, n, pct, type Tone } from './parts'

// The annunciator panel (#367): a row of lamps that stay dark until
// something is wrong. A pilot does not read twelve numbers to learn
// that nothing is on fire — the panel is dark, or it is not.

export function Annunciator({
  s,
  i,
  status,
  jump,
}: {
  s: StatsSummary | undefined
  i: InsightsSummary | undefined
  status: StatusResponse | undefined
  jump: (id: string) => void
}) {
  const count = (v: number | bigint | null | undefined, bad = false): Tone =>
    n(v) > 0 ? (bad ? 'bad' : 'warn') : 'ok'

  const scored = (s?.tool_outcomes ?? []).reduce((a, t) => a + n(t.ok) + n(t.failed) + n(t.cancelled), 0)
  const failed = (s?.tool_outcomes ?? []).reduce((a, t) => a + n(t.failed), 0)
  const failRate = pct(failed, scored)

  const live = s?.live ?? []
  const circling = live.filter(
    (l) => n(l.self_churn_pct) >= BANDS.selfChurnBad || n(l.files_revisited) >= BANDS.revisitedBad,
  ).length
  const nearCeiling = live.filter((l) => l.context_pct != null && n(l.context_pct) >= BANDS.contextWarn).length
  const atCeiling = live.filter((l) => l.context_pct != null && n(l.context_pct) >= BANDS.contextBad).length

  const modulesDown = (status?.modules ?? []).filter((m) => m.lifecycle !== 'active').length
  const moduleFailures = (i?.module_health ?? []).reduce((a, h) => a + n(h.failures_recent), 0)

  const lag = i?.last_event_secs == null ? null : n(i.last_event_secs)
  const lagTone = lowGood(lag, BANDS.lagWarn, BANDS.lagBad)

  const loading = !s
  const v = (x: number | bigint | null | undefined) => (loading ? '…' : fmtCompact(x))

  return (
    <SimpleGrid cols={{ base: 2, sm: 4, lg: 7 }} spacing="xs" mb="md">
      <Lamp
        label="Capture lag"
        value={lag == null ? (i ? '—' : '…') : fmtAge(lag)}
        tone={lagTone}
        sub={i ? `${fmtCompact(i.events_last_hour)} events this hour` : ''}
        tip="age of the newest captured event — the capture-alive signal. Amber past five minutes, red past thirty."
        onClick={() => jump('systems')}
      />
      <Lamp
        label="Modules"
        value={status ? (modulesDown > 0 ? `${modulesDown} down` : moduleFailures > 0 ? `${moduleFailures} fail` : 'all up') : '…'}
        tone={modulesDown > 0 ? 'bad' : moduleFailures > 0 ? 'warn' : 'ok'}
        sub={status ? `${status.modules.length} registered` : ''}
        tip="modules not in the active state, and module failures logged in the last day"
        onClick={() => jump('systems')}
      />
      <Lamp
        label="Circling"
        value={loading ? '…' : String(circling)}
        tone={circling > 0 ? 'bad' : 'ok'}
        sub="agents rewriting themselves"
        tip={`live sessions with ${BANDS.selfChurnBad}%+ of their rewrites unasked, or files touched ${s?.revisit_at ?? 3}+ times — the one worth interrupting`}
        onClick={() => jump('deck')}
      />
      <Lamp
        label="Context"
        value={loading ? '…' : nearCeiling > 0 ? `${nearCeiling} high` : 'clear'}
        tone={atCeiling > 0 ? 'bad' : nearCeiling > 0 ? 'warn' : 'ok'}
        sub={`agents past ${BANDS.contextWarn}% of the window`}
        tip="live sessions whose context window is filling — compaction is next, and compaction is dead time"
        onClick={() => jump('deck')}
      />
      <Lamp
        label="Tests failed"
        value={v(s?.tests_failed)}
        tone={loading ? 'none' : count(s?.tests_failed, true)}
        sub={s ? `${fmtCompact(s.tests_passed)} passed` : ''}
        tip="failing tests read out of what the test runners printed"
        onClick={() => jump('quality')}
      />
      <Lamp
        label="Compile errors"
        value={v(s?.compile_errors)}
        tone={loading ? 'none' : count(s?.compile_errors, true)}
        tip="rustc / tsc diagnostics seen in tool output"
        onClick={() => jump('quality')}
      />
      <Lamp
        label="Tool failures"
        value={loading ? '…' : failRate == null ? '—' : `${failRate}%`}
        tone={loading ? 'none' : lowGood(failRate, BANDS.toolFailWarn, BANDS.toolFailBad)}
        sub={scored > 0 ? `${failed} of ${scored} scored` : ''}
        tip="calls that came back an error. Some failure is normal — a grep with no match — so the lamp lights on the rate."
        onClick={() => jump('quality')}
      />
      <Lamp
        label="Interventions"
        value={v(s?.interventions)}
        tone={loading ? 'none' : count(s?.interventions)}
        sub="you stepped in"
        tip="messages carrying an interruption or a correction from the operator"
        onClick={() => jump('quality')}
      />
      <Lamp
        label="Denials"
        value={v(s?.denials)}
        tone={loading ? 'none' : count(s?.denials)}
        sub="not permitted"
        tip="tool calls the agent was not allowed to make"
        onClick={() => jump('quality')}
      />
      <Lamp
        label="Compactions"
        value={v(s?.compactions)}
        tone={loading ? 'none' : count(s?.compactions)}
        sub="context ran out"
        tip="the agent stopped, re-read its own history and resumed with less of it"
        onClick={() => jump('quality')}
      />
      <Lamp
        label="Interrupted"
        value={v(s?.interrupted_calls)}
        tone={loading ? 'none' : count(s?.interrupted_calls)}
        sub="calls stopped early"
        tip="commands that were stopped before they finished"
        onClick={() => jump('quality')}
      />
      <Lamp
        label="Secrets sent"
        value={v(s?.exposure?.secret_hits)}
        tone={loading ? 'none' : count(s?.exposure?.secret_hits, true)}
        sub="in prompts"
        tip="tool results whose content matched a credential shape and therefore went into a prompt. Anything sent cannot be recalled."
        onClick={() => jump('cost')}
      />
      <Lamp
        label="Outside reads"
        value={v(s?.exposure?.outside_reads)}
        tone={loading ? 'none' : count(s?.exposure?.outside_reads)}
        sub="beyond the working dir"
        tip="file reads from outside the directory the agent was working in — exposure nobody asked for"
        onClick={() => jump('cost')}
      />
      <Lamp
        label="Sample"
        value={loading ? '…' : s.truncated ? 'capped' : 'complete'}
        tone={loading ? 'none' : s.truncated ? 'warn' : 'ok'}
        sub={s?.truncated ? 'row cap reached' : 'every row in range'}
        tip="whether the range walk read every message in the range or hit its row cap — capped figures are a sample"
      />
    </SimpleGrid>
  )
}
