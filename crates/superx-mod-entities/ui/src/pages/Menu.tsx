import { useCallback, useEffect, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  ActionIcon,
  Alert,
  Anchor,
  Badge,
  Box,
  Breadcrumbs,
  Button,
  Card,
  Group,
  Loader,
  NavLink,
  Popover,
  Stack,
  Switch,
  Text,
  TextInput,
} from '@mantine/core'
import {
  IS,
  LABEL,
  createEntity,
  fetchAllEntities,
  fetchChildren,
  fetchRoots,
  putAttribute,
  searchEntities,
  type TreeNodeView,
} from '../api'

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

/// What a brand-new entity is called until you rename it. It has to be
/// SOMETHING: the name is an attribute, a listing shows it, and a blank
/// row is unreadable. Renaming it is the first field on the tab that
/// opens.
const NEW_NAME = 'Untitled' // skill-allow: §9-const — the module's own vocabulary, not a tunable

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
  trail,
  onCrumb,
}: {
  onOpen: (uuid: string, trail: { uuid: string; name: string }[]) => void
  opened: string | null
  trail: { uuid: string; name: string }[]
  onCrumb: (uuid: string, upto: number) => void
}) {
  const qc = useQueryClient()
  const [archived, setArchived] = useState(false)
  const [term, setTerm] = useState('')
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [nodes, setNodes] = useState<Node[]>([])
  const [naming, setNaming] = useState(false)
  const [labelName, setLabelName] = useState('')

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
    mutationFn: () => createEntity(NEW_NAME),
    // CREATING OPENS IT. There is nowhere else to go afterwards: a new
    // entity is a uuid and a placeholder name, and everything you would
    // do next — name it, label it, give it fields — is on the Entity
    // tab. Leaving you on the menu staring at a row called "Untitled"
    // just adds a click.
    onSuccess: (created) => {
      onOpen(created.uuid, [{ uuid: created.uuid, name: NEW_NAME }])
      void qc.invalidateQueries({ queryKey: ['roots'] })
      // The pickers list every entity, so a new one belongs in them
      // immediately — otherwise you create a label and cannot use it
      // until the page is reloaded.
      void qc.invalidateQueries({ queryKey: ['all-entities'] })
    },
  })

  // A LABEL IS AN ENTITY CARRYING `label` — and `label` is itself one,
  // carrying itself. Nothing seeds it: a fresh instance has no
  // vocabulary and, because a picker leaves out the entity itself, no
  // way to mark the first word as a label. The live instance sat at
  // four rows called Untitled for exactly this reason. So the first New
  // label bootstraps the word `label`; every one after finds it there.
  const newLabel = useMutation({
    mutationFn: async (name: string) => {
      // Archived included: a put-away `label` is still the word, and a
      // second one would split the vocabulary in two.
      const every = await fetchAllEntities(true)
      let meta = every.find((e) => e.name === LABEL)?.uuid
      if (!meta) {
        meta = (await createEntity(LABEL)).uuid
        await putAttribute(meta, { name: IS, datatype: 'text', content: null, labels: [meta] })
      }
      if (name.toLowerCase() === LABEL) return { uuid: meta, name: LABEL }
      const made = await createEntity(name)
      await putAttribute(made.uuid, { name: IS, datatype: 'text', content: null, labels: [meta] })
      return { uuid: made.uuid, name }
    },
    onSuccess: (made) => {
      setNaming(false)
      setLabelName('')
      onOpen(made.uuid, [{ uuid: made.uuid, name: made.name }])
      void qc.invalidateQueries({ queryKey: ['roots'] })
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
      {/* A COLUMN. A list of entities is read down, not across — the
          controls that drive it belong at its own width, not spread to
          the far edge of a monitor. Search sits directly above the list
          it filters, which is the only place it means anything. */}
      <Stack gap="xs" maw={640}>
        <Group gap="sm" wrap="nowrap">
          <Button onClick={() => add.mutate()} loading={add.isPending} leftSection="+" size="sm">
            New entity
          </Button>
          <Popover
            opened={naming}
            onChange={setNaming}
            trapFocus
            withArrow
            position="bottom-start"
            width={300}
          >
            <Popover.Target>
              <Button variant="light" size="sm" onClick={() => setNaming((o) => !o)}>
                New label
              </Button>
            </Popover.Target>
            <Popover.Dropdown>
              <TextInput
                data-autofocus
                label="New label"
                description="what a thing IS — role, task, credential, resource"
                placeholder="role"
                value={labelName}
                onChange={(e) => setLabelName(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && labelName.trim()) newLabel.mutate(labelName.trim())
                }}
              />
              <Group justify="flex-end" mt="xs">
                <Button
                  size="xs"
                  loading={newLabel.isPending}
                  disabled={!labelName.trim()}
                  onClick={() => newLabel.mutate(labelName.trim())}
                >
                  Create
                </Button>
              </Group>
              {newLabel.error && (
                <Text c="red" size="xs" mt="xs">
                  {String(newLabel.error)}
                </Text>
              )}
            </Popover.Dropdown>
          </Popover>
          <Box style={{ flex: 1, minWidth: 0 }}>
            <Breadcrumbs
              separator="›"
              separatorMargin={6}
              styles={{
                root: { flexWrap: 'nowrap', overflow: 'hidden' },
                separator: { color: 'var(--mantine-color-dimmed)' },
              }}
            >
              {trail.map((c, i) =>
                i === trail.length - 1 ? (
                  <Text key={c.uuid} size="sm" fw={600} truncate>
                    {c.name}
                  </Text>
                ) : (
                  <Anchor key={c.uuid} size="sm" c="dimmed" onClick={() => onCrumb(c.uuid, i)}>
                    {c.name}
                  </Anchor>
                ),
              )}
            </Breadcrumbs>
          </Box>
          <Switch
            label="archived"
            size="xs"
            checked={archived}
            onChange={(e) => setArchived(e.currentTarget.checked)}
          />
        </Group>

        {add.error && (
          <Text c="red" size="sm">
            {String(add.error)}
          </Text>
        )}

        <TextInput
          placeholder="Search every entity"
          value={term}
          onChange={(e) => setTerm(e.currentTarget.value)}
          size="sm"
        />

        <Card withBorder padding={4}>
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
      </Stack>
    </>
  )
}
