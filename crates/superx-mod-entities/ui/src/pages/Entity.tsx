import { useMemo, useState } from 'react'
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
  const dirty = dragged !== null

  const fields = useMemo(
    () => (e.data?.attributes ?? []).filter((a) => a.name !== SCREEN),
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

      <AddField frag={frag} />

      <GridLayout
        className="layout"
        layout={layout}
        width={1100}
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
            value={typeof field.content === 'number' ? field.content : undefined}
            onBlur={(ev) => {
              const n = Number(ev.currentTarget.value)
              if (!Number.isNaN(n)) write.mutate(n)
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
        return (
          <TextInput
            type="datetime-local"
            defaultValue={typeof field.content === 'string' ? field.content.slice(0, 16) : ''}
            onBlur={(ev) => ev.currentTarget.value && write.mutate(new Date(ev.currentTarget.value).toISOString())}
          />
        )
      default:
        // text AND json: the same editor. Only the storage differs.
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
