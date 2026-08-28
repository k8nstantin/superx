import { useEffect, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Badge,
  Button,
  Card,
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
import { createEntity, fetchChildren, fetchRoots, type TreeNodeView } from '../api'

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

function toNode(v: TreeNodeView): TreeNodeData {
  return {
    value: v.uuid,
    label: v.name || '(unnamed)',
    nodeProps: { labels: v.labels, via: v.via },
    children: v.has_children ? [] : undefined,
    // Mantine reads this to decide whether the row opens at all.
    ...(v.has_children ? { hasChildren: true } : {}),
  } as TreeNodeData
}

export default function MenuTab({ onOpen }: { onOpen: (uuid: string) => void }) {
  const qc = useQueryClient()
  const [archived, setArchived] = useState(false)
  const [name, setName] = useState('')

  const roots = useQuery({
    queryKey: ['roots', archived],
    queryFn: () => fetchRoots(archived),
  })

  // Mantine hands back the node that was expanded and expects the data
  // to be spliced in — `mergeAsyncChildren` does the splice, and the
  // one query it needs is the one level below that node.
  const [data, setData] = useState<TreeNodeData[]>([])
  useEffect(() => {
    setData((roots.data ?? []).map(toNode))
  }, [roots.data])

  const tree = useTree({
    onLoadChildren: async (value) => {
      const kids = await fetchChildren(value)
      setData((current) => mergeAsyncChildren(current, value, kids.map(toNode)))
    },
  })

  const add = useMutation({
    mutationFn: (n: string) => createEntity(n),
    onSuccess: () => {
      setName('')
      void qc.invalidateQueries({ queryKey: ['roots'] })
    },
  })

  return (
    <>
      <Card withBorder padding="md" mb="md">
        <Group align="flex-end" gap="sm">
          <TextInput
            label="New entity"
            description="a name is all it needs — everything else is an attribute"
            placeholder="DBA"
            value={name}
            onChange={(e) => setName(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && name.trim()) add.mutate(name.trim())
            }}
            style={{ flex: 1 }}
          />
          <Button
            onClick={() => name.trim() && add.mutate(name.trim())}
            loading={add.isPending}
            disabled={!name.trim()}
          >
            Add
          </Button>
          <Switch
            label="show archived"
            checked={archived}
            onChange={(e) => setArchived(e.currentTarget.checked)}
            mb={6}
          />
        </Group>
        {add.error && (
          <Text c="red" size="sm" mt="xs">
            {String(add.error)}
          </Text>
        )}
      </Card>

      <Card withBorder padding="md">
        {roots.isLoading && <Loader size="sm" />}
        {roots.data?.length === 0 && (
          <Text c="dimmed" size="sm">
            Nothing yet. Add an entity above.
          </Text>
        )}
        <Tree
          data={data}
          tree={tree}
          levelOffset={22}
          renderNode={({ node, expanded, hasChildren, elementProps, tree: t }) => {
            const props = (node.nodeProps ?? {}) as {
              labels?: { uuid: string; name: string }[]
              via?: string | null
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
                <Text
                  size="sm"
                  style={{ cursor: 'pointer' }}
                  onClick={() => onOpen(node.value)}
                >
                  {node.label}
                </Text>
                {(props.labels ?? []).map((l) => (
                  <Badge key={l.uuid} size="xs" variant="light">
                    {l.name}
                  </Badge>
                ))}
              </Group>
            )
          }}
        />
      </Card>
    </>
  )
}
