import { Container, Stack, Text, Title } from '@mantine/core'
import { Now } from './panels/Now'
import { Repos } from './panels/Repos'
import { Stats } from './panels/Stats'
import { Params } from './panels/Params'

// The tab, one panel per file under panels/: each reads its own route of
// recon's URL, so a panel is added without touching the others.
export default function App() {
  return (
    <Container size="xl" py="lg">
      <Stack gap="xl">
        <Stack gap={2}>
          <Title order={2}>Recon</Title>
          <Text c="dimmed" size="sm">
            The crawler: what it is doing now, each repository as its last crawl left it, what it did day by day, and the parameters it runs on.
          </Text>
        </Stack>
        <Now />
        <Repos />
        <Stats />
        <Params />
      </Stack>
    </Container>
  )
}
