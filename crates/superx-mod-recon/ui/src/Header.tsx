import { Anchor, Breadcrumbs, Group, Text } from '@mantine/core'

// Where you are, and the way back (#562). The core dashboard opens recon on
// its own URL, so the page carries the trail itself, as the core
// dashboard's header does: SuperX › Recon › Results › Landed. A crumb with
// somewhere to go is a link; the last is where you are. SuperX is a link
// only when the registry knows where the core dashboard is, never a dead one.

export type Crumb = { label: string; href?: string }

export function Header({ crumbs }: { crumbs: Crumb[] }) {
  const home = crumbs[0]?.href
  const logo = <img src="/logo.svg" alt="" width={26} height={26} style={{ borderRadius: 6, display: 'block' }} />
  return (
    <Group h="100%" px="lg" gap="sm" wrap="nowrap">
      {home ? (
        <Anchor href={home} aria-label="SuperX" title="SuperX">
          {logo}
        </Anchor>
      ) : (
        logo
      )}
      <Breadcrumbs
        separator="›"
        separatorMargin={8}
        styles={{
          root: { flexWrap: 'nowrap', overflow: 'hidden', minWidth: 0 },
          separator: { color: 'var(--mantine-color-dimmed)' },
        }}
      >
        {crumbs.map((c, i) => {
          const last = i === crumbs.length - 1
          const style = { whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' } as const
          if (last || !c.href)
            return (
              <Text key={c.label} fw={last ? 600 : 400} c={last ? undefined : 'dimmed'} style={style}>
                {c.label}
              </Text>
            )
          return (
            <Anchor key={c.label} href={c.href} c="dimmed" style={style}>
              {c.label}
            </Anchor>
          )
        })}
      </Breadcrumbs>
    </Group>
  )
}
