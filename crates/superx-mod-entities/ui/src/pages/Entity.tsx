import { useEffect, useMemo, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  ActionIcon,
  Alert,
  Anchor,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Group,
  Loader,
  Menu,
  NumberInput,
  Pill,
  Popover,
  Stack,
  Switch,
  Text,
  TextInput,
  Title,
  Tooltip,
} from '@mantine/core'
import GridLayout, { type LayoutItem } from 'react-grid-layout'
import 'react-grid-layout/css/styles.css'
import 'react-resizable/css/styles.css'
import '../designer.css'
import {
  MACHINERY,
  NAME,
  NAME_SIZE,
  PALETTE,
  SCREEN,
  SIZE,
  declarationOf,
  declareLabels,
  fetchEntity,
  isDatatype,
  linkEntities,
  putAttribute,
  retireAttribute,
  setArchived,
  unlinkEntities,
  type AttributeView,
  type Datatype,
  type Kind,
  type LabelView,
  type LinkView,
} from '../api'
import { Editor } from '../Editor'
import { EntityPicker, LabelsPicker } from '../EntityPicker'

// THE ENTITY IS A SCREEN YOU DESIGN. One field is required — the name —
// and everything else is added from the palette (operator, 2026-09-03):
// a text, a number, a yes/no, a date, a structure, the entity's own
// LABELS, a LINK to another entity. There are no fixed sections. Each
// card is dragged where it should sit and resized to what it needs, and
// the arrangement is saved as data on the entity itself.
//
// Links join entities to entities and nothing else; a label is applied,
// never linked. Both rules live in the pickers, not here.

const COLS = 12
const ROW = 28
const GAP: readonly [number, number] = [10, 10]
const DRAFT_FIELD = 'field:new'
const DRAFT_LINK = 'link:new'
const linkKey = (uid: string) => `link:${uid}`

/// EVERY WRITE ON THIS PAGE CHANGES WHAT THE LISTS SAY. A rename changes
/// a menu row and every picker option; archiving removes a row; a link
/// gives a row its expander; a label moves an entity between the
/// vocabulary and the things. So one refresh, for all of it.
function useRefresh(frag: string) {
  const qc = useQueryClient()
  return () => {
    void qc.invalidateQueries({ queryKey: ['entity', frag] })
    void qc.invalidateQueries({ queryKey: ['roots'] })
    void qc.invalidateQueries({ queryKey: ['all-entities'] })
  }
}

/// One thing on the screen: a field, the labels card, a link, or a draft
/// of one of those that has a place but no record yet.
type CardModel = {
  i: string
  kind: Kind
  attr?: AttributeView
  link?: LinkView
  draft?: boolean
}

type Spot = { x: number; y: number; w: number; h: number }

/// WHERE NEW THINGS LAND. Saved positions win. Anything without one flows
/// left to right, top to bottom, below whatever is saved — so a screen
/// nobody has designed reads as a form, not as a column of banners.
function flow(cards: CardModel[], saved: LayoutItem[], spots: Record<string, Spot>): LayoutItem[] {
  const known = new Map(saved.map((l) => [l.i, l]))
  const out: LayoutItem[] = []
  let bottom = 0
  for (const l of saved) bottom = Math.max(bottom, l.y + l.h)
  let x = 0
  let y = bottom
  let rowH = 0
  for (const c of cards) {
    const s = known.get(c.i)
    if (s) {
      out.push(s)
      continue
    }
    const size = c.attr?.name === NAME ? NAME_SIZE : SIZE[c.kind]
    const spot = spots[c.i]
    if (spot) {
      out.push({ i: c.i, ...spot })
      continue
    }
    if (x + size.w > COLS) {
      x = 0
      y += rowH
      rowH = 0
    }
    out.push({ i: c.i, x, y, w: size.w, h: size.h })
    x += size.w
    rowH = Math.max(rowH, size.h)
  }
  return out
}

const strip = ({ i, x, y, w, h }: LayoutItem) => ({ i, x, y, w, h })

export default function EntityTab({ frag, onOpen }: { frag: string; onOpen: (uuid: string) => void }) {
  const refresh = useRefresh(frag)
  const e = useQuery({ queryKey: ['entity', frag], queryFn: () => fetchEntity(frag) })
  const [design, setDesign] = useState(false)
  const [dragged, setDragged] = useState<LayoutItem[] | null>(null)
  const [live, setLive] = useState<LayoutItem | null>(null)
  const [draftField, setDraftField] = useState<{ kind: Datatype; spot?: Spot } | null>(null)
  const [draftLink, setDraftLink] = useState<{ spot?: Spot } | null>(null)
  const dragKind = useRef<Kind | null>(null)

  const grid = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(1100)
  useEffect(() => {
    const el = grid.current
    if (!el) return
    const ro = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  const attributes = e.data?.attributes ?? []
  const screen = useMemo(() => attributes.find((a) => a.name === SCREEN), [attributes])
  const isRow = useMemo(() => declarationOf(attributes), [attributes])

  // The cards: fields in the order they were added, the labels card if
  // the entity has declared anything, one card per link, then drafts.
  const cards = useMemo<CardModel[]>(() => {
    const out: CardModel[] = attributes
      .filter((a) => !MACHINERY.includes(a.name))
      .map((a) => ({ i: a.uid, kind: (isDatatype(a.datatype) ? a.datatype : 'text') as Kind, attr: a }))
    if (isRow) out.push({ i: isRow.uid, kind: 'labels', attr: isRow })
    for (const l of e.data?.links ?? []) out.push({ i: linkKey(l.uid), kind: 'link', link: l })
    if (draftField) out.push({ i: DRAFT_FIELD, kind: draftField.kind, draft: true })
    if (draftLink) out.push({ i: DRAFT_LINK, kind: 'link', draft: true })
    return out
  }, [attributes, isRow, e.data?.links, draftField, draftLink])

  const saved = useMemo<LayoutItem[]>(
    () => (Array.isArray(screen?.content) ? (screen.content as LayoutItem[]) : []),
    [screen],
  )
  const spots = useMemo(() => {
    const s: Record<string, Spot> = {}
    if (draftField?.spot) s[DRAFT_FIELD] = draftField.spot
    if (draftLink?.spot) s[DRAFT_LINK] = draftLink.spot
    return s
  }, [draftField, draftLink])
  const computed = useMemo(() => flow(cards, saved, spots), [cards, saved, spots])
  // A drag overrides the computed layout until it is saved; anything that
  // appeared since (a dropped draft) is appended so it is not lost.
  const layout = useMemo(
    () => (dragged ? [...dragged, ...computed.filter((c) => !dragged.some((d) => d.i === c.i))] : computed),
    [dragged, computed],
  )
  const dirty = dragged !== null

  /// Write the arrangement to the entity's own `screen` attribute.
  const persist = (items: LayoutItem[]) =>
    putAttribute(frag, {
      uid: screen?.uid ?? null,
      name: SCREEN,
      datatype: 'json',
      content: items.filter((l) => l.i !== DRAFT_FIELD && l.i !== DRAFT_LINK).map(strip),
    })

  const save = useMutation({
    mutationFn: () => persist(layout),
    onSuccess: () => {
      setDragged(null)
      refresh()
    },
  })

  const archive = useMutation({
    mutationFn: (a: boolean) => setArchived(frag, a),
    onSuccess: refresh,
  })

  // FROM THE PALETTE. A field needs a name before it exists, so it lands
  // as a draft card where it was dropped and is written when named; the
  // labels card is the entity's `is` row, started empty; a link is a
  // draft until its far end is chosen.
  const addLabelsCard = useMutation({
    mutationFn: async (spot?: Spot) => {
      if (isRow) return
      const { uid } = await declareLabels(frag, attributes, [])
      await persist([...layout, { i: uid, ...(spot ?? { ...nextFree(layout, SIZE.labels) }) }])
    },
    onSuccess: refresh,
  })
  const add = (kind: Kind, spot?: Spot) => {
    if (kind === 'labels') addLabelsCard.mutate(spot)
    // A link's draft needs a row more than the link it becomes: two rows
    // of controls, then one line of "to whom".
    else if (kind === 'link') setDraftLink({ spot: { ...(spot ?? nextFree(layout, SIZE.link)), h: 4 } })
    // A field's draft likewise: a name row and a labels row, then it
    // settles to its kind's own height.
    else setDraftField({ kind, spot: { ...(spot ?? nextFree(layout, SIZE[kind])), h: 4 } })
  }

  /// Where a card added by click (not by drop) goes: after the last row.
  function nextFree(items: LayoutItem[], size: { w: number; h: number }): Spot {
    let bottom = 0
    for (const l of items) bottom = Math.max(bottom, l.y + l.h)
    return { x: 0, y: bottom, w: size.w, h: size.h }
  }

  /// A draft becomes a record: swap its key in the layout and save.
  const settle = async (draftKey: string, realKey: string, h: number) => {
    const items = layout.map((l) => (l.i === draftKey ? { ...l, i: realKey, h } : l))
    await persist(items)
  }

  const guides = {
    '--sx-col': `${(width - GAP[0] * (COLS - 1) - GAP[0] * 2) / COLS + GAP[0]}px`,
    '--sx-row': `${ROW + GAP[1]}px`,
    '--sx-pad': `${GAP[0]}px`,
  } as React.CSSProperties

  if (e.isLoading) return <Loader size="sm" />
  if (e.isError) {
    return (
      <Alert color="red" title="Cannot read this entity" maw={640}>
        {String(e.error)}
      </Alert>
    )
  }
  if (!e.data) return null

  return (
    <>
      <Card withBorder padding="sm" mb="sm">
        <Group justify="space-between" wrap="nowrap" gap="md">
          <Group gap="xs" wrap="nowrap" style={{ minWidth: 0 }}>
            <Title order={4} lineClamp={1} title={e.data.name}>
              {e.data.name || '(unnamed)'}
            </Title>
            {e.data.archived && <Badge color="gray">archived</Badge>}
          </Group>
          <Group gap="sm" wrap="nowrap">
            <Tooltip label="Drag cards to arrange them; pull the corner grip to resize" withArrow>
              <Switch
                label="design"
                checked={design}
                onChange={(ev) => setDesign(ev.currentTarget.checked)}
              />
            </Tooltip>
            {design && (
              <Button size="xs" onClick={() => save.mutate()} loading={save.isPending} disabled={!dirty}>
                Save layout
              </Button>
            )}
            <Button
              size="xs"
              variant="light"
              color="gray"
              onClick={() => archive.mutate(!e.data!.archived)}
            >
              {e.data.archived ? 'Restore' : 'Archive'}
            </Button>
          </Group>
        </Group>
        <Text size="xs" c="dimmed" ff="monospace" mt={2}>
          {e.data.uuid}
        </Text>
      </Card>

      {/* THE PALETTE. Pick something up and drop it where it goes, or
          click it to add it after the last row. */}
      <Group gap={6} mb="sm" wrap="wrap" align="center">
        <Text size="xs" c="dimmed" mr={4}>
          Add
        </Text>
        {PALETTE.map((p) => (
          <Tooltip key={p.kind} label={p.hint} withArrow openDelay={400}>
            <Badge
              component="div"
              className="sx-palette-item"
              variant={p.kind === 'labels' || p.kind === 'link' ? 'outline' : 'light'}
              size="lg"
              radius="sm"
              draggable
              onDragStart={(ev: React.DragEvent) => {
                dragKind.current = p.kind
                ev.dataTransfer.setData('text/plain', p.kind)
                ev.dataTransfer.effectAllowed = 'copy'
              }}
              onDragEnd={() => {
                dragKind.current = null
              }}
              onClick={() => add(p.kind)}
              role="button"
              tabIndex={0}
              onKeyDown={(ev: React.KeyboardEvent) => {
                if (ev.key === 'Enter' || ev.key === ' ') add(p.kind)
              }}
              aria-label={`Add ${p.title}`}
              style={{ cursor: 'grab', textTransform: 'none', fontWeight: 500 }}
            >
              {p.title}
            </Badge>
          </Tooltip>
        ))}
        <Text size="xs" c="dimmed" ml={4}>
          drag onto the screen, or click
        </Text>
        {(addLabelsCard.error ?? save.error) && (
          <Text c="red" size="xs">
            {String(addLabelsCard.error ?? save.error)}
          </Text>
        )}
      </Group>

      <div ref={grid} className={`designer${design ? ' designing' : ''}`} style={guides}>
        <GridLayout
          className="layout"
          layout={layout}
          width={width}
          gridConfig={{ cols: COLS, rowHeight: ROW, margin: GAP, containerPadding: GAP }}
          // Dragging is OFF until design is on, and a pointer inside a
          // control must never start a drag — otherwise typing moves the
          // box.
          dragConfig={{
            enabled: design,
            cancel: 'input,textarea,button,a,[contenteditable],.bn-container,.mantine-Popover-dropdown',
          }}
          resizeConfig={{ enabled: design, handles: ['se', 'e', 's'] }}
          // The palette drops in whether or not design is on: placing a
          // new field is not redesigning the ones that are there.
          dropConfig={{
            enabled: true,
            defaultItem: { w: 6, h: 3 },
            onDragOver: () => (dragKind.current ? SIZE[dragKind.current] : false),
          }}
          onDrop={(_l, item) => {
            const kind = dragKind.current
            dragKind.current = null
            if (kind && item) add(kind, { x: item.x, y: item.y, w: item.w, h: item.h })
          }}
          onDrag={(_l, _o, n) => setLive(n)}
          onResize={(_l, _o, n) => setLive(n)}
          onDragStop={(l) => {
            setDragged([...l])
            setLive(null)
          }}
          onResizeStop={(l) => {
            setDragged([...l])
            setLive(null)
          }}
        >
          {cards.map((c) => (
            <div key={c.i}>
              {live?.i === c.i && (
                <div className="sx-live">
                  {live.w} × {live.h}
                </div>
              )}
              {c.draft && c.kind === 'link' ? (
                <DraftLink
                  frag={frag}
                  onDone={async (uid) => {
                    if (uid) await settle(DRAFT_LINK, linkKey(uid), SIZE.link.h)
                    setDraftLink(null)
                    refresh()
                  }}
                />
              ) : c.draft ? (
                <DraftField
                  frag={frag}
                  kind={c.kind as Datatype}
                  onDone={async (uid) => {
                    if (uid) await settle(DRAFT_FIELD, uid, SIZE[c.kind].h)
                    setDraftField(null)
                    refresh()
                  }}
                />
              ) : c.kind === 'labels' ? (
                <LabelsCard
                  frag={frag}
                  attributes={attributes}
                  labels={e.data!.labels}
                  onOpen={onOpen}
                  readOnly={design}
                  onRemove={async () => {
                    await retireAttribute(frag, c.i)
                    await persist(layout.filter((l) => l.i !== c.i))
                    refresh()
                  }}
                />
              ) : c.kind === 'link' && c.link ? (
                <LinkCard
                  link={c.link}
                  onOpen={onOpen}
                  readOnly={design}
                  onCut={async () => {
                    await unlinkEntities(frag, c.link!.uid)
                    await persist(layout.filter((l) => l.i !== c.i))
                    refresh()
                  }}
                />
              ) : c.attr ? (
                <Field
                  frag={frag}
                  field={c.attr}
                  readOnly={design}
                  onRemove={async () => {
                    await retireAttribute(frag, c.i)
                    await persist(layout.filter((l) => l.i !== c.i))
                    refresh()
                  }}
                />
              ) : null}
            </div>
          ))}
        </GridLayout>
      </div>
    </>
  )
}

/// The header every card shares: what it is, what it carries, and the
/// menu that takes it away.
function CardHeader({
  title,
  kind,
  children,
  menu,
}: {
  title: string
  kind: string
  children?: React.ReactNode
  menu?: { label: string; onClick: () => void; danger?: boolean }[]
}) {
  return (
    <Group gap={6} mb={4} wrap="nowrap" align="center">
      <Text size="xs" fw={600} truncate style={{ minWidth: 0 }} title={title}>
        {title}
      </Text>
      <Text size="xs" c="dimmed" ff="monospace" style={{ flexShrink: 0 }}>
        {kind}
      </Text>
      {children}
      <Box style={{ flex: 1 }} />
      {menu && menu.length > 0 && (
        <Menu position="bottom-end" withinPortal shadow="md">
          <Menu.Target>
            <ActionIcon size="xs" variant="subtle" color="gray" aria-label={`${title} actions`}>
              ⋯
            </ActionIcon>
          </Menu.Target>
          <Menu.Dropdown>
            {menu.map((m) => (
              <Menu.Item key={m.label} color={m.danger ? 'red' : undefined} onClick={m.onClick}>
                {m.label}
              </Menu.Item>
            ))}
          </Menu.Dropdown>
        </Menu>
      )}
    </Group>
  )
}

/// One field, rendered by its datatype. Prose shows as a preview until it
/// is clicked, then hands over to the editor.
function Field({
  frag,
  field,
  readOnly,
  onRemove,
}: {
  frag: string
  field: AttributeView
  readOnly: boolean
  onRemove: () => Promise<void>
}) {
  const refresh = useRefresh(frag)
  const [draft, setDraft] = useState<string | number | undefined>(undefined)
  const [editing, setEditing] = useState(false)
  const [adding, setAdding] = useState(false)
  const write = useMutation({
    mutationFn: (content: unknown) =>
      putAttribute(frag, {
        uid: field.uid,
        name: field.name,
        datatype: field.datatype,
        content,
        labels: field.labels.map((l) => l.uuid),
        options: field.options,
      }),
    onSuccess: () => {
      setEditing(false)
      refresh()
    },
  })
  // WHAT A FIELD IS. Labels apply to fields as much as to entities, and
  // the runner acts on them — so they are on the card, removable, and one
  // more can be added in place. The value travels with the amend
  // untouched: only the labels change.
  const relabel = useMutation({
    mutationFn: (labels: LabelView[]) =>
      putAttribute(frag, {
        uid: field.uid,
        name: field.name,
        datatype: field.datatype,
        content: field.content,
        labels: labels.map((l) => l.uuid),
        options: field.options,
      }),
    onSuccess: () => {
      setAdding(false)
      refresh()
    },
  })
  const remove = useMutation({ mutationFn: onRemove })

  const body = () => {
    if (readOnly) {
      return (
        <Text size="sm" c="dimmed" lineClamp={4}>
          {preview(field)}
        </Text>
      )
    }
    switch (field.datatype) {
      case 'number':
        return (
          <NumberInput
            aria-label={field.name}
            value={draft ?? (typeof field.content === 'number' ? field.content : '')}
            onChange={setDraft}
            onBlur={(ev) => {
              // BLANK IS NOT ZERO, and only a change is written.
              const raw = ev.currentTarget.value.trim()
              if (raw === '') return
              const n = Number(raw)
              if (!Number.isNaN(n) && n !== field.content) write.mutate(n)
            }}
          />
        )
      case 'boolean':
        return (
          <Checkbox
            aria-label={field.name}
            label={field.content === true ? 'yes' : 'no'}
            checked={field.content === true}
            onChange={(ev) => write.mutate(ev.currentTarget.checked)}
          />
        )
      case 'datetime':
        // `datetime-local` speaks LOCAL time; the store speaks UTC.
        return (
          <TextInput
            aria-label={field.name}
            type="datetime-local"
            defaultValue={toLocalInput(field.content)}
            onBlur={(ev) => {
              if (!ev.currentTarget.value) return
              const iso = new Date(ev.currentTarget.value).toISOString()
              const now = typeof field.content === 'string' ? Date.parse(field.content) : NaN
              if (Date.parse(iso) !== now) write.mutate(iso)
            }}
          />
        )
      case 'text':
        // THE NAME IS ONE LINE. The editor would rewrite it as `<p>DBA</p>`.
        if (field.name === NAME) {
          return (
            <TextInput
              aria-label={field.name}
              defaultValue={typeof field.content === 'string' ? field.content : ''}
              onBlur={(ev) => {
                const v = ev.currentTarget.value.trim()
                if (v && v !== field.content) write.mutate(v)
              }}
            />
          )
        }
        if (!editing) {
          const shown = preview(field)
          return (
            <Box
              className="sx-prose-preview"
              role="button"
              tabIndex={0}
              aria-label={`Edit ${field.name}`}
              onClick={() => setEditing(true)}
              onKeyDown={(ev) => {
                if (ev.key === 'Enter') setEditing(true)
              }}
            >
              <Text size="sm" c={shown ? undefined : 'dimmed'} style={{ whiteSpace: 'pre-wrap' }}>
                {shown || 'Click to write…'}
              </Text>
            </Box>
          )
        }
        return (
          <Editor
            value={field.content}
            json={false}
            autoFocus
            onSave={(v) => write.mutate(v)}
            onDone={() => setEditing(false)}
          />
        )
      default:
        return <Editor value={field.content} json={field.datatype === 'json'} onSave={(v) => write.mutate(v)} />
    }
  }

  return (
    <Card withBorder padding="xs" h="100%" style={{ overflow: 'auto', display: 'flex', flexDirection: 'column' }}>
      <CardHeader
        title={field.name}
        kind={field.datatype}
        menu={field.name === NAME ? undefined : [{ label: 'Remove field', danger: true, onClick: () => remove.mutate() }]}
      >
        {field.labels.map((l) => (
          <Pill
            key={l.uuid}
            size="xs"
            withRemoveButton={!readOnly}
            removeButtonProps={{ 'aria-label': `Remove label ${l.name}` }}
            onRemove={() => relabel.mutate(field.labels.filter((x) => x.uuid !== l.uuid))}
            style={{ flexShrink: 0 }}
          >
            {l.name}
          </Pill>
        ))}
        {!readOnly && (
          <Popover opened={adding} onChange={setAdding} trapFocus withArrow position="bottom-start" width={260}>
            <Popover.Target>
              <ActionIcon
                size="xs"
                variant="subtle"
                aria-label={`Add a label to ${field.name}`}
                onClick={() => setAdding((o) => !o)}
              >
                +
              </ActionIcon>
            </Popover.Target>
            <Popover.Dropdown>
              <EntityPicker
                label="Add a label"
                placeholder="mandate"
                size="xs"
                kind="label"
                exclude={[frag, ...field.labels.map((l) => l.uuid)]}
                value={null}
                onChange={(v) => v && relabel.mutate([...field.labels, { uuid: v, name: '' }])}
              />
            </Popover.Dropdown>
          </Popover>
        )}
      </CardHeader>
      <Box style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>{body()}</Box>
      {(write.error ?? relabel.error ?? remove.error) && (
        <Text c="red" size="xs" mt={4}>
          {String(write.error ?? relabel.error ?? remove.error)}
        </Text>
      )}
    </Card>
  )
}

/// A field with a place but no name yet. Name it and it exists.
function DraftField({
  frag,
  kind,
  onDone,
}: {
  frag: string
  kind: Datatype
  onDone: (uid: string | null) => Promise<void>
}) {
  const [name, setName] = useState('')
  const [labels, setLabels] = useState<string[]>([])
  const create = useMutation({
    mutationFn: async () => {
      const { uid } = await putAttribute(frag, { name: name.trim(), datatype: kind, content: null, labels })
      await onDone(uid)
    },
  })
  return (
    <Card withBorder padding="xs" h="100%" style={{ borderColor: '#b833e8', overflow: 'auto' }}>
      <CardHeader title={`new ${PALETTE.find((p) => p.kind === kind)?.title.toLowerCase() ?? kind}`} kind={kind} />
      <Stack gap={6}>
        <TextInput
          size="xs"
          data-autofocus
          autoFocus
          aria-label="Field name"
          placeholder="name this field"
          value={name}
          onChange={(ev) => setName(ev.currentTarget.value)}
          onKeyDown={(ev) => {
            if (ev.key === 'Enter' && name.trim() && !create.isPending) create.mutate()
            if (ev.key === 'Escape') void onDone(null)
          }}
        />
        <Group gap={6} wrap="nowrap" align="flex-end">
          <LabelsPicker placeholder="labels (optional)" exclude={[frag]} value={labels} onChange={setLabels} />
          <Button size="xs" onClick={() => create.mutate()} loading={create.isPending} disabled={!name.trim()}>
            Add
          </Button>
          <Button size="xs" variant="subtle" color="gray" onClick={() => void onDone(null)}>
            Cancel
          </Button>
        </Group>
        {create.error && (
          <Text c="red" size="xs">
            {String(create.error)}
          </Text>
        )}
      </Stack>
    </Card>
  )
}

/// The entity's own labels: the `is` row, as a card on the screen.
function LabelsCard({
  frag,
  attributes,
  labels,
  onOpen,
  readOnly,
  onRemove,
}: {
  frag: string
  attributes: AttributeView[]
  labels: LabelView[]
  onOpen: (uuid: string) => void
  readOnly: boolean
  onRemove: () => Promise<void>
}) {
  const refresh = useRefresh(frag)
  const set = useMutation({
    mutationFn: (next: string[]) => declareLabels(frag, attributes, next),
    onSuccess: refresh,
  })
  const remove = useMutation({ mutationFn: onRemove })
  return (
    <Card withBorder padding="xs" h="100%" style={{ overflow: 'auto' }}>
      <CardHeader
        title="labels"
        kind="what this IS"
        menu={[{ label: 'Remove labels card', danger: true, onClick: () => remove.mutate() }]}
      />
      <Group gap={6} wrap="wrap" align="center">
        {labels.map((l) => (
          <Pill
            key={l.uuid}
            withRemoveButton={!readOnly}
            removeButtonProps={{ 'aria-label': `Remove label ${l.name}` }}
            onRemove={() => set.mutate(labels.filter((x) => x.uuid !== l.uuid).map((x) => x.uuid))}
          >
            <span style={{ cursor: 'pointer' }} onClick={() => onOpen(l.uuid)}>
              {l.name}
            </span>
          </Pill>
        ))}
        {labels.length === 0 && (
          <Text size="sm" c="dimmed">
            Nothing yet.
          </Text>
        )}
        {!readOnly && (
          <Box style={{ flex: 1, minWidth: 160 }}>
            <EntityPicker
              placeholder="add a label…"
              size="xs"
              kind="label"
              exclude={[frag, ...labels.map((l) => l.uuid)]}
              value={null}
              onChange={(v) => v && set.mutate([...labels.map((l) => l.uuid), v])}
            />
          </Box>
        )}
      </Group>
      {(set.error ?? remove.error) && (
        <Text c="red" size="xs" mt={4}>
          {String(set.error ?? remove.error)}
        </Text>
      )}
    </Card>
  )
}

/// One link, from this entity's point of view.
function LinkCard({
  link,
  onOpen,
  readOnly,
  onCut,
}: {
  link: LinkView
  onOpen: (uuid: string) => void
  readOnly: boolean
  onCut: () => Promise<void>
}) {
  const cut = useMutation({ mutationFn: onCut })
  return (
    <Card withBorder padding="xs" h="100%" style={{ overflow: 'auto' }}>
      <CardHeader
        title={link.name}
        kind={link.outbound ? 'link →' : '← link'}
        menu={[{ label: 'Cut link', danger: true, onClick: () => cut.mutate() }]}
      >
        {link.labels.map((l) => (
          <Badge key={l.uuid} size="xs" variant="light" style={{ flexShrink: 0 }}>
            {l.name}
          </Badge>
        ))}
      </CardHeader>
      <Group gap={6} wrap="nowrap">
        <Text size="xs" c="dimmed">
          {link.outbound ? 'to' : 'from'}
        </Text>
        <Anchor size="sm" fw={600} onClick={() => onOpen(link.other)} truncate>
          {link.other_name || link.other.slice(0, 8)}
        </Anchor>
      </Group>
      {cut.error && (
        <Text c="red" size="xs" mt={4}>
          {String(cut.error)}
        </Text>
      )}
    </Card>
  )
}

/// A link with a place but no far end yet.
function DraftLink({ frag, onDone }: { frag: string; onDone: (uid: string | null) => Promise<void> }) {
  const [name, setName] = useState('')
  const [to, setTo] = useState<string | null>(null)
  const [meaning, setMeaning] = useState<string | null>(null)
  const create = useMutation({
    mutationFn: async () => {
      const { uid } = await linkEntities(frag, to ?? '', name.trim(), meaning ? [meaning] : [])
      await onDone(uid)
    },
  })
  return (
    <Card withBorder padding="xs" h="100%" style={{ borderColor: '#b833e8', overflow: 'auto' }}>
      <CardHeader title="new link" kind="entity → entity" />
      <Stack gap={6}>
        <Group gap={6} wrap="nowrap" align="flex-end">
          <TextInput
            size="xs"
            autoFocus
            aria-label="What this link is called"
            placeholder="depends on"
            value={name}
            onChange={(ev) => setName(ev.currentTarget.value)}
            style={{ flex: 1 }}
          />
          <EntityPicker placeholder="to…" size="xs" kind="thing" exclude={[frag]} value={to} onChange={setTo} />
        </Group>
        <Group gap={6} wrap="nowrap" align="flex-end">
          <EntityPicker placeholder="meaning (a label)" size="xs" kind="label" exclude={[frag]} value={meaning} onChange={setMeaning} />
          <Button
            size="xs"
            onClick={() => create.mutate()}
            loading={create.isPending}
            disabled={!to || !name.trim()}
          >
            Link
          </Button>
          <Button size="xs" variant="subtle" color="gray" onClick={() => void onDone(null)}>
            Cancel
          </Button>
        </Group>
        {create.error && (
          <Text c="red" size="xs">
            {String(create.error)}
          </Text>
        )}
      </Stack>
    </Card>
  )
}

/// A stored UTC instant, as the local wall-clock the widget expects.
function toLocalInput(content: unknown): string {
  if (typeof content !== 'string') return ''
  const d = new Date(content)
  if (Number.isNaN(d.getTime())) return ''
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`
}

/// What a field shows when it is not being edited: formatted the way the
/// control would show it, not the way the store holds it.
function preview(f: AttributeView): string {
  const c = f.content
  if (c === null || c === undefined) return ''
  switch (f.datatype) {
    case 'datetime': {
      const d = new Date(String(c))
      return Number.isNaN(d.getTime()) ? String(c) : d.toLocaleString()
    }
    case 'boolean':
      return c === true ? 'yes' : 'no'
    case 'json':
      return JSON.stringify(c)
    default:
      return typeof c === 'string' ? c.replace(/<\/p>\s*<p>/g, '\n').replace(/<[^>]*>/g, '') : JSON.stringify(c)
  }
}
