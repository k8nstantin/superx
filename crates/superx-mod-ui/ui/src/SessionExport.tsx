import { useEffect, useState } from 'react'
import { Button, Checkbox, Group, Popover, SegmentedControl, Stack, Switch, Text } from '@mantine/core'
import type { SessionView } from './generated/SessionView'
import {
  DEFAULT_EXPORT,
  TRIMMED_TOOL_LINES,
  canChooseWhere,
  chooseWhere,
  download,
  exportFileName,
  exportUrl,
  formatBytes,
  runExport,
  type ExportOptions,
} from './exportSession'

// The session view's export (#442, #460): the whole session as a file, in
// the shape the reader picks, every byte of it written by the server and
// streamed into the file. The last choice is remembered per viewer.

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

export function SessionExport({
  session,
  search,
  onBusy,
}: {
  session: SessionView
  search: string
  /// Told when an export starts and ends, so the view can stand aside.
  onBusy?: (busy: boolean) => void
}) {
  const [opened, setOpened] = useState(false)
  const [o, setO] = useState<ExportOptions>(storedOptions)
  const [busy, setBusy] = useState<number | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [saved, setSaved] = useState<string | null>(null)
  const q = search.trim()
  const set = (patch: Partial<ExportOptions>) =>
    setO((prev) => {
      const next = { ...prev, ...patch }
      storeOptions(next)
      return next
    })

  // A run's end is told once its result is on screen: the view it frees
  // redraws its feed, and on a long session that held "saved as" back for
  // seconds. An effect runs after the browser has painted.
  const [ended, setEnded] = useState(0)
  useEffect(() => {
    if (ended) onBusy?.(false)
  }, [ended, onBusy])

  const choose = canChooseWhere()
  const run = async () => {
    setError(null)
    setSaved(null)
    try {
      const now = new Date()
      const name = exportFileName(session, o, now)
      // The view stands aside from the click on, so a page it was reading
      // lands while the dialog is open rather than after it.
      onBusy?.(true)
      // The dialog first, straight off the click: the browser only opens
      // it inside the reader's gesture, and a long session could outlast
      // that.
      const target = choose ? await chooseWhere(name, o.format) : null
      if (choose && !target) return // cancelled
      setBusy(0)
      const url = exportUrl(session.session_id, o, o.matching && q ? q : undefined)
      const done = await runExport(url, target, o.format, setBusy)
      if (target) {
        setSaved(`${target.name} · ${done.holds.split(' · ')[0]} · ${formatBytes(done.bytes)}`)
      } else if (done.file) {
        // No save dialog: the file was gathered and checked whole; it is
        // handed over as a download.
        download(name, done.file)
        setOpened(false)
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
      setEnded((n) => n + 1)
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
            <Text size="xs" c="dimmed" mt={4}>
              each tool call’s input and each tool’s output alike
            </Text>
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
              could not export ({error}); nothing was saved
            </Text>
          )}
          {saved && busy == null && (
            <Text size="xs" c="teal.4">
              saved as {saved}
            </Text>
          )}
          <Group justify="space-between">
            <Text size="xs" c="dimmed">
              {busy != null
                ? busy === 0
                  ? 'asking for the session…'
                  : `writing… ${formatBytes(busy)}`
                : choose
                  ? 'the whole session — you pick the folder and name'
                  : 'the whole session, to your downloads folder (this browser has no save dialog)'}
            </Text>
            <Button size="xs" onClick={() => void run()} loading={busy != null}>
              {choose ? 'Save as…' : 'Download'}
            </Button>
          </Group>
        </Stack>
      </Popover.Dropdown>
    </Popover>
  )
}
