import { useState } from 'react'
import { Button, Checkbox, Group, Popover, SegmentedControl, Stack, Switch, Text } from '@mantine/core'
import type { SessionView } from './generated/SessionView'
import {
  DEFAULT_EXPORT,
  TRIMMED_TOOL_LINES,
  collectSession,
  download,
  exportFileName,
  formatSession,
  type ExportOptions,
} from './exportSession'

// The session view's export (#442): the whole session as a text file, in
// the shape the reader picks. The last choice is remembered per viewer.

const EXPORT_KEY = 'superx.export.options'

function storedOptions(): ExportOptions {
  try {
    const v = JSON.parse(localStorage.getItem(EXPORT_KEY) ?? 'null') as Partial<ExportOptions> | null
    return { ...DEFAULT_EXPORT, ...(v ?? {}) }
  } catch {
    return DEFAULT_EXPORT
  }
}

function storeOptions(o: ExportOptions) {
  try {
    localStorage.setItem(EXPORT_KEY, JSON.stringify(o))
  } catch {
    // A blocked store only loses the remembered choice.
  }
}

export function SessionExport({ session, search }: { session: SessionView; search: string }) {
  const [opened, setOpened] = useState(false)
  const [o, setO] = useState<ExportOptions>(storedOptions)
  const [busy, setBusy] = useState<number | null>(null)
  const [error, setError] = useState<string | null>(null)
  const q = search.trim()
  const set = (patch: Partial<ExportOptions>) =>
    setO((prev) => {
      const next = { ...prev, ...patch }
      storeOptions(next)
      return next
    })

  const run = async () => {
    setError(null)
    setBusy(0)
    try {
      const filter = o.matching && q ? q : undefined
      const rows = await collectSession(session.session_id, filter, setBusy)
      const now = new Date()
      download(exportFileName(session, o, now), formatSession(session, rows, o, filter, now), o.format)
      setOpened(false)
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(null)
    }
  }

  return (
    <Popover opened={opened} onChange={setOpened} position="bottom-end" withArrow shadow="md" width={340}>
      <Popover.Target>
        <Button size="compact-xs" variant="default" onClick={() => setOpened((v) => !v)}>
          export
        </Button>
      </Popover.Target>
      <Popover.Dropdown>
        <Stack gap="sm">
          <Text size="sm" fw={600}>
            Export this session as a file
          </Text>
          <div>
            <Text size="xs" c="dimmed" mb={4}>
              Include
            </Text>
            <SegmentedControl
              size="xs"
              fullWidth
              value={o.include}
              onChange={(v) => set({ include: v as ExportOptions['include'] })}
              data={[
                { value: 'conversation', label: 'Conversation' },
                { value: 'everything', label: 'Everything' },
              ]}
            />
            <Text size="xs" c="dimmed" mt={4}>
              {o.include === 'conversation'
                ? 'user, assistant, tool and system messages'
                : 'the messages, plus the OS’s action events'}
            </Text>
          </div>
          <div>
            <Text size="xs" c="dimmed" mb={4}>
              Tool output
            </Text>
            <SegmentedControl
              size="xs"
              fullWidth
              value={o.tools}
              onChange={(v) => set({ tools: v as ExportOptions['tools'] })}
              data={[
                { value: 'full', label: 'Full' },
                { value: 'trimmed', label: `First ${TRIMMED_TOOL_LINES} lines` },
                { value: 'none', label: 'Leave out' },
              ]}
            />
          </div>
          <div>
            <Text size="xs" c="dimmed" mb={4}>
              Format
            </Text>
            <SegmentedControl
              size="xs"
              fullWidth
              value={o.format}
              onChange={(v) => set({ format: v as ExportOptions['format'] })}
              data={[
                { value: 'txt', label: 'Plain text (.txt)' },
                { value: 'md', label: 'Markdown (.md)' },
              ]}
            />
          </div>
          <Switch
            size="xs"
            label="Timestamps (your clock)"
            checked={o.timestamps}
            onChange={(e) => set({ timestamps: e.currentTarget.checked })}
          />
          {q && (
            <Checkbox
              size="xs"
              label={`Only rows matching “${q}”`}
              checked={o.matching}
              onChange={(e) => set({ matching: e.currentTarget.checked })}
            />
          )}
          {error && (
            <Text size="xs" c="red.4">
              could not export: {error}
            </Text>
          )}
          <Group justify="space-between">
            <Text size="xs" c="dimmed">
              {busy == null ? 'the whole session, back to its first row' : `reading… ${busy.toLocaleString()} rows`}
            </Text>
            <Button size="xs" onClick={() => void run()} loading={busy != null}>
              Download
            </Button>
          </Group>
        </Stack>
      </Popover.Dropdown>
    </Popover>
  )
}
