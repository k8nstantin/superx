import { useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Badge, Group, Select, Text, type ComboboxData, type ComboboxItem } from '@mantine/core'
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

export function EntityPicker({
  label,
  description,
  kind,
  exclude,
  value,
  onChange,
}: {
  label: string
  description?: string
  /** `label`: only entities that are labels. `any`: everything, things first. */
  kind: 'label' | 'any'
  /** uuids to leave out — the entity itself, the labels it already carries. */
  exclude: string[]
  value: string | null
  onChange: (v: string | null) => void
}) {
  const all = useQuery({ queryKey: ['all-entities'], queryFn: () => fetchAllEntities() })

  const { data, byUuid } = useMemo(() => {
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
    return { data, byUuid: new Map(offered.map((n) => [n.uuid, n])) }
  }, [all.data, exclude, kind])

  // An empty label picker is a fresh instance, not a typo: say where the
  // first word comes from.
  const nothing =
    kind === 'label'
      ? byUuid.size > 0 && data.length === 0
        ? 'no labels yet — New label, on the Menu tab'
        : 'no label by that name'
      : 'no entity by that name'

  return (
    <Select
      label={label}
      description={description}
      data={data}
      value={value}
      onChange={onChange}
      searchable
      clearable
      nothingFoundMessage={nothing}
      // The option shows what the entity IS beside its name, so `DBA
      // role` and a task called DBA can be told apart before choosing.
      // In a label picker every option carries `label`; that one chip
      // says nothing and is left off.
      renderOption={({ option }) => {
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
      }}
      style={{ flex: 1 }}
    />
  )
}
