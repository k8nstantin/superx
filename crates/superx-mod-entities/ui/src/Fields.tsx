import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Alert,
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
import { fetchFields, setField } from './api'
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

  const rows = fields.data ?? []
  if (rows.length === 0) {
    return (
      <Text size="sm" c="dimmed">
        This type declares no values — only prose. Give it slots on the Dictionary
        page and they appear here.
      </Text>
    )
  }

  return (
    <Stack gap="sm">
      {rows.map((f) => (
        <FieldRow
          key={f.key}
          field={f}
          entityId={entityId}
          onSaved={() => {
            void qc.invalidateQueries({ queryKey: ['fields', entityId] })
            void qc.invalidateQueries({ queryKey: ['entity', entityId] })
          }}
        />
      ))}
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
  const [draft, setDraft] = useState<string>(field.value ?? '')
  const [error, setError] = useState<string | null>(null)
  const dirty = draft !== (field.value ?? '')

  const save = useMutation({
    mutationFn: () => setField(entityId, { key: field.key, value: draft }),
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
            {field.semantics && (
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
