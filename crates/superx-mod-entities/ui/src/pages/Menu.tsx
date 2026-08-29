import { useCallback, useEffect, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  ActionIcon,
  Alert,
  Badge,
  Box,
  Card,
  Group,
  Loader,
  NavLink,
  Switch,
  Text,
  TextInput,
} from '@mantine/core'
import { createEntity, fetchChildren, fetchRoots, searchEntities, type TreeNodeView } from '../api'

// THE MENU TAB: add entities, search them, and walk the graph.
//
// The data is a GRAPH, not a tree — an entity can have many parents and
// the graph can contain cycles — so there is no global hierarchy to
// draw. What there is, is a view ROOTED WHERE YOU ARE: open a row and it
// asks for exactly one level. Depth stops mattering because the whole
// thing is never requested, and a cycle is just a row you can open
// again.
//
// IT IS BUILT ON NavLink, the same menu component the core dashboard's
// navigation uses, nested. The previous cut rendered rows by hand
// through Mantine's Tree, which draws bare text: no hover, no active
// state, nothing that looks like a control. A menu should look like the
// product's menu.

/// A row's key is its PATH, not its uuid.
///
/// The data is a graph: an entity can hang under two parents at once and
/// both rows would then share one key — opening one would open the
/// other, and the fetched level would be spliced into whichever was
/// found first. The uuid rides alongside for opening the entity; the
/// path is what makes a DAG renderable as a menu.
type Node = {
  path: string
  uuid: string
  name: string
  via: string | null
  labels: { uuid: string; name: string }[]
  hasChildren: boolean
  children?: Node[]
}

const toNode = (v: TreeNodeView, parent = ''): Node => ({
  path: parent ? `${parent}/${v.uuid}` : v.uuid,
  uuid: v.uuid,
  name: v.name || '(unnamed)',
  via: v.via,
  labels: v.labels,
  hasChildren: v.has_children,
})

/// The trail to a row, read off the path it already carries — nothing is
/// fetched, so the breadcrumbs cannot disagree with the branch that is
/// open.
function trailTo(nodes: Node[], path: string): { uuid: string; name: string }[] {
  const out: { uuid: string; name: string }[] = []
  let level = nodes
  let key = ''
  for (const uuid of path.split('/')) {
    key = key ? `${key}/${uuid}` : uuid
    const found = level.find((n) => n.path === key)
    if (!found) break
    out.push({ uuid: found.uuid, name: found.name })
    level = found.children ?? []
  }
  return out
}

/// Splice a fetched level in, leaving every other branch alone.
function withChildren(nodes: Node[], path: string, kids: Node[]): Node[] {
  return nodes.map((n) => {
    if (n.path === path) return { ...n, children: kids }
    if (n.children && path.startsWith(`${n.path}/`)) {
      return { ...n, children: withChildren(n.children, path, kids) }
    }
    return n
  })
}

export default function MenuTab({
  onOpen,
  opened,
}: {
  onOpen: (uuid: string, trail: { uuid: string; name: string }[]) => void
  opened: string | null
}) {
  const qc = useQueryClient()
  const [archived, setArchived] = useState(false)
  const [name, setName] = useState('')
  const [term, setTerm] = useState('')
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [nodes, setNodes] = useState<Node[]>([])

  const roots = useQuery({ queryKey: ['roots', archived], queryFn: () => fetchRoots(archived) })

  // SEARCH REACHES PAST THE MENU. It loads one level at a time, so
  // filtering what is on screen would only ever find what you had
  // already navigated to — the opposite of what a search box is for. The
  // match runs server-side over every entity and comes back flat,
  // because a hit was not reached through a parent.
  const searching = term.trim().length > 0
  const hits = useQuery({
    queryKey: ['search', term.trim(), archived],
    queryFn: () => searchEntities(term.trim(), archived),
    enabled: searching,
  })

  // MERGE, NEVER REPLACE. Adding an entity invalidates `roots`, and a
  // window-focus refetch does the same with no user action at all —
  // replacing the list threw away every branch that was open.
  useEffect(() => {
    const fresh = (roots.data ?? []).map((n) => toNode(n))
    setNodes((current) => {
      const held = new Map(current.map((n) => [n.path, n]))
      return fresh.map((n) => {
        const already = held.get(n.path)
        return already?.children ? { ...n, children: already.children } : n
      })
    })
  }, [roots.data])

  const toggle = useCallback(
    async (node: Node) => {
      const next = new Set(expanded)
      if (next.has(node.path)) {
        next.delete(node.path)
        setExpanded(next)
        return
      }
      next.add(node.path)
      setExpanded(next)
      // One level, on first open. Asking again on every re-open would
      // re-fetch a branch that has not changed.
      if (!node.children) {
        const kids = await fetchChildren(node.uuid)
        setNodes((cur) => withChildren(cur, node.path, kids.map((k) => toNode(k, node.path))))
      }
    },
    [expanded],
  )

  const add = useMutation({
    mutationFn: (n: string) => createEntity(n),
    onSuccess: () => {
      setName('')
      void qc.invalidateQueries({ queryKey: ['roots'] })
      // The pickers list every entity, so a new one belongs in them
      // immediately — otherwise you create a label and cannot use it
      // until the page is reloaded.
      void qc.invalidateQueries({ queryKey: ['all-entities'] })
    },
  })

  // A ROW IS A NavLink; THE BRANCH UNDER IT IS OURS.
  //
  // NavLink will collapse its own children, and that collapse would not
  // open: it measures the content it is given, and the content arrives
  // one fetch later — so the chevron flipped and nothing appeared.
  // Rendering the branch here instead means what is on screen is
  // exactly what the expanded set says, with no animation state in
  // between to disagree with it.
  const row = (n: Node) => (
    <Box key={n.path}>
      <NavLink
        active={n.uuid === opened}
        // THE CHEVRON IS ITS OWN TARGET. Opening an entity switches to
        // the Entity tab; expanding a branch must not. Two things a row
        // can do, two places to click, each looking like what it does.
        leftSection={
          n.hasChildren ? (
            <ActionIcon
              component="div"
              variant="subtle"
              size="sm"
              aria-label={expanded.has(n.path) ? 'Collapse' : 'Expand'}
              onClick={(ev) => {
                ev.stopPropagation()
                void toggle(n)
              }}
            >
              <Text size="xs" c="dimmed">
                {expanded.has(n.path) ? '▾' : '▸'}
              </Text>
            </ActionIcon>
          ) : (
            <Box w={22} />
          )
        }
        onClick={() => onOpen(n.uuid, trailTo(nodes, n.path))}
        label={
          <Group gap={7} wrap="nowrap">
            {n.via && (
              <Text size="xs" c="dimmed" ff="monospace" style={{ whiteSpace: 'nowrap' }}>
                {n.via} →
              </Text>
            )}
            <Text size="sm" truncate>
              {n.name}
            </Text>
            {n.labels.map((l) => (
              <Badge key={l.uuid} size="xs" variant="light">
                {l.name}
              </Badge>
            ))}
          </Group>
        }
      />
      {expanded.has(n.path) && (
        <Box pl={20}>
          {n.children ? n.children.map(row) : <Loader size="xs" ml="sm" my={6} />}
        </Box>
      )}
    </Box>
  )

  return (
    <>
      <Card withBorder padding="md" mb="md">
        <Group align="flex-end" gap="sm" wrap="wrap">
          <TextInput
            label="New entity"
            description="a name is all it needs — everything else is an attribute"
            placeholder="DBA"
            value={name}
            onChange={(e) => setName(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && name.trim()) add.mutate(name.trim())
            }}
            style={{ flex: 1, minWidth: 220 }}
          />
          {/* A REAL BUTTON, BESIDE THE FIELD. It used to sit inside the
              input as a right section, where Mantine sets
              `pointer-events: none` by default — it looked live and
              swallowed every click, and nothing caught it because the
              tests posted to the endpoint and dispatched synthetic
              clicks straight at the element, both of which walk past the
              CSS that was the whole problem. */}
          <ActionIcon
            size={36}
            variant="filled"
            onClick={() => name.trim() && add.mutate(name.trim())}
            loading={add.isPending}
            disabled={!name.trim()}
            aria-label="Add entity"
          >
            +
          </ActionIcon>
          <TextInput
            label="Search"
            description="every entity, not just the ones on screen"
            placeholder="jira"
            value={term}
            onChange={(e) => setTerm(e.currentTarget.value)}
            style={{ flex: 1, minWidth: 200 }}
          />
          <Switch
            label="show archived"
            checked={archived}
            onChange={(e) => setArchived(e.currentTarget.checked)}
            mb={8}
          />
        </Group>
        {add.error && (
          <Text c="red" size="sm" mt="xs">
            {String(add.error)}
          </Text>
        )}
      </Card>

      <Card withBorder padding="xs">
        {/* SAY WHAT WENT WRONG. This rendered an empty box when the read
            failed, so an unprovisioned instance looked like a page that
            had not loaded rather than one with something to report. */}
        {roots.error && (
          <Alert color="red" title="Cannot read entities" m="xs">
            <Text size="sm">{String(roots.error)}</Text>
            <Text size="xs" c="dimmed" mt="xs">
              If this says a table does not exist, this module's database has not been
              provisioned yet: run superx modules provision entities.
            </Text>
          </Alert>
        )}

        {searching ? (
          <Box>
            {hits.isLoading && <Loader size="sm" m="sm" />}
            {hits.data?.length === 0 && (
              <Text c="dimmed" size="sm" m="sm">
                Nothing matches “{term.trim()}”.
              </Text>
            )}
            {/* A HIT IS NOT A BRANCH. Results are flat because they were
                not reached through a parent, and splicing them into the
                menu would put an entity somewhere it does not live. */}
            {(hits.data ?? []).map((h) => (
              <NavLink
                key={h.uuid}
                active={h.uuid === opened}
                onClick={() => onOpen(h.uuid, [{ uuid: h.uuid, name: h.name }])}
                label={
                  <Group gap={7} wrap="nowrap">
                    <Text size="sm">{h.name || '(unnamed)'}</Text>
                    {h.labels.map((l) => (
                      <Badge key={l.uuid} size="xs" variant="light">
                        {l.name}
                      </Badge>
                    ))}
                  </Group>
                }
              />
            ))}
          </Box>
        ) : (
          <Box>
            {roots.isLoading && <Loader size="sm" m="sm" />}
            {!roots.error && roots.data?.length === 0 && (
              <Text c="dimmed" size="sm" m="sm">
                Nothing yet. Add an entity above.
              </Text>
            )}
            {nodes.map(row)}
          </Box>
        )}
      </Card>
    </>
  )
}
