import { useEffect, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  ActionIcon,
  Alert,
  Anchor,
  Badge,
  Group,
  Loader,
  mergeAsyncChildren,
  Switch,
  Text,
  TextInput,
  Tree,
  useTree,
  type TreeNodeData,
} from '@mantine/core'
import {
  createEntity,
  fetchChildren,
  fetchRoots,
  searchEntities,
  type TreeNodeView,
} from '../api'

// THE MENU: add entities, and traverse them.
//
// The data is a GRAPH, not a tree — an entity can have many parents and
// the graph can contain cycles — so there is no global hierarchy to
// draw. What there is, is a view ROOTED WHERE YOU ARE: expand a row and
// it asks for exactly one level. Depth stops mattering because the whole
// thing is never requested, and a cycle is just a row you can open
// again.
//
// Mantine's Tree does the lazy half natively: `hasChildren` with no
// children triggers `onLoadChildren` on first expand.

/// A node's key is its PATH, not its uuid.
///
/// The data is a graph: an entity can hang under two parents at once,
/// and both rows would then share one key — one arrow would open both,
/// `mergeAsyncChildren` would splice the fetched level into whichever it
/// found first, and React would see duplicate keys among siblings. The
/// uuid rides in `nodeProps` for opening the entity; the path is what
/// makes a DAG renderable as a tree.
///
/// `hasChildren` with NO `children` key is what marks a node as lazy —
/// an empty `children` array means "expanded, and there is nothing
/// here", so `onLoadChildren` never fires.
/// THE PATH IS ALREADY IN THE TREE. A node's `value` is the chain of
/// uuids that reached it, so the breadcrumb trail is that chain with
/// each step's label looked up — nothing has to be fetched, and the
/// trail always agrees with the branch that is actually open.
export function trailFrom(
  data: TreeNodeData[],
  path: string,
): { uuid: string; name: string }[] {
  const steps = path.split('/')
  const out: { uuid: string; name: string }[] = []
  let level = data
  let key = ''
  for (const uuid of steps) {
    key = key ? `${key}/${uuid}` : uuid
    const node = level.find((n) => n.value === key)
    if (!node) break
    out.push({ uuid, name: String(node.label) })
    level = node.children ?? []
  }
  return out
}

function toNode(v: TreeNodeView, parentKey = ''): TreeNodeData {
  return {
    value: parentKey ? `${parentKey}/${v.uuid}` : v.uuid,
    label: v.name || '(unnamed)',
    nodeProps: { labels: v.labels, via: v.via, uuid: v.uuid },
    hasChildren: v.has_children,
  }
}

export default function MenuTree({
  onOpen,
  opened,
}: {
  onOpen: (uuid: string, trail: { uuid: string; name: string }[]) => void
  opened: string | null
}) {
  const qc = useQueryClient()
  const [archived, setArchived] = useState(false)
  const [name, setName] = useState('')

  const roots = useQuery({
    queryKey: ['roots', archived],
    queryFn: () => fetchRoots(archived),
  })

  // SEARCH REACHES PAST THE TREE. The tree loads one level at a time, so
  // filtering what is already on screen would only ever find what the
  // operator had already navigated to — which is not what a search box
  // is for. The match runs server-side over every entity and comes back
  // flat, because a hit was not reached through a parent.
  const [term, setTerm] = useState('')
  const searching = term.trim().length > 0
  const hits = useQuery({
    queryKey: ['search', term.trim(), archived],
    queryFn: () => searchEntities(term.trim(), archived),
    enabled: searching,
  })

  // Mantine hands back the node that was expanded and expects the data
  // to be spliced in — `mergeAsyncChildren` does the splice, and the one
  // query it needs is the level below that node.
  //
  // MERGE, NEVER REPLACE. Adding an entity invalidates `roots`, and a
  // window-focus refetch does the same with no user action at all —
  // replacing `data` threw away every expanded subtree while `useTree`
  // still believed those nodes were open. Because it only calls
  // `onLoadChildren` on the FIRST expand, the branches rendered empty
  // and could not be reloaded without collapsing each one by hand. So a
  // refetch adds and removes roots and leaves loaded children alone.
  const [data, setData] = useState<TreeNodeData[]>([])
  useEffect(() => {
    const fresh = (roots.data ?? []).map((n) => toNode(n))
    setData((current) => {
      const held = new Map(current.map((n) => [n.value, n]))
      return fresh.map((n) => {
        const already = held.get(n.value)
        return already?.children ? { ...n, children: already.children } : n
      })
    })
  }, [roots.data])

  const tree = useTree({
    onLoadChildren: async (value) => {
      // The key is a path; the entity to ask about is its last segment.
      const uuid = value.split('/').pop() ?? value
      const kids = await fetchChildren(uuid)
      setData((current) => mergeAsyncChildren(current, value, kids.map((k) => toNode(k, value))))
    },
  })

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

  return (
    <>
      {/* THE TREE LIVES IN THE SIDEBAR, so the controls stack instead
          of spreading: a row of label-beside-field made sense across a
          page and does not in a 300px column. */}
      <TextInput
        placeholder="New entity"
        value={name}
        onChange={(e) => setName(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && name.trim()) add.mutate(name.trim())
        }}
        rightSection={
          <ActionIcon
            variant="subtle"
            onClick={() => name.trim() && add.mutate(name.trim())}
            loading={add.isPending}
            disabled={!name.trim()}
            aria-label="Add entity"
          >
            +
          </ActionIcon>
        }
        mb="xs"
      />
      {add.error && (
        <Text c="red" size="xs" mb="xs">
          {String(add.error)}
        </Text>
      )}
      <TextInput
        placeholder="Search entities"
        value={term}
        onChange={(e) => setTerm(e.currentTarget.value)}
        rightSection={
          term ? (
            <ActionIcon variant="subtle" onClick={() => setTerm('')} aria-label="Clear search">
              ×
            </ActionIcon>
          ) : null
        }
        mb="xs"
      />
      <Switch
        label="show archived"
        size="xs"
        checked={archived}
        onChange={(e) => setArchived(e.currentTarget.checked)}
        mb="sm"
      />

      {/* A HIT IS NOT A BRANCH. Search results are shown as a flat
          list, not spliced into the tree, because they were not reached
          through a parent and pretending otherwise would put an entity
          somewhere it does not live. */}
      {searching && (
        <>
          {hits.isLoading && <Loader size="sm" />}
          {hits.data?.length === 0 && (
            <Text c="dimmed" size="xs" px={6}>
              Nothing matches “{term.trim()}”.
            </Text>
          )}
          {(hits.data ?? []).map((h) => (
            <Group key={h.uuid} gap="xs" wrap="nowrap" py={2} px={6}>
              <Anchor
                component="button"
                type="button"
                size="sm"
                underline="hover"
                fw={h.uuid === opened ? 700 : undefined}
                onClick={() => onOpen(h.uuid, [{ uuid: h.uuid, name: h.name }])}
              >
                {h.name || '(unnamed)'}
              </Anchor>
              {h.labels.map((l) => (
                <Badge key={l.uuid} size="xs" variant="light">
                  {l.name}
                </Badge>
              ))}
            </Group>
          ))}
        </>
      )}

      {!searching && roots.isLoading && <Loader size="sm" />}
        {/* SAY WHAT WENT WRONG. This rendered an empty box when the read
            failed, so an unprovisioned instance looked like a blank page
            with no explanation — the reader cannot tell "nothing here"
            from "this is broken". */}
        {!searching && roots.error && (
          <Alert color="red" title="Cannot read entities">
            <Text size="sm">{String(roots.error)}</Text>
            <Text size="xs" c="dimmed" mt="xs">
              If this says a table does not exist, the module's database has not been
              provisioned into this shape yet: retire the old one with
              run superx modules provision entities.
            </Text>
          </Alert>
        )}
        {!searching && !roots.error && roots.data?.length === 0 && (
          <Text c="dimmed" size="sm">
            Nothing yet. Add an entity above.
          </Text>
        )}
        {!searching && (
        <Tree
          data={data}
          tree={tree}
          levelOffset={22}
          renderNode={({ node, expanded, hasChildren, elementProps, tree: t }) => {
            const props = (node.nodeProps ?? {}) as {
              labels?: { uuid: string; name: string }[]
              via?: string | null
              uuid?: string
            }
            return (
              <Group gap="xs" {...elementProps} wrap="nowrap" py={2}>
                <Text
                  size="sm"
                  c="dimmed"
                  w={14}
                  style={{ cursor: hasChildren ? 'pointer' : 'default' }}
                  onClick={(e) => {
                    e.stopPropagation()
                    if (hasChildren) t.toggleExpanded(node.value)
                  }}
                >
                  {hasChildren ? (expanded ? '▾' : '▸') : ''}
                </Text>
                {props.via && (
                  <Text size="xs" c="dimmed" ff="monospace">
                    {props.via} →
                  </Text>
                )}
                {/* A LINK, NOT A CLICKABLE WORD. Opening an entity was
                    already wired to this text, but nothing said so —
                    plain body copy with a cursor is not an affordance,
                    and the click ALSO bubbled to the row, so the one
                    guess an operator might make toggled the branch
                    instead. An anchor looks like what it is, takes
                    keyboard focus, and keeps the row's expander to
                    itself. */}
                <Anchor
                  component="button"
                  type="button"
                  size="sm"
                  underline="hover"
                  // WHICH ONE AM I LOOKING AT. The tree stays put while
                  // you work in the pane beside it, so without this the
                  // sidebar gives no clue which branch the open entity
                  // is.
                  fw={props.uuid === opened ? 700 : undefined}
                  c={props.uuid === opened ? undefined : 'var(--mantine-color-text)'}
                  onClick={(ev) => {
                    ev.stopPropagation()
                    onOpen(props.uuid ?? node.value, trailFrom(data, node.value))
                  }}
                >
                  {node.label}
                </Anchor>
                {(props.labels ?? []).map((l) => (
                  <Badge key={l.uuid} size="xs" variant="light">
                    {l.name}
                  </Badge>
                ))}
              </Group>
            )
          }}
        />
        )}
    </>
  )
}
