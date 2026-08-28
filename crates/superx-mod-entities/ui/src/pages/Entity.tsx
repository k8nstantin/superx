import { useEffect, useMemo, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  ActionIcon,
  Badge,
  Button,
  Card,
  Checkbox,
  Group,
  Loader,
  NumberInput,
  Select,
  Switch,
  Text,
  TextInput,
  Title,
} from '@mantine/core'
import GridLayout, { type LayoutItem } from 'react-grid-layout'
import 'react-grid-layout/css/styles.css'
import 'react-resizable/css/styles.css'
import {
  DATATYPES,
  MACHINERY,
  NAME,
  fetchRoots,
  linkEntities,
  NATURAL_HEIGHT,
  NATURAL_WIDTH,
  SCREEN,
  fetchEntity,
  isDatatype,
  putAttribute,
  setArchived,
  type AttributeView,
  type Datatype,
} from '../api'
import { Editor } from '../Editor'

// THE ENTITY: add fields, and put them where you want them.
//
// A field renders from its DATATYPE and nothing else — there is no
// widget setting to configure. Adding one costs no UI work: it appears
// below the last, at the size its datatype wants.
//
// Positions are saved once, in an attribute called `screen` on the
// entity itself, so the layout is data like everything else — versioned,
// attributed, and undoable. Nothing is stored until you press Save,
// which is what makes dragging safe to experiment with.

const COLS = 12

export default function EntityTab({
  frag,
  onOpen,
}: {
  frag: string
  onOpen: (uuid: string) => void
}) {
  const qc = useQueryClient()
  const e = useQuery({ queryKey: ['entity', frag], queryFn: () => fetchEntity(frag) })
  const [design, setDesign] = useState(false)
  const [dragged, setDragged] = useState<LayoutItem[] | null>(null)
  const gridBox = useRef<HTMLDivElement>(null)
  const [gridWidth, setGridWidth] = useState(0)
  useEffect(() => {
    const box = gridBox.current
    if (!box) return
    const ro = new ResizeObserver(([entry]) =>
      setGridWidth(entry.contentRect.width),
    )
    ro.observe(box)
    setGridWidth(box.clientWidth)
    return () => ro.disconnect()
    // Re-observe when the fields first render: the box does not exist
    // while the entity is loading, so a mount-only effect would attach
    // to nothing and leave the grid at zero forever.
  }, [e.data?.uuid])
  const dirty = dragged !== null

  // A DECLARATION IS NOT A FIELD. An attribute carrying labels and no
  // content is how the model says what a thing IS — rendering it as a
  // prose editor meant one click on Save wrote `<p></p>` into it, and
  // because `labels_in` counts only content-less attributes, the entity
  // silently stopped being a role. Declarations show as chips in the
  // header instead, where they can be removed on purpose.
  const fields = useMemo(
    () =>
      (e.data?.attributes ?? []).filter(
        (a) =>
          !MACHINERY.includes(a.name) &&
          !(a.content === null && a.labels.length > 0),
      ),
    [e.data],
  )
  const saved = useMemo(() => {
    const s = (e.data?.attributes ?? []).find((a) => a.name === SCREEN)
    return Array.isArray(s?.content) ? (s.content as LayoutItem[]) : []
  }, [e.data])

  // THE LAYOUT MUST EXIST ON THE FIRST RENDER. Computing it in an
  // effect was wrong twice over: the grid mounts with nothing, defaults
  // every field to one cell, and never re-syncs — and a refetch (React
  // Query does one when the window regains focus) rebuilt it mid-drag,
  // throwing the arrangement away and clearing the unsaved flag with it.
  //
  // So it is derived during render, and a drag simply overrides it until
  // it is saved or the fields change underneath.
  const computed = useMemo(() => {
    let y = 0
    return fields.map((f) => {
      const found = saved.find((l) => l.i === f.uid)
      if (found) {
        y = Math.max(y, found.y + found.h)
        return found
      }
      const dt = (isDatatype(f.datatype) ? f.datatype : 'text') as Datatype
      const item = { i: f.uid, x: 0, y, w: NATURAL_WIDTH[dt], h: NATURAL_HEIGHT[dt] }
      y += item.h
      return item
    })
  }, [fields, saved])

  const layout = dragged ?? computed

  const save = useMutation({
    mutationFn: () => {
      const existing = (e.data?.attributes ?? []).find((a) => a.name === SCREEN)
      return putAttribute(frag, {
        uid: existing?.uid ?? null,
        name: SCREEN,
        datatype: 'json',
        content: layout.map(({ i, x, y, w, h }) => ({ i, x, y, w, h })),
      })
    },
    onSuccess: () => {
      setDragged(null)
      void qc.invalidateQueries({ queryKey: ['entity', frag] })
    },
  })

  const archive = useMutation({
    mutationFn: (a: boolean) => setArchived(frag, a),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['entity', frag] }),
  })

  if (e.isLoading) return <Loader size="sm" />
  if (e.error) return <Text c="red">{String(e.error)}</Text>
  if (!e.data) return null

  return (
    <>
      <Card withBorder padding="md" mb="md">
        <Group justify="space-between" wrap="nowrap">
          <Group gap="xs" wrap="nowrap">
            <Title order={4}>{e.data.name || '(unnamed)'}</Title>
            {e.data.labels.map((l) => (
              <Badge key={l.uuid} variant="light" style={{ cursor: 'pointer' }} onClick={() => onOpen(l.uuid)}>
                {l.name}
              </Badge>
            ))}
            {e.data.archived && <Badge color="gray">archived</Badge>}
          </Group>
          <Group gap="sm" wrap="nowrap">
            <Switch
              label="design"
              checked={design}
              onChange={(ev) => setDesign(ev.currentTarget.checked)}
            />
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
        <Text size="xs" c="dimmed" ff="monospace" mt={4}>
          {e.data.uuid}
        </Text>
      </Card>

      <Declare frag={frag} existing={e.data.labels.map((l) => l.uuid)} />
      <Link frag={frag} />
      <AddField frag={frag} />

      {/* THE GRID NEEDS A NUMBER, THE PAGE HAS A WIDTH. react-grid-layout
          computes column geometry in pixels, so a hardcoded width either
          overflows a narrow window or leaves a gutter on a wide one.
          Measure the column the fields actually sit in. */}
      <div ref={gridBox}>
      <GridLayout
        className="layout"
        layout={layout}
        width={gridWidth}
        gridConfig={{ cols: COLS, rowHeight: 28 }}
        // Dragging is OFF until you turn design on, and a click inside a
        // field must never start a drag — otherwise typing moves the box.
        dragConfig={{
          enabled: design,
          cancel: 'input,textarea,button,[contenteditable],.bn-container',
        }}
        resizeConfig={{ enabled: design }}
        // Only a deliberate move counts as a change: the grid also emits
        // this on mount, and treating that as an edit would light up
        // Save before anyone touched anything.
        onDragStop={(l) => setDragged([...l])}
        onResizeStop={(l) => setDragged([...l])}
      >
        {fields.map((f) => (
          <div key={f.uid}>
            <Field frag={frag} field={f} readOnly={design} />
          </div>
        ))}
      </GridLayout>
      </div>

      {e.data.links.length > 0 && (
        <Card withBorder padding="md" mt="md">
          <Text fw={600} size="sm" mb="xs">
            Links
          </Text>
          {e.data.links.map((l) => (
            <Group key={l.uid} gap="xs" py={2}>
              <Text size="xs" c="dimmed" ff="monospace" w={28}>
                {l.outbound ? '→' : '←'}
              </Text>
              <Text size="sm">{l.name}</Text>
              <Text size="sm" style={{ cursor: 'pointer' }} c="pelican" onClick={() => onOpen(l.other)}>
                {l.other_name || l.other.slice(0, 8)}
              </Text>
              {l.labels.map((lb) => (
                <Badge key={lb.uuid} size="xs" variant="light">
                  {lb.name}
                </Badge>
              ))}
            </Group>
          ))}
        </Card>
      )}
    </>
  )
}

/// Pick another entity by name. Everything in this model points at
/// something else — a label, the far end of a link — so choosing one is
/// the single most repeated act on this page.
function EntityPicker({
  label,
  description,
  exclude,
  value,
  onChange,
}: {
  label: string
  description?: string
  exclude: string
  value: string | null
  onChange: (v: string | null) => void
}) {
  const all = useQuery({ queryKey: ['roots', true], queryFn: () => fetchRoots(true) })
  const data = (all.data ?? [])
    .filter((e) => e.uuid !== exclude)
    .map((e) => ({ value: e.uuid, label: e.name || e.uuid.slice(0, 8) }))
  return (
    <Select
      label={label}
      description={description}
      data={data}
      value={value}
      onChange={onChange}
      searchable
      clearable
      nothingFoundMessage="no entity by that name"
      style={{ flex: 1 }}
    />
  )
}

/// SAY WHAT A THING IS. An attribute carrying labels and no content is
/// the whole mechanism — there is no separate "type" to set, and the
/// label is just another entity.
function Declare({ frag, existing }: { frag: string; existing: string[] }) {
  const qc = useQueryClient()
  const [label, setLabel] = useState<string | null>(null)
  const declare = useMutation({
    mutationFn: () =>
      putAttribute(frag, {
        name: 'is',
        datatype: 'text',
        content: null,
        labels: [...existing, label ?? ''],
      }),
    onSuccess: () => {
      setLabel(null)
      void qc.invalidateQueries({ queryKey: ['entity', frag] })
    },
  })
  return (
    <Card withBorder padding="sm" mb="md">
      <Group align="flex-end" gap="sm">
        <EntityPicker
          label="This is a…"
          description="a label is an entity; what it MEANS is a field on it"
          exclude={frag}
          value={label}
          onChange={setLabel}
        />
        <Button
          onClick={() => label && declare.mutate()}
          loading={declare.isPending}
          disabled={!label}
        >
          Label it
        </Button>
      </Group>
      {declare.error && (
        <Text c="red" size="sm" mt="xs">
          {String(declare.error)}
        </Text>
      )}
    </Card>
  )
}

/// CONNECT IT TO SOMETHING. The connection carries a name and its own
/// labels — what it MEANS is the label, not the name.
function Link({ frag }: { frag: string }) {
  const qc = useQueryClient()
  const [to, setTo] = useState<string | null>(null)
  const [name, setName] = useState('')
  const [label, setLabel] = useState<string | null>(null)
  const link = useMutation({
    mutationFn: () => linkEntities(frag, to ?? '', name.trim(), label ? [label] : []),
    onSuccess: () => {
      setTo(null)
      setName('')
      setLabel(null)
      void qc.invalidateQueries({ queryKey: ['entity', frag] })
    },
  })
  return (
    <Card withBorder padding="sm" mb="md">
      <Group align="flex-end" gap="sm">
        <TextInput
          label="Connection"
          placeholder="signs in with"
          value={name}
          onChange={(e) => setName(e.currentTarget.value)}
          style={{ flex: 1 }}
        />
        <EntityPicker label="to" exclude={frag} value={to} onChange={setTo} />
        <EntityPicker label="meaning" exclude={frag} value={label} onChange={setLabel} />
        <Button
          onClick={() => to && name.trim() && link.mutate()}
          loading={link.isPending}
          disabled={!to || !name.trim()}
        >
          Link
        </Button>
      </Group>
      {link.error && (
        <Text c="red" size="sm" mt="xs">
          {String(link.error)}
        </Text>
      )}
    </Card>
  )
}

/// One field, rendered by its datatype. Five datatypes, five controls,
/// and `text` and `json` share one.
function Field({
  frag,
  field,
  readOnly,
}: {
  frag: string
  field: AttributeView
  readOnly: boolean
}) {
  const qc = useQueryClient()
  const [draft, setDraft] = useState<string | number | undefined>(undefined)
  const write = useMutation({
    mutationFn: (content: unknown) =>
      putAttribute(frag, {
        uid: field.uid,
        name: field.name,
        datatype: field.datatype,
        content,
        labels: field.labels,
        options: field.options,
      }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['entity', frag] }),
  })

  const body = () => {
    if (readOnly) return <Text size="sm" c="dimmed">{preview(field)}</Text>
    switch (field.datatype) {
      case 'number':
        return (
          <NumberInput
            // CONTROLLED NEEDS onChange. Mantine treats a defined
            // `value` as controlled and defaults the setter to a no-op,
            // so a field that already held a number could not be typed
            // into at all — only empty ones appeared to work.
            value={draft ?? (typeof field.content === 'number' ? field.content : '')}
            onChange={setDraft}
            onBlur={(ev) => {
              // BLANK IS NOT ZERO. `Number('')` is 0 and passes a NaN
              // check, so clicking into an empty field and out again
              // used to write an explicit 0 onto the permanent record
              // with the operator's name on it.
              const raw = ev.currentTarget.value.trim()
              if (raw === '') return
              const n = Number(raw)
              // AND ONLY WHEN IT CHANGED. Tabbing through a field used
              // to append an identical version to a record that can
              // never be pruned.
              if (!Number.isNaN(n) && n !== field.content) write.mutate(n)
            }}
          />
        )
      case 'boolean':
        return (
          <Checkbox
            checked={field.content === true}
            onChange={(ev) => write.mutate(ev.currentTarget.checked)}
          />
        )
      case 'datetime':
        // `datetime-local` speaks LOCAL time; the store speaks UTC.
        // Slicing the stored instant into the widget and parsing it back
        // as local shifted the value by the browser's offset on every
        // open-and-blur, compounding each time.
        return (
          <TextInput
            type="datetime-local"
            defaultValue={toLocalInput(field.content)}
            onBlur={(ev) => {
              if (!ev.currentTarget.value) return
              const iso = new Date(ev.currentTarget.value).toISOString()
              // Comparing instants, not strings: the stored form and the
              // one we build differ in precision.
              const now = typeof field.content === 'string' ? Date.parse(field.content) : NaN
              if (Date.parse(iso) !== now) write.mutate(iso)
            }}
          />
        )
      case 'text':
        // THE NAME IS ONE LINE. Sending it through the prose editor made
        // Save rewrite it as `<p>DBA</p>`, and every list, title and
        // link chip then read the literal markup.
        if (field.name === NAME) {
          return (
            <TextInput
              defaultValue={typeof field.content === 'string' ? field.content : ''}
              onBlur={(ev) => {
                const v = ev.currentTarget.value.trim()
                if (v && v !== field.content) write.mutate(v)
              }}
            />
          )
        }
        return (
          <Editor
            value={field.content}
            json={false}
            onSave={(v) => write.mutate(v)}
          />
        )
      default:
        // json shares the editor; only the storage differs.
        return (
          <Editor
            value={field.content}
            json={field.datatype === 'json'}
            onSave={(v) => write.mutate(v)}
          />
        )
    }
  }

  return (
    <Card withBorder padding="xs" h="100%" style={{ overflow: 'auto' }}>
      <Group gap="xs" mb={4} wrap="nowrap">
        <Text size="xs" fw={600}>
          {field.name}
        </Text>
        <Text size="xs" c="dimmed" ff="monospace">
          {field.datatype}
        </Text>
      </Group>
      {body()}
      {write.error && (
        <Text c="red" size="xs" mt={4}>
          {String(write.error)}
        </Text>
      )}
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

function preview(f: AttributeView): string {
  if (f.content === null || f.content === undefined) return '—'
  if (typeof f.content === 'string') return f.content.replace(/<[^>]*>/g, '').slice(0, 80)
  return JSON.stringify(f.content).slice(0, 80)
}

/// Adding a field is a name and a datatype. Nothing else, ever.
function AddField({ frag }: { frag: string }) {
  const qc = useQueryClient()
  const [name, setName] = useState('')
  const [datatype, setDatatype] = useState<string | null>('text')
  const add = useMutation({
    mutationFn: () =>
      putAttribute(frag, { name: name.trim(), datatype: datatype ?? 'text', content: null }),
    onSuccess: () => {
      setName('')
      void qc.invalidateQueries({ queryKey: ['entity', frag] })
    },
  })
  return (
    <Card withBorder padding="sm" mb="md">
      <Group align="flex-end" gap="sm">
        <TextInput
          label="Add a field"
          placeholder="house_rules"
          value={name}
          onChange={(ev) => setName(ev.currentTarget.value)}
          style={{ flex: 1 }}
        />
        <Select
          label="Datatype"
          data={DATATYPES as unknown as string[]}
          value={datatype}
          onChange={setDatatype}
          // A field HAS a datatype — there is no such thing as one
          // without. Mantine lets you deselect by clicking the current
          // option, which left the control blank and the next field
          // silently defaulting.
          allowDeselect={false}
          w={140}
        />
        <ActionIcon
          size="lg"
          onClick={() => name.trim() && add.mutate()}
          loading={add.isPending}
          disabled={!name.trim()}
        >
          +
        </ActionIcon>
      </Group>
      {add.error && (
        <Text c="red" size="sm" mt="xs">
          {String(add.error)}
        </Text>
      )}
    </Card>
  )
}
