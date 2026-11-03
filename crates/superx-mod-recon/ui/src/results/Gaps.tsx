import { Alert, Button, Group, List, Paper, Stack, Text, Title } from '@mantine/core'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { fetchParams, setParam, type Findings } from '../api'
import { judged } from './scope'
import { group, n, sum } from './shared'

// What recon cannot judge yet, read from the facts. First, for each
// repository, whether the main line the crawler reads is the branch its work
// is merged into: where its merged pull requests went says so, and a
// mismatch is one click from set. Then repositories whose main line took
// none of the credited work, folders read as a repository, snapshots left
// out, repositories no CI judges, commits no session's work covers, and work
// done in no repository.

const MAINLINE = 'attr_recon_mainline_refs'

export type Gap = { title: string; detail: string; set?: { repo: string; ref: string } }

/// A ref as a branch's short name: `refs/remotes/origin/sandbox` is `sandbox`.
const short = (ref: string) => ref.replace(/^refs\/remotes\/[^/]+\//, '').replace(/^refs\/heads\//, '')

/// The done branch, read from merged pull requests: the branch most of a
/// repository's merged pull requests went into, with how many did.
export function mergedInto(f: Findings, repo: string): { branch: string; merged: number; of: number } | null {
  const merged = f.pulls.filter((p) => p.repo === repo && p.merged && p.target_branch)
  if (!merged.length) return null
  const [branch, count] = [...group(merged, (p) => p.target_branch).entries()].map(([b, ps]) => [b, ps.length] as [string, number]).sort((a, b) => b[1] - a[1])[0]
  return { branch, merged: count, of: merged.length }
}

export function gaps(f: Findings): Gap[] {
  const out: Gap[] = []
  for (const m of f.mainlines) {
    const into = mergedInto(f, m.repo)
    if (into && short(m.mainline) !== into.branch)
      out.push({
        title: `${m.repo}: the crawler reads ${short(m.mainline)} as its main line, but ${n(into.merged)} of ${n(into.of)} merged pull requests went into ${into.branch}`,
        detail: `Done is merged: until the main line is ${into.branch}, work merged there counts as never landed.`,
        set: { repo: m.repo, ref: `origin/${into.branch}` },
      })
  }
  const credited = f.commits.filter((c) => c.model && !c.bulk && c.fate !== 'snapshot')
  const ok = new Set(judged(f))
  const byRepo = group(credited, (c) => c.repo)
  for (const [repo, cs] of byRepo) {
    if (!ok.has(repo))
      out.push({
        title: `${repo}: none of its ${n(cs.length)} credited commits counts as landed`,
        detail: 'Its main line, as recon reads it, took none of them, so nothing there can be judged.',
      })
  }
  for (const [repo, folders] of group(f.folders, (x) => x.repo))
    out.push({
      title: `${folders.map((x) => x.folder).join(', ')} ${folders.length === 1 ? 'is' : 'are'} read as ${repo}`,
      detail: `Each is a folder whose remote names ${repo}'s host repository (a clone or a build folder): the work done there is ${repo}'s, and a commit both hold counts once. Work only such a folder holds is shown as a side folder's and left out of the rates.`,
    })
  const snapshots = f.commits.filter((c) => c.fate === 'snapshot')
  if (snapshots.length) {
    const stash = snapshots.filter((c) => /^(WIP on |index on |untracked files on |On [^:]+: )/.test(c.subject)).length
    out.push({
      title: `${n(snapshots.length)} snapshot commits count nowhere`,
      detail: `${n(stash)} are git's own stash commits; ${n(snapshots.length - stash)} say in their subject they are savepoints (attr_recon_snapshot_words), ${n(sum(snapshots, (c) => c.added))} lines in all.`,
    })
  }
  const withCi = new Set(f.workflows.map((w) => w.repo))
  for (const repo of byRepo.keys())
    if (!withCi.has(repo)) out.push({ title: `${repo}: no CI run held`, detail: 'Nothing on its host judges its code, or the crawler holds no host for it.' })
  for (const repo of [...withCi]) {
    const ws = f.workflows.filter((w) => w.repo === repo)
    const names = [...new Set(ws.map((w) => w.name))]
    if (names.length <= 2)
      out.push({
        title: `${repo}: CI runs ${names.length === 1 ? 'one workflow' : `${names.length} workflows`}, ${names.join(' and ')}`,
        detail: `${n(sum(ws, (w) => w.runs))} runs, ${n(sum(ws, (w) => w.failed))} failed. Whether it runs the tests decides whether its verdicts say anything about the code.`,
      })
  }
  const uncredited = group(f.commits.filter((c) => !c.model && !c.bulk && c.fate !== 'snapshot'), (c) => c.repo)
  for (const [repo, cs] of uncredited)
    if (byRepo.has(repo))
      out.push({
        title: `${repo}: ${n(cs.length)} commits of yours no session's work covers`,
        detail: 'Made outside a captured session, before capture began, or while two models worked there at once; no model is credited with them.',
      })
  const nowhere = f.weeks.filter((w) => w.repo === '')
  if (nowhere.length)
    out.push({
      title: `${n(sum(nowhere, (w) => w.replies))} replies were made in no repository`,
      detail: `${n(sum(nowhere, (w) => w.active_secs) / 3600)} active hours whose sessions worked outside every repository recon holds.`,
    })
  return out
}

/// The parameter's pairs as the page shows them: `repo → ref, …`, or none.
function pairs(shown: string | undefined): Record<string, string> {
  const out: Record<string, string> = {}
  if (!shown || shown === 'none') return out
  for (const pair of shown.split(', ')) {
    const [repo, ref] = pair.split(' → ')
    if (repo && ref) out[repo] = ref
  }
  return out
}

function SetMainline({ repo, refName }: { repo: string; refName: string }) {
  const client = useQueryClient()
  const params = useQuery({ queryKey: ['params'], queryFn: fetchParams })
  const change = useMutation({
    mutationFn: () => {
      const now = pairs(params.data?.params.find((p) => p.name === MAINLINE)?.value)
      return setParam(MAINLINE, { ...now, [repo]: refName })
    },
    onSuccess: (listing) => {
      client.setQueryData(['params'], listing)
      client.invalidateQueries({ queryKey: ['findings'] })
    },
  })
  return (
    <Group gap="xs">
      <Button size="compact-xs" loading={change.isPending} disabled={!params.data || change.isSuccess} onClick={() => change.mutate()}>
        Set {repo}'s main line to {refName}
      </Button>
      {change.isSuccess && <Text fz="xs" c="teal.4">Set: the next crawl of {repo} reads it.</Text>}
      {change.isError && <Text fz="xs" c="red.4">{String(change.error.message)}</Text>}
    </Group>
  )
}

export function Gaps({ f }: { f: Findings }) {
  const list = gaps(f)
  return (
    <Stack gap="md">
      <Paper withBorder radius="md" p="md">
        <Stack gap="xs">
          <Title order={4}>Where each repository's work is done</Title>
          <Text fz="sm" c="dimmed">Done is merged into the main line. The crawler's main line for each repository, set against where its merged pull requests went.</Text>
          <List spacing={4} size="sm">
            {f.mainlines.map((m) => {
              const into = mergedInto(f, m.repo)
              const agrees = !into || short(m.mainline) === into.branch
              return (
                <List.Item key={m.repo}>
                  <Text fz="sm" c={agrees ? undefined : 'yellow.5'}>
                    {m.repo}: {short(m.mainline)} ({m.from.replace('_', ' ')}){into ? ` · ${n(into.merged)} of ${n(into.of)} merged pull requests went into ${into.branch}` : ' · no merged pull requests to check it against'}
                  </Text>
                </List.Item>
              )
            })}
          </List>
        </Stack>
      </Paper>
      <Paper withBorder radius="md" p="md">
        <Stack gap="sm">
          <Title order={4}>{list.length ? `${list.length} things limit what recon can judge` : 'Nothing limits what recon can judge'}</Title>
          {list.length === 0 && <Alert color="teal">Every repository's work can be judged.</Alert>}
          <List spacing="sm">
            {list.map((g) => (
              <List.Item key={g.title}>
                <Text fw={600} fz="sm">{g.title}</Text>
                <Text fz="sm" c="dimmed">{g.detail}</Text>
                {g.set && <SetMainline repo={g.set.repo} refName={g.set.ref} />}
              </List.Item>
            ))}
          </List>
        </Stack>
      </Paper>
    </Stack>
  )
}
