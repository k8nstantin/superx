import { useMemo, useState } from 'react'
import { Alert, Group, Loader, SegmentedControl, Select, Stack, Text, Title } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchFindings } from '../api'
import { Ci, ciAnswer } from './Ci'
import { Cost, costAnswer } from './Cost'
import { Decide } from './Decide'
import { Direction, directionAnswer } from './Direction'
import { Gaps, gaps } from './Gaps'
import { Landed, landedAnswer } from './Landed'
import { Now } from './Now'
import { Practices, practicesAnswer } from './Practices'
import { Rewrites, rewritesAnswer } from './Rewrites'
import { BehindDrawer, type Behind } from './Rows'
import { ALL, JUDGED, judged, repositories, scoped, type Scope } from './scope'
import { local, n, palette } from './shared'

// The Results: what recon follows the agents' work into, as the decisions
// it informs. Each view asks one question; its title is the answer computed
// from the facts in view, and every bar opens on the rows behind it.

export const RESULT_VIEWS: [string, string][] = [
  ['decide', 'Decide'],
  ['now', 'Now'],
  ['landed', 'Landed'],
  ['ci', 'CI & pull requests'],
  ['practices', 'Practices'],
  ['direction', 'Direction'],
  ['cost', 'Cost'],
  ['rewrites', 'Rewrites'],
  ['gaps', 'Gaps'],
]

const QUESTION: Record<string, string> = {
  decide: 'Which model, how to direct it, and what to fix?',
  now: 'What was done in each repository, and what is stuck?',
  landed: "Which model's work reached the main line, and is still there?",
  ci: 'Does CI tell you anything, and how do pull requests go?',
  practices: 'Does testing first, thinking effort or context change what lands?',
  direction: 'Are you directing better, and does it show in the code?',
  cost: 'What does a surviving line cost?',
  rewrites: 'Where was work rewritten away?',
  gaps: 'What recon cannot judge yet',
}

const METHOD: Record<string, string> = {
  decide: 'A commit is a model\'s when its own session printed it, else when that model\'s run was the only one in the repository at the time. Bulk drops (above attr_recon_bulk_lines) are left out.',
  landed: 'Landed: the lines of each file whose version the main line carries, squash-merges included. Alive: what blame credits today; a squash\'s blame goes to the branch commits it carried. In flight: on a checked-out branch or an open pull request. Older work has had longer to be replaced: Decide sets models against each other only where both worked.',
  ci: 'Host CI runs are GitHub workflow runs and Bitbucket pipelines; a pull request\'s first verdict is the first run on its commits that gave one.',
  practices: 'Only a commit its session printed carries what came before it: the tests and builds that session ran since its previous commit, and the effort and context of the reply that committed.',
  direction: 'A correction is a prompt with one of attr_recon_correction_words, an escalation one with attr_recon_escalation_words, a stop the operator interrupting the agent.',
  cost: 'Output tokens and active hours of the replies made in the repositories in view; active time is the gaps between a session\'s replies of one model, each capped at attr_recon_active_secs.',
  rewrites: 'From the reflogs of every checkout and mirror recon reads: what each ref held, and what moved it.',
}

export function Results({ sub, go }: { sub: string; go: (sub: string) => void }) {
  const q = useQuery({ queryKey: ['findings'], queryFn: fetchFindings, refetchInterval: 300_000 })
  const [scope, setScope] = useState<Scope>({ repo: JUDGED, evidence: 'all' })
  const [behind, setBehind] = useState<Behind>(null)
  const f = q.data
  const pal = useMemo(() => palette(f?.weeks ?? []), [f])
  const s = useMemo(() => (f ? scoped(f, scope) : null), [f, scope])
  const view = RESULT_VIEWS.some(([k]) => k === sub) ? sub : 'decide'
  const answer = (() => {
    if (!f || !s) return null
    switch (view) {
      case 'landed':
        return landedAnswer(s)
      case 'ci':
        return ciAnswer(s)
      case 'practices':
        return practicesAnswer(s, pal)
      case 'direction':
        return directionAnswer(s)
      case 'cost':
        return costAnswer(s, pal)
      case 'rewrites':
        return rewritesAnswer(s)
      case 'gaps': {
        const g = gaps(f)
        return g.length ? `${g.length} things limit what recon can judge.` : 'Nothing limits what recon can judge.'
      }
      default:
        return null
    }
  })()
  const scoping = view !== 'now' && view !== 'gaps'
  return (
    <Stack gap="md">
      <SegmentedControl value={view} onChange={go} data={RESULT_VIEWS.map(([value, label]) => ({ value, label }))} />
      <Stack gap={4}>
        <Text ff="monospace" fz="sm" c="pelican.3">{QUESTION[view]}</Text>
        {answer && <Title order={3} style={{ textWrap: 'balance' }}>{answer}</Title>}
        {METHOD[view] && <Text fz="sm" c="dimmed" maw={1100}>{METHOD[view]}</Text>}
      </Stack>
      {scoping && f && (
        <Group gap="sm" align="end">
          <Select
            label="Repositories"
            size="xs"
            w={420}
            value={scope.repo}
            onChange={(v) => v && setScope({ ...scope, repo: v })}
            allowDeselect={false}
            data={[
              { value: JUDGED, label: `Judged: ${judged(f).join(', ') || 'none'}` },
              { value: ALL, label: 'Every repository' },
              ...repositories(f).map((r) => ({ value: r, label: r })),
            ]}
          />
          <SegmentedControl
            size="xs"
            value={scope.evidence}
            onChange={(v) => setScope({ ...scope, evidence: v as Scope['evidence'] })}
            data={[
              { value: 'all', label: 'Every credited commit' },
              { value: 'printed', label: 'Printed by their session only' },
            ]}
          />
          {s && <Text fz="xs" c="dimmed">{n(s.commits.length)} credited commits · {n(s.pulls.length)} pull requests · read {local(f.computed_at)}</Text>}
        </Group>
      )}
      {view === 'now' && <Now pal={pal} />}
      {view !== 'now' && q.isPending && <Loader size="sm" />}
      {view !== 'now' && q.isError && <Alert color="red" title="The findings did not load">{String(q.error)}</Alert>}
      {s && f && view === 'decide' && <Decide s={s} pal={pal} open={setBehind} go={go} />}
      {s && view === 'landed' && <Landed s={s} pal={pal} open={setBehind} />}
      {s && view === 'ci' && <Ci s={s} pal={pal} open={setBehind} />}
      {s && view === 'practices' && <Practices s={s} pal={pal} open={setBehind} />}
      {s && view === 'direction' && <Direction s={s} pal={pal} open={setBehind} />}
      {s && view === 'cost' && <Cost s={s} pal={pal} />}
      {s && view === 'rewrites' && <Rewrites s={s} pal={pal} open={setBehind} />}
      {f && view === 'gaps' && <Gaps f={f} />}
      <BehindDrawer behind={behind} close={() => setBehind(null)} pal={pal} />
    </Stack>
  )
}
