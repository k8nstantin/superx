import { useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import {
  Badge,
  Group,
  MultiSelect,
  Select,
  Text,
  type ComboboxData,
  type ComboboxItem,
} from '@mantine/core'
import { LABEL, fetchAllEntities, isLabel, type TreeNodeView } from './api'

// PICK AN ENTITY. Everything in this model points at something else — a
// label, the far end of a link — so choosing one is the single most
// repeated act in the interface, and what the picker OFFERS decides what
// can be built at all.
//
// EVERY ENTITY, FROM `all`. Both pickers used to read the ROOTS list, so
// the moment anything pointed at an entity it vanished from both. On the
// DBA example the link picker offered the five vocabulary words and none
// of the things: Backups, Runbooks, Nightly verify and Checksum step
// could not be chosen. The graph could never be built more than one
// level deep from the page that exists to build it.
//
// TWO KINDS OF QUESTION. "What is this?" and "what does this link mean?"
// want a LABEL — an entity carrying `label` (operator, 2026-09-03: "an
// entity that has label attached to it is a label"). "Link to what?"
// wants anything, things first. Vocabulary and things are one table;
// the picker is where they part.

type Kind = 'label' | 'any'

/// The options every picker on the page draws from — one query, one
/// shape, whether one entity is being chosen or several.
function useEntityOptions(kind: Kind, exclude: string[]) {
  const all = useQuery({ queryKey: ['all-entities'], queryFn: () => fetchAllEntities() })
  return useMemo(() => {
    const offered = (all.data ?? []).filter((n) => !exclude.includes(n.uuid))
    const item = (n: TreeNodeView): ComboboxItem => ({
      value: n.uuid,
      label: n.name || n.uuid.slice(0, 8),
    })
    const labels = offered.filter(isLabel).map(item)
    const things = offered.filter((n) => !isLabel(n)).map(item)
    const data: ComboboxData =
      kind === 'label'
        ? labels
        : [
            ...(things.length > 0 ? [{ group: 'entities', items: things }] : []),
            ...(labels.length > 0 ? [{ group: 'labels', items: labels }] : []),
          ]
    // An empty label picker is a fresh instance, not a typo: say where
    // the first word comes from.
    const nothing =
      kind === 'label'
        ? data.length === 0
          ? 'no labels yet — New label, on the Menu tab'
          : 'no label by that name'
        : 'no entity by that name'
    return { data, nothing, byUuid: new Map(offered.map((n) => [n.uuid, n])) }
  }, [all.data, exclude, kind])
}

/// The option shows what the entity IS beside its name, so `DBA role`
/// and a task called DBA can be told apart before choosing. In a label
/// picker every option carries `label`; that one chip says nothing and
/// is left off.
function optionRow(kind: Kind, byUuid: Map<string, TreeNodeView>) {
  return ({ option }: { option: ComboboxItem }) => {
    const node = byUuid.get(option.value)
    const chips = (node?.labels ?? []).filter((l) => kind !== 'label' || l.name !== LABEL)
    return (
      <Group gap={6} wrap="nowrap">
        <Text size="sm">{option.label}</Text>
        {chips.map((l) => (
          <Badge key={l.uuid} size="xs" variant="light">
            {l.name}
          </Badge>
        ))}
      </Group>
    )
  }
}

export function EntityPicker({
  label,
  description,
  placeholder,
  kind,
  exclude,
  value,
  onChange,
  size,
}: {
  label?: string
  description?: string
  placeholder?: string
  /** `label`: only entities that are labels. `any`: everything, things first. */
  kind: Kind
  /** uuids to leave out — the entity itself, the labels it already carries. */
  exclude: string[]
  value: string | null
  onChange: (v: string | null) => void
  size?: 'xs' | 'sm'
}) {
  const { data, nothing, byUuid } = useEntityOptions(kind, exclude)
  return (
    <Select
      label={label}
      description={description}
      placeholder={placeholder}
      size={size}
      data={data}
      value={value}
      onChange={onChange}
      searchable
      clearable
      nothingFoundMessage={nothing}
      renderOption={optionRow(kind, byUuid)}
      style={{ flex: 1 }}
    />
  )
}

/// Several labels at once — what a new field is born carrying. A field
/// can hold as many as an entity can (operator, 2026-09-03), so the
/// control that adds one takes a list, not a single choice.
export function LabelsPicker({
  label,
  description,
  placeholder,
  exclude,
  value,
  onChange,
}: {
  label?: string
  description?: string
  placeholder?: string
  exclude: string[]
  value: string[]
  onChange: (v: string[]) => void
}) {
  const { data, nothing, byUuid } = useEntityOptions('label', exclude)
  return (
    <MultiSelect
      label={label}
      description={description}
      placeholder={placeholder}
      data={data}
      value={value}
      onChange={onChange}
      searchable
      clearable
      hidePickedOptions
      nothingFoundMessage={nothing}
      renderOption={optionRow('label', byUuid)}
      style={{ flex: 1 }}
    />
  )
}
