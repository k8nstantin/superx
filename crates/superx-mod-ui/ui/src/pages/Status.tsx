import { useQuery } from '@tanstack/react-query'
import { AreaChart, BarChart, DonutChart } from '@mantine/charts'
import { Card, Grid, Group, Table, Text, Title, Badge, SimpleGrid } from '@mantine/core'
import { fetchCharts, fetchStatus } from '../api'

const AGENT_COLORS = ['pelican.3', 'pelican.5', 'teal.5', 'orange.5', 'cyan.5']

export default function StatusPage() {
  const status = useQuery({ queryKey: ['status'], queryFn: fetchStatus, refetchInterval: 5000 })
  const charts = useQuery({ queryKey: ['charts'], queryFn: fetchCharts, refetchInterval: 10000 })

  const s = status.data
  const c = charts.data
  const active = s?.modules.filter((m) => m.lifecycle === 'active').length ?? 0

  return (
    <>
      <SimpleGrid cols={{ base: 2, md: 4 }} mb="md">
        <Stat label="OS" value={s ? s.os : '…'} />
        <Stat label="Agents" value={s ? String(s.agents) : '…'} />
        <Stat label="Modules active" value={s ? `${active}/${s.modules.length}` : '…'} />
        <Stat label="UI version" value={s ? `v${s.ui_version}` : '…'} />
      </SimpleGrid>

      <Grid mb="md">
        <Grid.Col span={{ base: 12, md: 6 }}>
          <Card withBorder>
            <Title order={5} mb="xs">
              Events per minute
            </Title>
            <AreaChart
              h={180}
              data={c?.events_per_minute ?? []}
              dataKey="t"
              series={[{ name: 'value', label: 'events', color: 'pelican.4' }]}
              withDots={false}
            />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, md: 3 }}>
          <Card withBorder>
            <Title order={5} mb="xs">
              Activity by agent
            </Title>
            <DonutChart
              h={180}
              data={(c?.per_agent ?? []).map((a, i) => ({
                name: a.name,
                value: Number(a.value),
                color: AGENT_COLORS[i % AGENT_COLORS.length],
              }))}
              withLabels
            />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, md: 3 }}>
          <Card withBorder>
            <Title order={5} mb="xs">
              Message roles
            </Title>
            <BarChart
              h={180}
              data={(c?.message_roles ?? []).map((r) => ({ role: r.name, count: Number(r.value) }))}
              dataKey="role"
              series={[{ name: 'count', color: 'pelican.3' }]}
            />
          </Card>
        </Grid.Col>
      </Grid>

      <Card withBorder>
        <Title order={5} mb="xs">
          Modules
        </Title>
        <Table striped highlightOnHover>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Module</Table.Th>
              <Table.Th>Kind</Table.Th>
              <Table.Th>Lifecycle</Table.Th>
              <Table.Th>Provisioned</Table.Th>
              <Table.Th>Version</Table.Th>
              <Table.Th>Module ID</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {(s?.modules ?? []).map((m) => (
              <Table.Tr key={m.module_id}>
                <Table.Td>{m.name}</Table.Td>
                <Table.Td>{m.kind}</Table.Td>
                <Table.Td>
                  <Badge color={m.lifecycle === 'active' ? 'green' : m.lifecycle === 'disabled' ? 'gray' : 'yellow'}>
                    {m.lifecycle}
                  </Badge>
                </Table.Td>
                <Table.Td>{m.provisioned == null ? '—' : m.provisioned ? 'yes' : 'no'}</Table.Td>
                <Table.Td>v{m.version}</Table.Td>
                <Table.Td>
                  <Text size="xs" c="dimmed" ff="monospace">
                    {m.module_id}
                  </Text>
                </Table.Td>
              </Table.Tr>
            ))}
          </Table.Tbody>
        </Table>
      </Card>
    </>
  )
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <Card withBorder>
      <Text size="xs" c="dimmed" tt="uppercase">
        {label}
      </Text>
      <Group>
        <Title order={3}>{value}</Title>
      </Group>
    </Card>
  )
}
