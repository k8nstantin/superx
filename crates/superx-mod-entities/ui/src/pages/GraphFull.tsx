import { useEffect, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Anchor, Box, Group, Text } from '@mantine/core'
import { fetchDetail, fetchPing } from '../api'
import { useBreadcrumb, BreadcrumbTrail } from '../Breadcrumbs'
import { GraphPanel } from '../Graph'

// The graph in its own window (issue #250). A force-directed graph
// needs room: crammed into a panel between the other cards the labels
// collide and the layout has nowhere to spread. This is the same
// GraphPanel, given the whole viewport.
//
// Reached at /graph/<entity-id> — the module's static handler already
// falls back to index.html for unknown paths, so no server route is
// needed for it.

const HEADER = 52
const CAPTION = 78

/** The entity id this window is rooted at, or null when the path is not a graph path. */
export function graphRouteId(pathname: string): string | null {
  const m = /^\/graph\/([A-Za-z0-9-]+)\/?$/.exec(pathname)
  return m ? m[1] : null
}

export default function GraphFull({ frag }: { frag: string }) {
  const detail = useQuery({ queryKey: ['entity', frag], queryFn: () => fetchDetail(frag) })
  const ping = useQuery({ queryKey: ['ping'], queryFn: fetchPing })

  // The graph is a canvas: it needs a pixel height, so it tracks the
  // window rather than a percentage.
  const [height, setHeight] = useState(() => window.innerHeight - HEADER - CAPTION)
  useEffect(() => {
    const onResize = () => setHeight(window.innerHeight - HEADER - CAPTION)
    window.addEventListener('resize', onResize)
    return () => window.removeEventListener('resize', onResize)
  }, [])

  const d = detail.data
  useEffect(() => {
    document.title = d ? `graph · ${d.name} · superx` : 'graph · superx'
  }, [d])

  // Clicking a node re-roots this window on that entity, so exploring
  // never loses the full-window view (and Back still works).
  const reroot = (id: string) => {
    window.history.pushState({}, '', `/graph/${id}`)
    window.dispatchEvent(new PopStateEvent('popstate'))
  }

  // Same trail as the dashboard (#253) — here an ancestor click
  // re-roots the graph rather than leaving the window.
  useBreadcrumb([
    { label: 'Entities', onClick: () => (window.location.href = '/') },
    ...(d?.ancestors ?? []).map((a) => ({
      label: a.name || a.id.slice(0, 8),
      onClick: () => reroot(a.id),
    })),
    ...(d ? [{ label: d.name }] : []),
  ])

  return (
    <div style={{ height: '100vh', display: 'flex', flexDirection: 'column' }}>
      <Group
        h={HEADER}
        px="md"
        gap="lg"
        wrap="nowrap"
        style={{ borderBottom: '1px solid #3B2449', flexShrink: 0 }}
      >
        <Group gap={10} wrap="nowrap">
          <img src="/logo.svg" alt="" width={26} height={26} style={{ borderRadius: 6 }} />
          <Text
            component="span"
            fw={700}
            fz={18}
            c="#EDE4F4"
            style={{ fontFamily: "'Space Grotesk', system-ui, sans-serif" }}
          >
            superx
          </Text>
          <Text c="dimmed" fz={15} style={{ whiteSpace: 'nowrap' }}>
            · graph
          </Text>
        </Group>
        <Box style={{ flex: 1, minWidth: 0 }}>
          <BreadcrumbTrail onHome={() => (window.location.href = '/')} />
        </Box>
        <Group gap="md" wrap="nowrap">
          <Anchor href={`/?entity=${frag}`} size="sm">
            open the entity →
          </Anchor>
          {ping.data?.core_url && (
            <Anchor href={ping.data.core_url} size="sm">
              core dashboard →
            </Anchor>
          )}
        </Group>
      </Group>

      <div style={{ flex: 1, minHeight: 0, padding: '0 12px' }}>
        {detail.isError ? (
          <Text c="red.4" p="md">
            {String(detail.error)}
          </Text>
        ) : (
          <GraphPanel frag={frag} onOpen={reroot} height={height} />
        )}
      </div>
    </div>
  )
}
