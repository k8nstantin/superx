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
  MultiSelect,
  ScrollArea,
  Select,
  Switch,
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
import { Content } from '../Content'
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
  // §14: "the dictionary's type and label lists gain a show-archived
  // toggle that is OFF by default" — a term superseded by a better one
  // stops being offered without the dictionary losing what it meant.
  const [showArchived, setShowArchived] = useState(false)
  const labels = useQuery({
    queryKey: ['labels', showArchived],
    queryFn: () => fetchLabels(showArchived),
  })
  const types = useQuery({ queryKey: ['types'], queryFn: fetchTypes })
  const [forType, setForType] = useState<string | null>(null)
  // A label is argued about too — §3 gives it a thread of its own.
  const [forLabel, setForLabel] = useState<string | null>(null)

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
        <DefineLabel
          vocab={vocab.data}
          types={(types.data ?? []).map((t) => t.name)}
          onDone={refresh}
        />
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

      <Grid.Col span={{ base: 12, md: 7 }}>
        {forType && (
          <Card withBorder padding="md">
            <Content
              kind="type"
              uid={forType}
              title={`What belongs to the type '${forType}'`}
            />
          </Card>
        )}
      </Grid.Col>

      <Grid.Col span={{ base: 12, md: 5 }}>
        {forLabel && (
          <Content kind="label" uid={forLabel} title={`What belongs to '${forLabel}'`} />
        )}
      </Grid.Col>

      <Grid.Col span={12}>
        <LabelTable
          labels={labels.data ?? []}
          onPick={setForLabel}
          picked={forLabel}
          showArchived={showArchived}
          onShowArchived={setShowArchived}
        />
      </Grid.Col>
    </Grid>
  )
}

/// Defining a term and using it are one act, not two screens.
function DefineLabel({
  vocab,
  types,
  onDone,
}: {
  vocab: VocabularyView | undefined
  types: string[]
  onDone: () => void
}) {
  const [kind, setKind] = useState<string>('slot')
  const [key, setKey] = useState('')
  const [display, setDisplay] = useState('')
  const [semantics, setSemantics] = useState<string | null>(null)
  // Once the operator picks a semantics themselves, the kind stops
  // overwriting it — a default that fights a decision is worse than no
  // default.
  const [semanticsTouched, setSemanticsTouched] = useState(false)
  const [valueKind, setValueKind] = useState<string | null>(null)
  const [cardinality, setCardinality] = useState<string | null>('one')
  const [description, setDescription] = useState('')
  const [error, setError] = useState<string | null>(null)
  // Link labels: what may sit at each end, and whether it may close a
  // loop. A mislabelled edge is a wrong graph, and the graph is what
  // agents execute (§5.5).
  const [sourceTypes, setSourceTypes] = useState<string[]>([])
  const [targetTypes, setTargetTypes] = useState<string[]>([])
  const [inverse, setInverse] = useState('')
  const [acyclic, setAcyclic] = useState(false)

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
        source_types: isSlot ? null : sourceTypes,
        target_types: isSlot ? null : targetTypes,
        inverse: !isSlot && inverse.trim() ? inverse.trim() : null,
        acyclic: isSlot ? null : acyclic,
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
          // Slot and link have different closed vocabularies, so the
          // choice cannot carry over — and the default is free to
          // apply again.
          setSemantics(null)
          setSemanticsTouched(false)
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
      {isSlot ? (
        <>
          <Select
            label="Kind"
            description="prose becomes a versioned note; a value lives in the attributes bag"
            data={kindChoices}
            value={valueKind}
            onChange={(k) => {
              setValueKind(k)
              // A plain value is `data` — "a value you compute with" —
              // and prose read for background is `context`. Filled in
              // so the common case is name + kind; still a Select, so
              // a mandate or a directive is one click away.
              if (k && !semanticsTouched) {
                const prose = (vocab?.prose_kinds ?? []).includes(k)
                setSemantics(prose ? 'context' : 'data')
              }
            }}
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
      ) : (
        <>
          <MultiSelect
            label="Starts at"
            description="leave empty to accept anything — enforcement arrives with the declaration"
            data={types}
            value={sourceTypes}
            onChange={setSourceTypes}
            searchable
            mb="xs"
          />
          <MultiSelect
            label="Points at"
            description="a link that does not fit is refused when somebody tries to make it"
            data={types}
            value={targetTypes}
            onChange={setTargetTypes}
            searchable
            mb="xs"
          />
          <TextInput
            label="Reads the other way as"
            placeholder="{target} is required by {source}"
            value={inverse}
            onChange={(e) => setInverse(e.currentTarget.value)}
            mb="xs"
          />
          <Checkbox
            label="Acyclic — refuse a link that would close a loop"
            description="a cycle in an ordering label does not read oddly: the runner drops every task in it"
            checked={acyclic}
            onChange={(e) => setAcyclic(e.currentTarget.checked)}
            mb="xs"
          />
        </>
      )}
      {/* AFTER the kind, because the kind answers it most of the time.
          This is the question that matters when it is NOT `data`: a
          mandate binds and cannot be edited, a playbook is the role's
          own to refine — same kind, opposite meaning (§5.2). */}
      <Select
        label="How an agent must treat it"
        description={
          semantics
            ? SEMANTICS_HELP[semantics]
            : 'filled in from the kind — change it for a mandate, a directive or a secret'
        }
        data={semanticsChoices}
        value={semantics}
        onChange={(v) => {
          setSemantics(v)
          setSemanticsTouched(true)
        }}
        mb="xs"
      />
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
                    v &&
                    change.mutate({
                      label: s.label,
                      semantics_override: v,
                      required: null,
                      display_order: null,
                      active: null,
                      clear_semantics_override: null,
                    })
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
                      semantics_override: null,
                      display_order: null,
                      active: null,
                      clear_semantics_override: null,
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
                      clear_semantics_override: null,
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
              clear_semantics_override: null,
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

function LabelTable({
  labels,
  onPick,
  showArchived,
  onShowArchived,
  picked,
}: {
  labels: LabelView[]
  onPick: (key: string) => void
  showArchived: boolean
  onShowArchived: (v: boolean) => void
  picked: string | null
}) {
  return (
    <Card withBorder padding="md">
      <Group justify="space-between" mb="xs">
        <Title order={4}>The dictionary</Title>
        <Switch
          size="xs"
          label="Show archived"
          checked={showArchived}
          onChange={(e) => onShowArchived(e.currentTarget.checked)}
        />
      </Group>
      <ScrollArea.Autosize mah={460}>
        <Table striped withTableBorder>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Term</Table.Th>
              <Table.Th>Kind</Table.Th>
              <Table.Th>Treated as</Table.Th>
              <Table.Th>Storage / endpoints</Table.Th>
              <Table.Th>Means</Table.Th>
              <Table.Th w={90} />
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {labels.map((l) => (
              <Table.Tr
                key={`${l.label_kind}:${l.key}`}
                onClick={() => onPick(l.key)}
                style={{ cursor: 'pointer', opacity: l.archived ? 0.55 : 1 }}
                bg={picked === l.key ? 'rgba(122, 74, 143, 0.18)' : undefined}
              >
                <Table.Td>
                  <Group gap={6}>
                    <Text ff="monospace" size="sm">
                      {l.key}
                    </Text>
                    {l.archived && (
                      <Badge size="xs" variant="light" color="gray">
                        archived
                      </Badge>
                    )}
                  </Group>
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
                  {l.label_kind === 'link' ? (
                    <Text size="xs" ff="monospace">
                      {l.source_types.length ? l.source_types.join('|') : 'any'}
                      {' → '}
                      {l.target_types.length ? l.target_types.join('|') : 'any'}
                      {l.acyclic ? ' · acyclic' : ''}
                    </Text>
                  ) : (
                    <Text size="xs" ff="monospace">
                      {l.value_kind ?? '—'}
                      {l.cardinality ? ` · ${l.cardinality}` : ''}
                    </Text>
                  )}
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
