import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Badge, Card, Group, Loader, Slider, Text } from '@mantine/core'
import { fetchGraph, type GraphNodeView } from '../api'
import { EChart } from '../EChart'

// THE GRAPH IS PER ENTITY: rooted where you opened it, never a global
// map. A global map of a graph with no bottom is a hairball; the useful
// question is always "what is around THIS one".
//
// One request, whatever the depth — the walk happens in the engine.
// Colour is by distance from the root, so the shape reads at a glance,
// and clicking a node walks to it.

const RING = ['#EDE4F4', '#B833E8', '#8F7BA5', '#5C4470', '#3B2449']

export default function GraphTab({
  frag,
  onOpen,
}: {
  frag: string
  onOpen: (uuid: string) => void
}) {
  const [depth, setDepth] = useState(2)
  const g = useQuery({
    queryKey: ['graph', frag, depth],
    queryFn: () => fetchGraph(frag, depth),
  })

  if (g.isLoading) return <Loader size="sm" />
  if (g.error) return <Text c="red">{String(g.error)}</Text>
  if (!g.data) return null

  const colour = (n: GraphNodeView) => RING[Math.min(n.depth, RING.length - 1)]

  const option = {
    tooltip: {
      backgroundColor: '#1F062A',
      borderColor: '#3B2449',
      textStyle: { color: '#EDE4F4' },
    },
    series: [
      {
        type: 'graph',
        layout: 'force',
        roam: true,
        draggable: true,
        // Focus a node's own adjacency on hover — the only way to read a
        // dense patch without moving anything.
        emphasis: { focus: 'adjacency' },
        // SPREAD TO FILL THE BOX. Repulsion that grows with the node
        // count is backwards for the graphs anyone actually opens: four
        // entities got the weakest push and huddled in one corner of an
        // otherwise empty card with their labels on top of each other.
        // A small graph pushes hard and uses the room; a large one eases
        // off so it does not fly apart.
        center: ['50%', '50%'],
        force: {
          // The card is far wider than it is tall now that the graph
          // has the whole pane, and a force layout left to itself ties
          // a small knot in the middle of it. Push harder and let the
          // edges run longer so the drawing uses the room it has.
          repulsion: Math.max(700, 3200 / Math.sqrt(g.data.nodes.length || 1)),
          edgeLength: [160, 380],
          // Enough pull to settle in the middle of the card. Weaker and
          // the cluster drifts into a corner even when it has room; the
          // repulsion above is what keeps it from collapsing back in.
          gravity: 0.18,
        },
        label: { show: true, color: '#EDE4F4', fontSize: 13, position: 'right', distance: 8 },
        // Two names in the same place are worse than one name: where
        // they collide, the one further down the draw order steps out.
        labelLayout: { hideOverlap: true },
        edgeLabel: { show: true, color: '#9280A6', fontSize: 11, formatter: '{c}' },
        lineStyle: { color: '#5C4470', curveness: 0.08 },
        data: g.data.nodes.map((n) => ({
          id: n.uuid,
          name: n.name || n.uuid.slice(0, 8),
          symbolSize: n.depth === 0 ? 46 : 32,
          itemStyle: { color: colour(n) },
        })),
        links: g.data.edges.map((e) => ({
          source: e.from,
          target: e.to,
          value: e.name,
        })),
      },
    ],
  }

  return (
    <Card withBorder padding="md">
      <Group justify="space-between" mb="sm" wrap="nowrap">
        <Group gap="xs">
          <Text size="sm" fw={600}>
            {g.data.nodes.find((n) => n.depth === 0)?.name ?? 'graph'}
          </Text>
          <Badge size="xs" variant="light">
            {g.data.nodes.length} entities
          </Badge>
          <Badge size="xs" variant="light">
            {g.data.edges.length} links
          </Badge>
        </Group>
        <Group gap="xs" w={240}>
          <Text size="xs" c="dimmed">
            depth
          </Text>
          <Slider
            min={1}
            max={8}
            step={1}
            value={depth}
            onChange={setDepth}
            style={{ flex: 1 }}
            marks={[{ value: 1 }, { value: 4 }, { value: 8 }]}
          />
        </Group>
      </Group>
      <EChart
        option={option}
        height={520}
        onEvents={{
          click: (p: { dataType?: string; data?: { id?: string } }) => {
            if (p.dataType === 'node' && p.data?.id) onOpen(p.data.id)
          },
        }}
      />
    </Card>
  )
}
