import { List, Paper, Stack, Text, Title } from '@mantine/core'
import type { Findings } from '../api'
import { judged } from './scope'
import { group, n, sum } from './shared'

// What recon cannot judge yet, read from the facts: repositories whose main
// line took none of the credited work, folders that hold another
// repository's commits again, repositories no CI judges, commits no
// session's work covers, and work done in no repository.

export type Gap = { title: string; detail: string }

export function gaps(f: Findings): Gap[] {
  const out: Gap[] = []
  const credited = f.commits.filter((c) => c.model && !c.bulk)
  const ok = new Set(judged(f))
  const byRepo = group(credited, (c) => c.repo)
  for (const [repo, cs] of byRepo) {
    if (!ok.has(repo))
      out.push({
        title: `${repo}: none of its ${n(cs.length)} credited commits counts as landed`,
        detail: 'Its main line, as recon reads it, took none of them, so nothing there can be judged. If it lands work on another branch, attr_recon_mainline_refs names it.',
      })
  }
  const hashes = group(f.commits, (c) => c.hash)
  const shared = new Map<string, Map<string, number>>()
  for (const cs of hashes.values()) {
    const repos = [...new Set(cs.map((c) => c.repo))]
    if (repos.length < 2) continue
    for (const a of repos) for (const b of repos) if (a !== b) shared.set(a, (shared.get(a) ?? new Map()).set(b, (shared.get(a)?.get(b) ?? 0) + 1))
  }
  const ours = group(f.commits, (c) => c.repo)
  for (const [repo, others] of shared) {
    const [other, count] = [...others.entries()].sort((a, b) => b[1] - a[1])[0]
    const total = ours.get(repo)?.length ?? 0
    if (count * 2 >= total)
      out.push({ title: `${repo} holds ${n(count)} of the same commits as ${other}`, detail: `${n(count)} of its ${n(total)} commits are also in ${other}: a clone of it, so its commits are counted twice.` })
  }
  const withCi = new Set(f.workflows.map((w) => w.repo))
  for (const repo of byRepo.keys())
    if (!withCi.has(repo)) out.push({ title: `${repo}: no CI run held`, detail: 'Nothing on its host judges its code, or the crawler holds no host for it.' })
  for (const repo of [...withCi]) {
    const ws = f.workflows.filter((w) => w.repo === repo)
    const names = [...new Set(ws.map((w) => w.name))]
    if (names.length <= 2) out.push({ title: `${repo}: CI runs ${names.length === 1 ? 'one workflow' : `${names.length} workflows`}, ${names.join(' and ')}`, detail: `${n(sum(ws, (w) => w.runs))} runs, ${n(sum(ws, (w) => w.failed))} failed. Whether it runs the tests decides whether its verdicts say anything about the code.` })
  }
  const uncredited = group(f.commits.filter((c) => !c.model && !c.bulk), (c) => c.repo)
  for (const [repo, cs] of uncredited)
    if (byRepo.has(repo)) out.push({ title: `${repo}: ${n(cs.length)} commits of yours no session's work covers`, detail: 'Made outside a captured session, before capture began, or while two models worked there at once; no model is credited with them.' })
  const nowhere = f.weeks.filter((w) => w.repo === '')
  if (nowhere.length)
    out.push({ title: `${n(sum(nowhere, (w) => w.replies))} replies were made in no repository`, detail: `${n(sum(nowhere, (w) => w.active_secs) / 3600)} active hours whose sessions worked outside every repository recon holds; their commits, if any, are not credited.` })
  return out
}

export function Gaps({ f }: { f: Findings }) {
  const list = gaps(f)
  return (
    <Paper withBorder radius="md" p="md">
      <Stack gap="sm">
        <Title order={4}>{list.length ? `${list.length} things limit what recon can judge` : 'Nothing limits what recon can judge'}</Title>
        <List spacing="sm">
          {list.map((g) => (
            <List.Item key={g.title}>
              <Text fw={600} fz="sm">{g.title}</Text>
              <Text fz="sm" c="dimmed">{g.detail}</Text>
            </List.Item>
          ))}
        </List>
      </Stack>
    </Paper>
  )
}
