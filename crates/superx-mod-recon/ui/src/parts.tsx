import type { ReactNode } from 'react'
import { Badge, Card, Group, Text, Title, Tooltip } from '@mantine/core'

// The parts superx-mod-ui's Status page builds its cards from (parts.tsx,
// #367), as recon's Results uses them: a number renders one way, a scope
// reads one way.

export const n = (v: number | bigint | null | undefined): number => (v == null ? 0 : Number(v))

export function fmtCompact(v: number | bigint | null | undefined): string {
  if (v == null) return '—'
  const x = Number(v)
  if (x >= 1e12) return `${(x / 1e12).toFixed(1)}T`
  if (x >= 1e9) return `${(x / 1e9).toFixed(1)}B`
  if (x >= 1e6) return `${(x / 1e6).toFixed(1)}M`
  if (x >= 1e3) return `${(x / 1e3).toFixed(1)}k`
  return String(x)
}

// ── scope: what window a card reads ───────────────────────────────

export type Scope = 'live' | 'range' | '24h' | 'all'

export function rangeLabel(range: string | null, windowMessages?: number | bigint | null): string {
  if (!range) return '…'
  if (range === 'window') return `newest ${fmtCompact(windowMessages)} messages`
  if (range === 'all') return 'all history'
  return `last ${range}`
}

export function ScopeBadge({ scope, range }: { scope: Scope; range: string | null }) {
  const label =
    scope === 'live'
      ? 'live'
      : scope === 'all'
        ? 'all time'
        : scope === '24h'
          ? 'last 24h'
          : range === 'window'
            ? 'window'
            : range === 'all'
              ? 'all history'
              : range ?? '…'
  const tip =
    scope === 'live'
      ? 'a reading of right now — it does not follow the range'
      : scope === 'all'
        ? 'every captured event, regardless of the range'
        : scope === '24h'
          ? 'the last twenty-four hours, regardless of the range'
          : 'follows the range selected above'
  return (
    <Tooltip label={tip} withArrow>
      <Badge
        size="xs"
        variant={scope === 'range' ? 'filled' : 'outline'}
        color={scope === 'live' ? 'teal' : scope === 'range' ? 'pelican' : 'gray'}
        style={{ cursor: 'help' }}
      >
        {label}
      </Badge>
    </Tooltip>
  )
}

/// A card with a title, its scope, and a one-line note.
export function Panel({
  title,
  scope,
  range,
  note,
  children,
  h,
  mb,
  p,
}: {
  title: string
  scope: Scope
  range: string | null
  note?: string
  children: ReactNode
  h?: string | number
  mb?: string | number
  p?: string | number
}) {
  return (
    <Card withBorder h={h} mb={mb} p={p}>
      {/* The title and its badge never shrink; the note sits beside them
          where there is room and takes its own line where there is not.
          Held on one line, a narrow card squeezed the badge to a sliver
          and drew the note over it (operator QA). */}
      <Group justify="space-between" mb="xs" wrap="wrap" gap="xs" style={{ rowGap: 2 }}>
        {/* A title longer than a narrow card wraps rather than pushing
            its badge off the card's edge. */}
        <Group gap="xs" wrap="nowrap" style={{ flex: '0 1 auto', minWidth: 0 }}>
          <Title order={5} style={{ minWidth: 0 }}>
            {title}
          </Title>
          <span style={{ flexShrink: 0, display: 'inline-flex' }}>
            <ScopeBadge scope={scope} range={range} />
          </span>
        </Group>
        {note && (
          <Text size="xs" c="dimmed" ta="right" lineClamp={2} style={{ flex: '1 1 180px', minWidth: 0 }}>
            {note}
          </Text>
        )}
      </Group>
      {children}
    </Card>
  )
}

