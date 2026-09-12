import { Group, SimpleGrid, Text, Tooltip } from '@mantine/core'
import type { StatsSummary } from '../../generated/StatsSummary'
import { MONO } from '../../EChart'
import { CANCEL, Counter, FAIL, Panel, Stat, n, pct } from './parts'

// Deviations (#392). The page measures what the agents did, and since
// #385 what they shipped. This band is what they got WRONG, and what
// they skipped. Most of the signals already existed, scattered across
// six other bands where each read as trivia; gathered, they are an
// error record.
//
// Two are new and are the sharpest. A pull request opened without the
// gates having run is a rule broken, and the transcript carries every
// command with its time. A write into the kernel's crate or a schema
// file is the one line that is absolute.
//
// What is NOT here matters as much: an undo is found by comparing an
// edit's text with a later one's, and a shell edit carries no such
// text (#383). Where that is the case the counter says unknown rather
// than a confident zero.

export function DeviationsSection({ s, range }: { s: StatsSummary | undefined; range: string | null }) {
  const opened = n(s?.prs_opened)
  const gated = n(s?.prs_gated)
  const ungated = n(s?.prs_ungated)
  const bright = n(s?.bright_line_writes)
  const paths = s?.bright_line_paths ?? []
  const blind = n(s?.replaced_unknown) > 0

  const scored = (s?.tool_outcomes ?? []).reduce((a, t) => a + n(t.ok) + n(t.failed) + n(t.cancelled), 0)
  const failed = (s?.tool_outcomes ?? []).reduce((a, t) => a + n(t.failed), 0)
  const failRate = pct(failed, scored)

  return (
    <>
      <Panel
        title="Rules"
        scope="range"
        range={range}
        note="the checks that had to run, and the line that must not be crossed"
        mb="md"
      >
        <SimpleGrid cols={{ base: 2, md: 4 }} spacing="xs">
          <Stat
            label="PRs gated"
            value={opened === 0 ? '—' : `${gated} of ${gated + ungated}`}
            sub={
              opened === 0
                ? 'none opened in this range'
                : gated + ungated < opened
                  ? `${opened - gated - ungated} opened without writing`
                  : 'tests, clippy and the audit'
            }
            tone={ungated > 0 ? 'bad' : gated > 0 ? 'ok' : undefined}
            tip="a pull request counts as gated when tests, clippy and the skill audit all ran in that session after its last write. One opened by a session that changed nothing is neither gated nor ungated — there was nothing to check — so the two need not sum to the number opened."
          />
          <Stat
            label="Opened ungated"
            value={ungated === 0 ? '0' : String(ungated)}
            sub={ungated > 0 ? 'a gate was skipped' : 'no gate skipped'}
            tone={ungated > 0 ? 'bad' : 'ok'}
            tip="written, then a pull request opened with at least one of the three checks missing since that write"
          />
          <Stat
            label="Bright line"
            value={bright === 0 ? 'clear' : String(bright)}
            sub={bright === 0 ? 'kernel and schema untouched' : 'writes that should not exist'}
            tone={bright > 0 ? 'bad' : 'ok'}
            tip="writes into the kernel's own crate or into a schema file. The rule is absolute: those belong to the operator, in their own change."
          />
          <Stat
            label="Refused · stopped"
            value={`${n(s?.denials)} · ${n(s?.interventions)}`}
            sub="you said no, you stepped in"
            tone={n(s?.denials) + n(s?.interventions) > 0 ? 'warn' : 'ok'}
            tip="tool calls you refused, and turns where you interrupted or corrected — the deviations caught by a human rather than by a gate"
          />
        </SimpleGrid>
        {paths.length > 0 && (
          <Group gap={6} mt="sm" wrap="wrap">
            <Text size="xs" c="dimmed">
              crossed at:
            </Text>
            {paths.map((p) => (
              <Text key={p} size="xs" ff={MONO} c={FAIL}>
                {p.split('/').slice(-3).join('/')}
              </Text>
            ))}
          </Group>
        )}
      </Panel>

      <Panel
        title="What went wrong"
        scope="range"
        range={range}
        note="every error signal the range carries, in one place"
      >
        <SimpleGrid cols={{ base: 3, md: 5, lg: 8 }} spacing="xs">
          <Counter label="Tool failures" value={failed} tone={failed > 0 ? FAIL : undefined} tip="calls that came back an error" />
          <Counter label="Failure rate" value={failRate == null ? '—' : `${failRate}%`} tip="failed ÷ calls whose result was seen" />
          <Counter label="Compile errors" value={s?.compile_errors} tone={n(s?.compile_errors) > 0 ? FAIL : undefined} tip="diagnostics read out of what the compilers printed" />
          <Counter label="Tests failed" value={s?.tests_failed} tone={n(s?.tests_failed) > 0 ? FAIL : undefined} tip="from the runners' own tallies" />
          <Counter label="Interrupted" value={s?.interrupted_calls} tone={n(s?.interrupted_calls) > 0 ? CANCEL : undefined} tip="calls stopped before they finished" />
          <Counter label="Compactions" value={s?.compactions} tone={n(s?.compactions) > 0 ? CANCEL : undefined} tip="times a session ran out of context and had to be summarised" />
          <Counter
            label="Work undone"
            value={n(s?.reverts) === 0 && blind ? '—' : s?.reverts}
            tone={n(s?.reverts) > 0 ? FAIL : undefined}
            tip={
              n(s?.reverts) === 0 && blind
                ? `an undo is seen by comparing an edit's text with a later one's — ${n(s?.replaced_unknown)} edits here carried no such text, so undone work is not visible`
                : 'edits whose work a later edit threw away'
            }
          />
          <Counter label="Thrash files" value={s?.thrash_files} tone={n(s?.thrash_files) > 0 ? CANCEL : undefined} tip={`files touched ${s?.revisit_at ?? 3} or more times`} />
          <Counter label="Secrets sent" value={s?.exposure?.secret_hits} tone={n(s?.exposure?.secret_hits) > 0 ? FAIL : undefined} tip="tool output that looked like a credential and went into the next prompt" />
          <Counter label="Outside reads" value={s?.exposure?.outside_reads} tone={n(s?.exposure?.outside_reads) > 0 ? CANCEL : undefined} tip="files read from beyond the directory the agent was working in" />
          <Counter
            label="Unasked rewrites"
            value={n(s?.edits_directed) + n(s?.edits_self) === 0 ? '—' : `${pct(n(s?.edits_self), n(s?.edits_directed) + n(s?.edits_self))}%`}
            tip="share of rewrites with nobody steering — the agent going back over its own work"
          />
          <Counter label="Circling" value={(s?.live ?? []).filter((l) => n(l.files_revisited) > 0).length} tip="live sessions coming back to the same files" />
        </SimpleGrid>
        <Text size="xs" c="dimmed" mt="sm">
          An omission — asked for three things, delivered two — is not here, and cannot be, until the ask is recorded
          beside the work. That is a separate piece of work, not a number this range can produce.
        </Text>
      </Panel>
    </>
  )
}
