import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Badge,
  Button,
  Card,
  Grid,
  Group,
  ScrollArea,
  Table,
  Text,
  TextInput,
  Textarea,
  Title,
} from '@mantine/core'
import { addType, fetchTypes, typeColor } from '../api'
import { useBreadcrumb } from '../Breadcrumbs'

// The Types page (approved design): entity types only — no category
// concept. Relations are not types; they live in the link dialog.

export default function TypesPage() {
  useBreadcrumb([{ label: 'Types' }])
  const qc = useQueryClient()
  const types = useQuery({ queryKey: ['types'], queryFn: fetchTypes })
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [error, setError] = useState<string | null>(null)
  const create = useMutation({
    mutationFn: () => addType({ name, description: description.trim() ? description : null }),
    onSuccess: () => {
      setName('')
      setDescription('')
      setError(null)
      void qc.invalidateQueries({ queryKey: ['types'] })
    },
    onError: (e) => setError(String(e)),
  })
  return (
    <Grid gap="md">
      <Grid.Col span={{ base: 12, lg: 8 }}>
        <Card withBorder>
          <Title order={5} mb="sm">
            Entity types — the kinds of things in your graph, extensible at runtime
          </Title>
          <ScrollArea h={600}>
            <Table striped highlightOnHover>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th w={190}>Type</Table.Th>
                  <Table.Th>Description</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {(types.data ?? []).map((t) => (
                  <Table.Tr key={t.name}>
                    <Table.Td>
                      <Badge variant="filled" autoContrast color={typeColor(t.name)}>
                        {t.name}
                      </Badge>
                    </Table.Td>
                    <Table.Td>
                      <Text size="sm">
                        {t.description ?? '—'}
                        {t.system && (
                          <Text span size="xs" c="dimmed">
                            {' '}
                            · created for you when you write one — not in the New-entity list
                          </Text>
                        )}
                      </Text>
                    </Table.Td>
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </ScrollArea>
          <Text size="xs" c="dimmed" mt="xs">
            type names are create-once (immutable, UNIQUE)
          </Text>
        </Card>
      </Grid.Col>
      <Grid.Col span={{ base: 12, lg: 4 }}>
        <Card withBorder>
          <Title order={5} mb="sm">
            New type
          </Title>
          <TextInput
            label="Name — lowercase a-z 0-9 _"
            value={name}
            onChange={(e) => setName(e.currentTarget.value)}
            mb="sm"
          />
          <Textarea
            label="Description"
            rows={3}
            value={description}
            onChange={(e) => setDescription(e.currentTarget.value)}
          />
          {error && (
            <Text c="red.4" size="sm" mt="xs">
              {error}
            </Text>
          )}
          <Group justify="flex-end" mt="md">
            <Button
              onClick={() => create.mutate()}
              loading={create.isPending}
              disabled={!name.trim()}
            >
              Create type
            </Button>
          </Group>
          <Text size="xs" c="dimmed" mt="sm">
            a new type appears immediately in every type dropdown — create, filters. Relations
            (depends_on, describes, …) are not types: they live in the link dialog's relation
            picker.
          </Text>
        </Card>
      </Grid.Col>
    </Grid>
  )
}
