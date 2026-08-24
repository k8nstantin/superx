import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Alert,
  Badge,
  Button,
  Card,
  Checkbox,
  Grid,
  Group,
  ScrollArea,
  Select,
  Table,
  Text,
  TextInput,
  Textarea,
  Title,
} from '@mantine/core'
import {
  bindSlot,
  defineLabel,
  fetchLabels,
  fetchSlots,
  fetchTypes,
  fetchVocabulary,
} from '../api'
import { useBreadcrumb } from '../Breadcrumbs'
// The generated types ARE the contract — a hand-written duplicate here
// drifts from the module the moment either side changes.
import type { LabelView } from '../generated/LabelView'
import type { SlotView } from '../generated/SlotView'
import type { VocabularyView } from '../generated/VocabularyView'

// The dictionary, designable (#292).
//
// The spec's order is a dependency, not a convention: types -> labels ->
// entities, because each layer is required to interpret the next. So all
// three live on one surface — define the term, then say which types carry
// it, and only then does an entity of that type mean anything when read.
//
// Every closed vocabulary shown here comes from /api/vocabulary. Nothing
// on this page hardcodes what a semantics or a kind may be: the
// dictionary owns that, and a second copy in the frontend would rot the
// moment the substrate's changed.

/// How a reader must TREAT a slot — the part that is not obvious from the
/// kind, and the reason the dictionary exists at all.
const SEMANTICS_HELP: Record<string, string> = {
  binding: 'the envelope — obey it, you may not edit it, you can never complete it',
  directive: 'the assignment — do it; you may complete it, and you may refuse it',
  context: 'background; read, do not act on directly',
  guidance: 'advisory; yours to refine',
  dialogue: 'collaboration; may be addressed to you',
  data: 'a value you compute with',
  secret: 'resolve at use, never print',
  composition: 'the target is part of the source',
  ordering: 'the source waits for the target',
  sequence: 'the readable forward chain',
  reach: 'confers capability — what an audit reads',
  reference: 'context only; confers nothing',
  governance: 'oversight',
}

function semanticsColor(s: string): string {
  if (s === 'binding') return 'red'
  if (s === 'directive') return 'orange'
  if (s === 'secret') return 'grape'
  if (s === 'data') return 'cyan'
  return 'pelican'
}

export default function DictionaryPage() {
  useBreadcrumb([{ label: 'Dictionary' }])
  const qc = useQueryClient()
  const vocab = useQuery({ queryKey: ['vocabulary'], queryFn: fetchVocabulary })
  const labels = useQuery({ queryKey: ['labels'], queryFn: () => fetchLabels(false) })
  const types = useQuery({ queryKey: ['types'], queryFn: fetchTypes })
  const [forType, setForType] = useState<string | null>(null)

  const slots = useQuery({
    queryKey: ['slots', forType],
    queryFn: () => fetchSlots(forType as string),
    enabled: !!forType,
  })

  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['labels'] })
    void qc.invalidateQueries({ queryKey: ['slots'] })
    void qc.invalidateQueries({ queryKey: ['vocabulary'] })
  }

  return (
    <Grid gap="md">
      <Grid.Col span={{ base: 12, md: 5 }}>
        <DefineLabel vocab={vocab.data} onDone={refresh} />
      </Grid.Col>

      <Grid.Col span={{ base: 12, md: 7 }}>
        <Card withBorder padding="md" mb="md">
          <Group justify="space-between" mb="xs">
            <Title order={4}>What a type carries</Title>
            {vocab.data && (
              <Text size="xs" c="dimmed" ff="monospace">
                dictionary revision {vocab.data.revision}
              </Text>
            )}
          </Group>
          <Text size="sm" c="dimmed" mb="sm">
            A type that declares nothing is inert — there is nowhere to put a fact,
            so nothing can be said about one of its entities and nothing can act on
            it.
          </Text>
          <Select
            label="Type"
            placeholder="pick a type to design"
            data={(types.data ?? []).map((t) => t.name)}
            value={forType}
            onChange={setForType}
            searchable
            mb="sm"
          />
          {forType && (
            <SlotEditor
              type={forType}
              slots={slots.data ?? []}
              labels={(labels.data ?? []).filter((l) => l.label_kind === 'slot')}
              slotSemantics={vocab.data?.slot_semantics ?? []}
              onDone={refresh}
            />
          )}
        </Card>
      </Grid.Col>

      <Grid.Col span={12}>
        <LabelTable labels={labels.data ?? []} />
      </Grid.Col>
    </Grid>
  )
}

/// Defining a term and using it are one act, not two screens.
function DefineLabel({
  vocab,
  onDone,
}: {
  vocab: VocabularyView | undefined
  onDone: () => void
}) {
  const [kind, setKind] = useState<string>('slot')
  const [key, setKey] = useState('')
  const [display, setDisplay] = useState('')
  const [semantics, setSemantics] = useState<string | null>(null)
  const [valueKind, setValueKind] = useState<string | null>(null)
  const [cardinality, setCardinality] = useState<string | null>('one')
  const [description, setDescription] = useState('')
  const [error, setError] = useState<string | null>(null)

  const isSlot = kind === 'slot'
  const semanticsChoices = (isSlot ? vocab?.slot_semantics : vocab?.link_semantics) ?? []
  // Prose kinds become note chains; value kinds live in the attributes
  // bag. The declaration looks identical either way — the spec calls that
  // an implementation detail, not something to know when adding a field.
  const kindChoices = [...(vocab?.prose_kinds ?? []), ...(vocab?.value_kinds ?? [])]

  const save = useMutation({
    mutationFn: () =>
      defineLabel({
        key: key.trim(),
        label_kind: kind,
        display: display.trim() ? display.trim() : null,
        semantics: semantics ?? '',
        description: description.trim() ? description.trim() : null,
        cardinality: isSlot ? cardinality : null,
        value_kind: isSlot ? valueKind : null,
      }),
    onSuccess: () => {
      setKey('')
      setDisplay('')
      setDescription('')
      setError(null)
      onDone()
    },
    onError: (e) => setError(String(e)),
  })

  return (
    <Card withBorder padding="md">
      <Title order={4} mb="xs">
        Define a term
      </Title>
      <Text size="sm" c="dimmed" mb="sm">
        A product can carry both a <b>description</b> and a <b>spec</b> — same kind,
        same storage. The label is the entire difference, and it is what tells a
        reader which is the contract and which is the summary.
      </Text>

      <Select
        label="Kind of label"
        description="what an entity carries, or how entities connect"
        data={[
          { value: 'slot', label: 'slot — what an entity carries' },
          { value: 'link', label: 'link — how entities connect' },
        ]}
        value={kind}
        onChange={(v) => {
          setKind(v ?? 'slot')
          setSemantics(null)
        }}
        mb="xs"
      />
      <TextInput
        label="Key"
        description="lowercase, one spelling per term — this is the name everything reads"
        placeholder="max_notional"
        value={key}
        onChange={(e) => setKey(e.currentTarget.value)}
        mb="xs"
      />
      <TextInput
        label="Display"
        placeholder={key || 'Max notional'}
        value={display}
        onChange={(e) => setDisplay(e.currentTarget.value)}
        mb="xs"
      />
      <Select
        label="Semantics"
        description={semantics ? SEMANTICS_HELP[semantics] : 'how an agent must TREAT it'}
        data={semanticsChoices}
        value={semantics}
        onChange={setSemantics}
        mb="xs"
      />
      {isSlot && (
        <>
          <Select
            label="Kind"
            description="prose becomes a versioned note; a value lives in the attributes bag"
            data={kindChoices}
            value={valueKind}
            onChange={setValueKind}
            mb="xs"
          />
          <Select
            label="Cardinality"
            description="one is amended in place; many is a thread"
            data={vocab?.cardinalities ?? []}
            value={cardinality}
            onChange={setCardinality}
            mb="xs"
          />
        </>
      )}
      <Textarea
        label="Description"
        placeholder="what this term means, for whoever reads it next"
        value={description}
        onChange={(e) => setDescription(e.currentTarget.value)}
        autosize
        minRows={2}
        mb="sm"
      />
      {error && (
        <Alert color="red" mb="sm" title="Refused">
          {error}
        </Alert>
      )}
      <Button
        onClick={() => save.mutate()}
        loading={save.isPending}
        disabled={!key.trim() || !semantics}
      >
        Define
      </Button>
      <Text size="xs" c="dimmed" mt="xs">
        Redefining an existing term appends a version — the old meaning stays
        readable, because every entity written under it was written under it.
      </Text>
    </Card>
  )
}

/// The slots a type carries, in the order they read.
function SlotEditor({
  type,
  slots,
  labels,
  slotSemantics,
  onDone,
}: {
  type: string
  slots: SlotView[]
  labels: LabelView[]
  slotSemantics: string[]
  onDone: () => void
}) {
  const [add, setAdd] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const change = useMutation({
    mutationFn: (req: Parameters<typeof bindSlot>[1]) => bindSlot(type, req),
    onSuccess: () => {
      setError(null)
      setAdd(null)
      onDone()
    },
    onError: (e) => setError(String(e)),
  })

  const unbound = labels.filter((l) => !slots.some((s) => s.label === l.key))

  return (
    <>
      {error && (
        <Alert color="red" mb="sm" title="Refused">
          {error}
        </Alert>
      )}
      <Table striped withTableBorder mb="sm">
        <Table.Thead>
          <Table.Tr>
            <Table.Th>Slot</Table.Th>
            <Table.Th>Kind</Table.Th>
            <Table.Th>Treated as</Table.Th>
            <Table.Th>Required</Table.Th>
            <Table.Th />
          </Table.Tr>
        </Table.Thead>
        <Table.Tbody>
          {slots.map((s) => (
            <Table.Tr key={s.label} opacity={s.active ? 1 : 0.5}>
              <Table.Td>
                <Text ff="monospace" size="sm">
                  {s.label}
                </Text>
                <Text size="xs" c="dimmed">
                  {s.cardinality ?? '—'}
                </Text>
              </Table.Td>
              <Table.Td>
                <Text size="xs" ff="monospace">
                  {s.value_kind ?? '—'}
                </Text>
              </Table.Td>
              <Table.Td>
                <Select
                  size="xs"
                  data={slotSemantics}
                  value={s.semantics}
                  onChange={(v) =>
                    v && change.mutate({ label: s.label, semantics_override: v, required: s.required, display_order: null, active: null })
                  }
                  w={130}
                />
                {s.semantics_override && (
                  <Text size="xs" c="dimmed">
                    overridden for this type
                  </Text>
                )}
              </Table.Td>
              <Table.Td>
                <Checkbox
                  checked={s.required}
                  onChange={(e) =>
                    change.mutate({
                      label: s.label,
                      required: e.currentTarget.checked,
                      semantics_override: s.semantics_override,
                      display_order: null,
                      active: null,
                    })
                  }
                />
              </Table.Td>
              <Table.Td>
                <Button
                  size="xs"
                  variant="subtle"
                  color={s.active ? 'red' : 'green'}
                  onClick={() =>
                    change.mutate({
                      label: s.label,
                      active: !s.active,
                      required: null,
                      semantics_override: null,
                      display_order: null,
                    })
                  }
                >
                  {s.active ? 'Retire' : 'Restore'}
                </Button>
              </Table.Td>
            </Table.Tr>
          ))}
          {slots.length === 0 && (
            <Table.Tr>
              <Table.Td colSpan={5}>
                <Text size="sm" c="dimmed">
                  This type declares nothing — nothing can be attached to one of its
                  entities, so nothing can act on it.
                </Text>
              </Table.Td>
            </Table.Tr>
          )}
        </Table.Tbody>
      </Table>

      <Group>
        <Select
          placeholder="add a slot this type carries"
          data={unbound.map((l) => ({ value: l.key, label: `${l.key} — ${l.display}` }))}
          value={add}
          onChange={setAdd}
          searchable
          w={280}
        />
        <Button
          disabled={!add}
          loading={change.isPending}
          onClick={() =>
            add &&
            change.mutate({
              label: add,
              required: false,
              semantics_override: null,
              display_order: null,
              active: null,
            })
          }
        >
          Add
        </Button>
      </Group>
      <Text size="xs" c="dimmed" mt="xs">
        Retiring a slot never erases it: entities written while it stood still hold
        values in it, and a declaration that vanishes makes those look like junk.
      </Text>
    </>
  )
}

function LabelTable({ labels }: { labels: LabelView[] }) {
  return (
    <Card withBorder padding="md">
      <Title order={4} mb="xs">
        The dictionary
      </Title>
      <ScrollArea.Autosize mah={460}>
        <Table striped withTableBorder>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Term</Table.Th>
              <Table.Th>Kind</Table.Th>
              <Table.Th>Treated as</Table.Th>
              <Table.Th>Storage</Table.Th>
              <Table.Th>Means</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {labels.map((l) => (
              <Table.Tr key={`${l.label_kind}:${l.key}`}>
                <Table.Td>
                  <Text ff="monospace" size="sm">
                    {l.key}
                  </Text>
                </Table.Td>
                <Table.Td>
                  <Badge size="xs" variant="light">
                    {l.label_kind}
                  </Badge>
                </Table.Td>
                <Table.Td>
                  <Badge size="xs" color={semanticsColor(l.semantics)} variant="light">
                    {l.semantics}
                  </Badge>
                  <Text size="xs" c="dimmed">
                    {SEMANTICS_HELP[l.semantics] ?? ''}
                  </Text>
                </Table.Td>
                <Table.Td>
                  <Text size="xs" ff="monospace">
                    {l.value_kind ?? '—'}
                    {l.cardinality ? ` · ${l.cardinality}` : ''}
                  </Text>
                </Table.Td>
                <Table.Td>
                  <Text size="sm">{l.description ?? '—'}</Text>
                </Table.Td>
              </Table.Tr>
            ))}
          </Table.Tbody>
        </Table>
      </ScrollArea.Autosize>
    </Card>
  )
}
