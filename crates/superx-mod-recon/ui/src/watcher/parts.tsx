import { useState } from 'react'
import { Alert, Anchor, Badge, Loader, Modal, ScrollArea, Spoiler, Stack, Text } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchMemory, type Level, type MemoryFile } from '../api'
import { CANCEL, FAIL, UNKNOWN } from '../EChart'
import { day } from '../time'

// The Watcher's small parts (#566): a long text clamped, opening whole on a
// click; a report's level and kind in words; a memory folder's files, each
// opening whole.

/// A report's kind in words: stop not honoured for stop_not_honoured.
export const inWords = (kind: string) => kind.replaceAll('_', ' ')

const LEVEL_COLOUR: Record<Level, string> = { alert: FAIL, warn: CANCEL, info: UNKNOWN }

export function LevelBadge({ level }: { level: Level }) {
  return (
    <Badge color={LEVEL_COLOUR[level]} variant={level === 'info' ? 'light' : 'filled'} size="sm">
      {level}
    </Badge>
  )
}

/// A text shown clamped, opening whole on a click.
export function Clamped({ text, lines = 3, mono = false }: { text: string; lines?: number; mono?: boolean }) {
  return (
    <Spoiler maxHeight={lines * 20} showLabel="show all" hideLabel="show less">
      <Text fz="sm" ff={mono ? 'monospace' : undefined} style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>
        {text}
      </Text>
    </Spoiler>
  )
}

function MemoryText({ session, file }: { session: string; file: string }) {
  const text = useQuery({ queryKey: ['memory', session, file], queryFn: () => fetchMemory(session, file) })
  if (text.isLoading) return <Loader size="sm" />
  if (text.isError) return <Alert color="red" title="The file could not be read">{String(text.error)}</Alert>
  return (
    <Text fz="sm" ff="monospace" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>
      {text.data}
    </Text>
  )
}

/// A session's memory files, each with its description and when it last
/// changed; a file opens whole in a modal.
export function MemoryFiles({ session, files }: { session: string; files: MemoryFile[] }) {
  const [open, setOpen] = useState<string | null>(null)
  return (
    <Stack gap={2}>
      {files.map((f) => (
        <Text key={f.name} fz="sm">
          <Anchor component="button" type="button" fz="sm" onClick={() => setOpen(f.name)}>
            {f.name}
          </Anchor>
          {f.description ? <Text span c="dimmed" fz="sm">{` · ${f.description}`}</Text> : null}
          <Text span c="dimmed" fz="xs">{` · changed ${day(f.modified)}`}</Text>
        </Text>
      ))}
      <Modal opened={open != null} onClose={() => setOpen(null)} title={open} size="xl">
        <ScrollArea.Autosize mah="70vh">{open != null && <MemoryText session={session} file={open} />}</ScrollArea.Autosize>
      </Modal>
    </Stack>
  )
}
