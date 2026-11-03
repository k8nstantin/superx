import { useMemo, useState, type ReactNode } from 'react'
import { Button, Group, Paper, Text } from '@mantine/core'
import GridLayout, { useContainerWidth, type Layout, type LayoutItem } from 'react-grid-layout'
import 'react-grid-layout/css/styles.css'
import 'react-resizable/css/styles.css'

// A board of cards the operator arranges: each card is dragged by its title
// and resized from its right edge, its bottom edge or its corner, and what
// it draws takes the card's size. The arrangement is kept in this browser,
// per board; "Reset layout" puts the cards back where they started.

export type Card = {
  i: string
  title: string
  note?: string
  /// Its starting width, of 12 columns, and height, in rows.
  w: number
  h: number
  /// What it draws, given the height left under its title, in pixels.
  render: (height: number) => ReactNode
}

const COLS = 12
const ROW = 36
const GAP: readonly [number, number] = [14, 14]
/// The title and note at a card's top, and its padding.
const HEAD = 64

function key(board: string) {
  return `recon.board.${board}`
}

function saved(board: string): LayoutItem[] | null {
  try {
    const s = window.localStorage.getItem(key(board))
    return s ? (JSON.parse(s) as LayoutItem[]) : null
  } catch {
    return null
  }
}

function keep(board: string, layout: Layout) {
  try {
    window.localStorage.setItem(key(board), JSON.stringify(layout.map(({ i, x, y, w, h }) => ({ i, x, y, w, h }))))
  } catch {
    // A browser that keeps nothing still shows the board.
  }
}

/// The cards, left to right and row by row, at their starting sizes; where
/// this browser kept an arrangement, the cards it holds keep theirs.
function arranged(cards: Card[], kept: LayoutItem[] | null): LayoutItem[] {
  let x = 0
  let y = 0
  let rowH = 0
  const fresh = cards.map((c) => {
    if (x + c.w > COLS) {
      x = 0
      y += rowH
      rowH = 0
    }
    const item = { i: c.i, x, y, w: c.w, h: c.h, minW: 3, minH: 5 }
    x += c.w
    rowH = Math.max(rowH, c.h)
    return item
  })
  if (!kept) return fresh
  return fresh.map((f) => {
    const k = kept.find((s) => s.i === f.i)
    return k ? { ...f, x: k.x, y: k.y, w: k.w, h: k.h } : f
  })
}

export function Board({ name, cards }: { name: string; cards: Card[] }) {
  const { width, containerRef, mounted } = useContainerWidth()
  const ids = cards.map((c) => c.i).join(',')
  const [layout, setLayout] = useState<LayoutItem[]>(() => arranged(cards, saved(name)))
  // Cards added since the arrangement was kept take their starting place.
  const shown = useMemo(() => {
    const have = new Set(layout.map((l) => l.i))
    return cards.every((c) => have.has(c.i)) ? layout : arranged(cards, layout)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ids, layout])
  return (
    <div ref={containerRef}>
      <Group justify="flex-end" mb={4}>
        <Text size="xs" c="dimmed">drag a card by its title · resize it from its edge or corner</Text>
        <Button
          size="compact-xs"
          variant="subtle"
          onClick={() => {
            const fresh = arranged(cards, null)
            setLayout(fresh)
            keep(name, fresh)
          }}
        >
          Reset layout
        </Button>
      </Group>
      {mounted && (
        <GridLayout
          layout={shown}
          width={width}
          gridConfig={{ cols: COLS, rowHeight: ROW, margin: GAP, containerPadding: [0, 0] }}
          dragConfig={{ enabled: true, handle: '.sx-card-head', cancel: 'button,input,textarea,a' }}
          resizeConfig={{ enabled: true, handles: ['se', 'e', 's'] }}
          onLayoutChange={(l) => {
            setLayout([...l])
            keep(name, l)
          }}
        >
          {cards.map((c) => {
            const item = shown.find((l) => l.i === c.i)
            const px = item ? item.h * ROW + (item.h - 1) * GAP[1] : c.h * ROW
            return (
              <div key={c.i}>
                <Paper withBorder radius="md" p="sm" h="100%" style={{ display: 'flex', flexDirection: 'column', overflow: 'hidden' }}>
                  <div className="sx-card-head" style={{ cursor: 'grab' }}>
                    <Text fw={600}>{c.title}</Text>
                    {c.note && <Text size="xs" c="dimmed" lineClamp={1}>{c.note}</Text>}
                  </div>
                  <div style={{ flex: 1, minHeight: 0, overflow: 'auto' }}>{c.render(Math.max(120, px - HEAD))}</div>
                </Paper>
              </div>
            )
          })}
        </GridLayout>
      )}
    </div>
  )
}
