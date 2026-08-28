import { useEffect, useRef, useState } from 'react'
import { Button, Group, Text, Textarea } from '@mantine/core'
import { useCreateBlockNote } from '@blocknote/react'
import { BlockNoteView } from '@blocknote/mantine'
import '@blocknote/core/fonts/inter.css'
import '@blocknote/mantine/style.css'

// `text` IS ALWAYS THIS EDITOR (operator, 2026-08-28). Two text fields
// on one entity get the same control; what makes one a description and
// the other a comment is the LABEL it carries, never a different widget.
//
// `json` shares it in the sense that it is the same field on the page,
// but its content is a structure the database can look inside — so it
// gets a structured surface rather than prose, and is validated before
// it is sent. Storing JSON as text would make it a lump the database
// could never query.

export function Editor({
  value,
  json,
  onSave,
}: {
  value: unknown
  json: boolean
  onSave: (v: unknown) => void
}) {
  if (json) return <JsonEditor value={value} onSave={onSave} />
  return <ProseEditor value={typeof value === 'string' ? value : ''} onSave={onSave} />
}

function ProseEditor({ value, onSave }: { value: string; onSave: (v: string) => void }) {
  const editor = useCreateBlockNote()
  const [ready, setReady] = useState(false)

  // BlockNote holds blocks, the module holds a string. Parse in on
  // mount, serialise out on save — the conversion lives here so nothing
  // else has to know the editor exists.
  useEffect(() => {
    let cancelled = false
    void (async () => {
      const blocks = await editor.tryParseHTMLToBlocks(value || '<p></p>')
      if (!cancelled) {
        editor.replaceBlocks(editor.document, blocks)
        setReady(true)
      }
    })()
    return () => {
      cancelled = true
    }
    // Only on mount, and when the stored value changes underneath us.
  }, [editor, value])

  return (
    <>
      <BlockNoteView editor={editor} theme="dark" />
      <Group justify="flex-end" mt={4}>
        <Button
          size="compact-xs"
          variant="subtle"
          disabled={!ready}
          onClick={() => {
            void Promise.resolve(editor.blocksToHTMLLossy(editor.document)).then(onSave)
          }}
        >
          Save
        </Button>
      </Group>
    </>
  )
}

function JsonEditor({ value, onSave }: { value: unknown; onSave: (v: unknown) => void }) {
  const serialised = JSON.stringify(value ?? {}, null, 2)
  const [draft, setDraft] = useState(serialised)
  const [error, setError] = useState<string | null>(null)

  // RESET ONLY WHEN THE STORED VALUE ACTUALLY CHANGED. `value` is a
  // parsed object, so React Query hands back a new identity on every
  // refetch even when the bytes are identical — and refetches happen on
  // window focus and whenever any other field on this entity is saved.
  // Keying on the serialised form means an in-progress edit survives all
  // of them; the same trap the grid layout fell into next door.
  const loaded = useRef(serialised)
  useEffect(() => {
    if (loaded.current === serialised) return
    loaded.current = serialised
    setDraft(serialised)
  }, [serialised])
  return (
    <>
      <Textarea
        autosize
        minRows={4}
        value={draft}
        onChange={(e) => {
          setDraft(e.currentTarget.value)
          setError(null)
        }}
        styles={{ input: { fontFamily: 'var(--mantine-font-family-monospace)', fontSize: 12 } }}
      />
      {error && (
        <Text c="red" size="xs" mt={2}>
          {error}
        </Text>
      )}
      <Group justify="flex-end" mt={4}>
        <Button
          size="compact-xs"
          variant="subtle"
          onClick={() => {
            try {
              const parsed: unknown = JSON.parse(draft)
              // The module refuses a bare string stored as json, so
              // catching it here means a clear message instead of a
              // round trip to be told no.
              if (parsed === null || typeof parsed !== 'object') {
                setError('json holds an object or an array — a bare value is text')
                return
              }
              onSave(parsed)
            } catch (err) {
              setError(String(err))
            }
          }}
        >
          Save
        </Button>
      </Group>
    </>
  )
}
