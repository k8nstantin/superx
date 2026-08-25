import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Alert,
  Autocomplete,
  Badge,
  Button,
  Checkbox,
  Group,
  NumberInput,
  Select,
  Stack,
  Text,
  TextInput,
  Tooltip,
} from '@mantine/core'
import {
  fetchAddableFields,
  fetchFields,
  fetchLabels,
  fetchVocabulary,
  promoteField,
  setField,
} from './api'
import type { FieldView } from './generated/FieldView'

// Declared fields, rendered as what they are (#294).
//
// The entity page used to show `attributes_json` as raw text in a
// textarea — so everything the dictionary declares (kind, semantics,
// cardinality, required) was invisible exactly where entities are read
// and edited, and the same textarea was a way round every rule the field
// checks enforce.
//
// A key the type no longer declares still shows, marked. Reads never
// fail, and a removed declaration must not make existing data look like
// junk.

/// How an agent must TREAT this value — the part the kind does not say.
const SEMANTICS_HELP: Record<string, string> = {
  binding: 'the envelope — obey it, you may not edit it',
  directive: 'the assignment — do it, and you may refuse it',
  context: 'background; read, do not act on directly',
  guidance: 'advisory; yours to refine',
  dialogue: 'collaboration; may be addressed to you',
  data: 'a value you compute with',
  secret: 'resolve at use, never print',
}

export function Fields({ entityId }: { entityId: string }) {
  const qc = useQueryClient()
  const fields = useQuery({
    queryKey: ['fields', entityId],
    queryFn: () => fetchFields(entityId),
  })

  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['fields', entityId] })
    void qc.invalidateQueries({ queryKey: ['addable', entityId] })
    void qc.invalidateQueries({ queryKey: ['entity', entityId] })
    void qc.invalidateQueries({ queryKey: ['slots'] })
  }

  const rows = fields.data ?? []

  return (
    <Stack gap="sm">
      {rows.length === 0 && (
        <Text size="sm" c="dimmed">
          Nothing here yet. Add a field below — it lands on THIS entity, and you
          can promote it to the type when every one of them should carry it.
        </Text>
      )}
      {rows.map((f) => (
        <FieldRow key={f.key} field={f} entityId={entityId} onSaved={refresh} />
      ))}
      <AddField entityId={entityId} onAdded={refresh} />
    </Stack>
  )
}

/// §6: "Seed, then design … add fields and label them, FROM LABELS
/// DESIGNED AHEAD. You never invent a label inline — you pick one from
/// the dictionary, or you add it to the dictionary."
///
/// So this is a picker over the dictionary, never a free-text key. A
/// label the type already carries just gets filled in; one it does not
/// is an AD HOC field on this entity alone, which the row then offers
/// to promote.
function AddField({ entityId, onAdded }: { entityId: string; onAdded: () => void }) {
  // The closed set of datatypes comes from the substrate — a second
  // copy in the frontend would rot the moment the dictionary changes.
  const vocab = useQuery({ queryKey: ['vocabulary'], queryFn: fetchVocabulary })
  const kinds = vocab.data?.value_kinds ?? []
  // Every slot label in the dictionary, prose included: labelling a
  // NUMBER as `mandate` is odd but it is the operator's odd, and the
  // meaning it borrows is the semantics, not the storage.
  const dictionary = useQuery({ queryKey: ['labels', false], queryFn: () => fetchLabels(false) })
  const labels = (dictionary.data ?? [])
    .filter((l) => l.label_kind === 'slot')
    .map((l) => l.key)
  const offers = useQuery({
    queryKey: ['addable', entityId],
    queryFn: () => fetchAddableFields(entityId),
  })
  const [key, setKey] = useState('')
  const [kind, setKind] = useState('string')
  // ALWAYS OPTIONAL, and what makes the field actionable. Without it
  // the field is yours: named, typed, and an agent does nothing with
  // it. With it, the field takes that term's semantics — a mandate
  // binds, a directive may be refused, a secret is never printed.
  const [label, setLabel] = useState<string | null>(null)
  const [value, setValue] = useState('')
  const [error, setError] = useState<string | null>(null)
  // A name the dictionary already knows brings its own kind — you do
  // not get to redeclare what a term means by filling in a form (§5.6:
  // changing what a label means carries the tightest gate in the
  // system).
  const known = (offers.data ?? []).find((o) => o.key === key.trim())

  const save = useMutation({
    mutationFn: () =>
      setField(entityId, {
        key: key.trim(),
        value,
        // Only when it is NEW: naming an existing field must not carry
        // a kind or a label that could disagree with what it already
        // declares.
        value_kind: known ? null : kind,
        label: known ? null : label,
      }),
    onSuccess: () => {
      setKey('')
      setValue('')
      setLabel(null)
      setError(null)
      onAdded()
    },
    onError: (e) => setError(String(e)),
  })

  return (
    <Stack gap={4} mt="md">
      <Text size="sm" fw={600}>
        Add a field
      </Text>
      <Text size="xs" c="dimmed">
        Name it and say what kind of value it holds. A label is optional — without
        one the field is yours for reference; with one it takes that term's
        meaning and an agent acts on it. A name the dictionary already knows keeps
        its own kind and label.
      </Text>
      <Group align="flex-end" gap="xs">
        <Autocomplete
          label="Name"
          placeholder="branch, host, owner…"
          w={200}
          value={key}
          data={(offers.data ?? []).map((o) => o.key)}
          onChange={(k) => {
            setKey(k)
            setError(null)
          }}
        />
        <Select
          label="Datatype"
          w={140}
          data={kinds}
          value={known ? known.value_kind : kind}
          disabled={!!known}
          onChange={(k) => setKind(k ?? 'string')}
        />
        <Select
          label="Label — optional"
          description="what makes it actionable"
          placeholder="none"
          w={190}
          clearable
          searchable
          disabled={!!known}
          data={labels}
          value={label}
          onChange={setLabel}
        />
        <TextInput
          label="Value"
          description={known?.description || undefined}
          w={220}
          value={value}
          onChange={(e) => setValue(e.currentTarget.value)}
        />
        <Button
          disabled={!key.trim() || !value.trim()}
          loading={save.isPending}
          onClick={() => save.mutate()}
        >
          Add
        </Button>
      </Group>
      {known && !known.on_the_type && (
        <Text size="xs" c="dimmed">
          {key} is already a known field, and this type does not carry it — it
          lands on this entity alone, and Promote gives every entity of the type
          the slot.
        </Text>
      )}
      {error && (
        <Text size="xs" c="red.4">
          {error}
        </Text>
      )}
    </Stack>
  )
}

function FieldRow({
  field,
  entityId,
  onSaved,
}: {
  field: FieldView
  entityId: string
  onSaved: () => void
}) {
  // Promoting BINDS the slot to the type; it does not make it required,
  // and §7 is explicit that making a field required does not
  // retroactively invalidate what already exists.
  const promote = useMutation({
    mutationFn: () => promoteField(entityId, field.key),
    onSuccess: onSaved,
  })
  const [draft, setDraft] = useState<string>(field.value ?? '')
  const [error, setError] = useState<string | null>(null)
  const dirty = draft !== (field.value ?? '')

  const save = useMutation({
    mutationFn: () =>
      // Editing an existing field never carries a kind: the label
      // already declares one, and a form must not redeclare it.
      setField(entityId, {
        key: field.key,
        value: draft,
        // Editing carries neither: the label already declares a kind
        // and a meaning, and a form must not redeclare either.
        value_kind: null,
        label: null,
      }),
    onSuccess: () => {
      setError(null)
      onSaved()
    },
    // The refusal is the useful part: it says which rule the value broke.
    onError: (e) => setError(String(e).replace(/^Error:\s*/, '')),
  })

  return (
    <div>
      <Group gap="xs" mb={4} wrap="nowrap">
        <Text size="sm" fw={600} ff="monospace">
          {field.key}
        </Text>
        {field.required && (
          <Badge size="xs" color="orange" variant="light">
            required
          </Badge>
        )}
        {field.undeclared ? (
          <Tooltip label="Nothing declares this key any more — it still reads, and it is not editable here">
            <Badge size="xs" color="gray" variant="light">
              no longer declared
            </Badge>
          </Tooltip>
        ) : (
          <>
            <Badge size="xs" variant="light" ff="monospace">
              {field.value_kind}
            </Badge>
            {/* `data` is the DEFAULT for every value field — "a value
                you compute with" — so the badge said nothing and made
                `budget NUMBER DATA` look like two things to understand
                instead of one. The other semantics change how an agent
                must TREAT the value (§5.2), so those still show. */}
            {field.semantics && field.semantics !== 'data' && (
              <Tooltip label={SEMANTICS_HELP[field.semantics] ?? field.semantics}>
                <Badge
                  size="xs"
                  variant="light"
                  color={field.semantics === 'secret' ? 'grape' : 'pelican'}
                >
                  {field.semantics}
                </Badge>
              </Tooltip>
            )}
            {/* §6: added ad hoc to THIS entity, and promotable to the
                type when every one of them should carry it. Marked, so
                it is visibly an exception rather than looking like part
                of the type. */}
            {field.label && (
              <Tooltip label={`takes its meaning from the '${field.label}' label — that is what makes it actionable`}>
                <Badge size="xs" variant="light" color="grape">
                  {field.label}
                </Badge>
              </Tooltip>
            )}
            {field.ad_hoc && (
              <>
                <Tooltip label="On this entity only — its type does not carry this slot">
                  <Badge size="xs" color="cyan" variant="light">
                    ad hoc
                  </Badge>
                </Tooltip>
                <Button
                  size="compact-xs"
                  variant="subtle"
                  loading={promote.isPending}
                  onClick={() => promote.mutate()}
                >
                  Promote to type
                </Button>
              </>
            )}
          </>
        )}
      </Group>

      {field.undeclared ? (
        <Text size="sm" ff="monospace" c="dimmed">
          {field.value ?? '—'}
        </Text>
      ) : (
        <Group gap="xs" align="flex-start" wrap="nowrap">
          <Input field={field} value={draft} onChange={setDraft} />
          <Button
            size="xs"
            disabled={!dirty}
            loading={save.isPending}
            onClick={() => save.mutate()}
          >
            Save
          </Button>
        </Group>
      )}

      {error && (
        <Alert color="red" mt={6} p="xs">
          <Text size="xs">{error}</Text>
        </Alert>
      )}
    </div>
  )
}

/// One input per kind. The kind is a declaration, so the control follows
/// from it rather than from a guess about the value.
function Input({
  field,
  value,
  onChange,
}: {
  field: FieldView
  value: string
  onChange: (v: string) => void
}) {
  const kind = field.value_kind

  if (kind === 'boolean') {
    return (
      <Checkbox
        checked={value === 'true'}
        onChange={(e) => onChange(e.currentTarget.checked ? 'true' : 'false')}
        label={value === 'true' ? 'true' : 'false'}
      />
    )
  }

  if (kind === 'enum') {
    return (
      <Select
        size="xs"
        w="100%"
        data={field.options}
        value={value || null}
        onChange={(v) => onChange(v ?? '')}
        placeholder={field.options.length ? 'choose' : 'this enum declares no options'}
        disabled={field.options.length === 0}
      />
    )
  }

  if (kind === 'number' || kind === 'integer') {
    return (
      <NumberInput
        size="xs"
        w="100%"
        value={value === '' ? '' : Number(value)}
        onChange={(v) => onChange(v === '' ? '' : String(v))}
        allowDecimal={kind === 'number'}
        placeholder={kind}
      />
    )
  }

  if (kind === 'secret_ref') {
    return (
      <TextInput
        size="xs"
        w="100%"
        value={value}
        onChange={(e) => onChange(e.currentTarget.value)}
        placeholder="env:NAME · keychain:ITEM · vault:ID"
        // The rule, where it is needed rather than in a refusal after the
        // fact: what goes here is where to FIND the secret.
        description="a pointer, never the secret itself"
      />
    )
  }

  return (
    <TextInput
      size="xs"
      w="100%"
      value={value}
      onChange={(e) => onChange(e.currentTarget.value)}
      placeholder={
        kind === 'datetime' ? '2026-08-24T09:00:00Z' : kind === 'url' ? 'https://…' : kind
      }
    />
  )
}
