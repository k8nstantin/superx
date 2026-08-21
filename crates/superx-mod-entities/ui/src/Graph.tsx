import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Badge, Group, Loader, SegmentedControl, Slider, Text } from '@mantine/core'
import { fetchGraph, typeColor } from './api'
import { EChart, INK, INK_MUTED, TOOLTIP } from './EChart'
import type { GraphNodeView } from './generated/GraphNodeView'

// EU5 — the graph is PER ENTITY (approved design): rooted where you
// opened it, never a global map. Force-directed ECharts `graph`, the
// pattern proven in the operator's own dag view: force tuning scales
// with node count, hover focuses a node's adjacency, click walks to
// the entity you clicked.

const RING = ['#EDE4F4', '#B833E8', '#8F7BA5', '#5C4470']

export function GraphPanel({
  frag,
  onOpen,
  height = 460,
}: {
  frag: string
  onOpen: (id: string) => void
  height?: number
}) {
  const [depth, setDepth] = useState(2)
  const [direction, setDirection] = useState('both')
  const g = useQuery({
    queryKey: ['graph', frag, depth, direction],
    queryFn: () => fetchGraph(frag, depth, direction),
  })

  const nodes = g.data?.nodes ?? []
  const edges = g.data?.edges ?? []
  const types = [...new Set(nodes.map((n) => n.entity_type))].sort()
  const byId = new Map(nodes.map((n) => [n.id, n]))

  const data = nodes.map((n: GraphNodeView) => {
    const d = Number(n.depth)
    const root = n.id === g.data?.root
    return {
      id: n.id,
      name: n.name || n.id.slice(0, 8),
      category: types.indexOf(n.entity_type),
      // The root is biggest; each hop out is a little smaller, so the
      // eye finds the entity you opened without reading a label.
      symbolSize: root ? 42 : Math.max(14, 30 - d * 5),
      itemStyle: {
        color: typeColor(n.entity_type),
        borderColor: RING[Math.min(d, RING.length - 1)],
        borderWidth: root ? 3 : 1,
      },
      label: { fontWeight: root ? 700 : 400 },
    }
  })

  // How much room this canvas has, relative to the inline panel the
  // force numbers were originally tuned against (460px tall).
  const roominess = Math.min(2.2, Math.max(1, height / 460))

  const links = edges.map((e) => ({
    source: e.from,
    target: e.to,
    value: e.rel_type,
    lineStyle: {
      // depends_on is execution order, not just association — it earns
      // the solid, brighter line.
      color: e.rel_type === 'depends_on' ? '#B833E8' : '#5C4470',
      width: e.rel_type === 'depends_on' ? 2 : 1.2,
      type: e.rel_type === 'attached' ? 'dashed' : 'solid',
      curveness: 0.12,
      opacity: 0.85,
    },
  }))

  return (
    <div>
      <Group justify="space-between" mb="xs">
        <Group gap="xs">
          <SegmentedControl
            size="xs"
            value={direction}
            onChange={setDirection}
            data={[
              { label: 'both ways', value: 'both' },
              { label: 'outbound', value: 'out' },
              { label: 'inbound', value: 'in' },
            ]}
          />
          <Text size="xs" c="dimmed">
            depth
          </Text>
          <Slider
            w={110}
            size="xs"
            min={1}
            max={5}
            step={1}
            value={depth}
            onChange={setDepth}
            marks={[{ value: 1 }, { value: 3 }, { value: 5 }]}
          />
        </Group>
        <Group gap={6}>
          {g.isFetching && <Loader size="xs" />}
          <Text size="xs" c="dimmed">
            {nodes.length} nodes · {edges.length} edges
            {g.data?.truncated ? ' · more beyond this depth' : ''}
          </Text>
        </Group>
      </Group>

      {g.isError && (
        <Text c="red.4" size="sm">
          {String(g.error)}
        </Text>
      )}
      {!g.isError && nodes.length <= 1 && !g.isFetching && (
        <Text c="dimmed" size="sm" py="xl" ta="center">
          nothing linked yet — use + Link to connect this entity to others
        </Text>
      )}
      {nodes.length > 1 && (
        <EChart
          height={height}
          option={{
            tooltip: {
              ...TOOLTIP,
              formatter: (p: { dataType: string; data: { name?: string; value?: string } }) =>
                p.dataType === 'edge'
                  ? `—[${p.data.value}]→`
                  : (p.data.name ?? ''),
            },
            legend: [
              {
                data: types,
                top: 0,
                textStyle: { color: INK_MUTED, fontSize: 11 },
                inactiveColor: '#5C4470',
              },
            ],
            animationDurationUpdate: 600,
            animationEasingUpdate: 'quinticInOut',
            series: [
              {
                type: 'graph',
                layout: 'force',
                data,
                links,
                categories: types.map((t) => ({ name: t, itemStyle: { color: typeColor(t) } })),
                roam: true,
                draggable: true,
                edgeSymbol: ['none', 'arrow'],
                edgeSymbolSize: 7,
                label: {
                  show: true,
                  position: 'right',
                  fontSize: 11,
                  color: INK,
                  formatter: (p: { data: { name: string } }) =>
                    p.data.name.length > 28 ? `${p.data.name.slice(0, 26)}…` : p.data.name,
                },
                emphasis: { focus: 'adjacency', label: { show: true, fontWeight: 'bold' } },
                // Sparse graphs need far more repulsion to use the
                // canvas; dense ones fly apart with it. Scale to size.
                force: {
                  // Sparse graphs need far more repulsion to use the
                  // canvas; dense ones fly apart with it. Scaled by the
                  // canvas too (#250): the same graph in a full window
                  // has more room to spread than in a panel, and fixed
                  // numbers leave a big window mostly empty.
                  repulsion: Math.round(
                    (data.length <= 10 ? 1500 : data.length <= 30 ? 600 : 320) * roominess,
                  ),
                  edgeLength: (data.length <= 10 ? [180, 280] : [100, 180]).map((l) =>
                    Math.round(l * roominess),
                  ),
                  gravity: 0.08,
                  friction: 0.6,
                  layoutAnimation: true,
                },
                center: ['50%', '50%'],
              },
            ],
          }}
          onEvents={{
            click: ((p: { dataType?: string; data?: { id?: string } }) => {
              if (p.dataType === 'node' && p.data?.id && p.data.id !== g.data?.root) {
                onOpen(p.data.id)
              }
            }) as never,
          }}
        />
      )}
      {nodes.length > 1 && (
        <Group gap={6} mt="xs">
          <Text size="xs" c="dimmed">
            click a node to open it · drag to pan · scroll to zoom
          </Text>
          {byId.get(g.data?.root ?? '') && (
            <Badge size="xs" variant="light" color="pelican">
              rooted at {byId.get(g.data?.root ?? '')?.name}
            </Badge>
          )}
        </Group>
      )}
    </div>
  )
}
