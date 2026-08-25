import { useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Alert,
  Anchor,
  Badge,
  Button,
  Card,
  Group,
  Select,
  Stack,
  Text,
  Textarea,
  Title,
} from '@mantine/core'
import { fetchContent, fetchLabels, uploadFile, writeContentNote } from './api'
import { MarkdownView } from './Markdown'

// Content on anything (#296, spec §3).
//
// Notes and attachments are one idea in two shapes: content that belongs
// to something and is never a node. The something is an entity, a TYPE
// or a LABEL — "because a type is exactly the thing people argue about
// and needs a thread of its own".
//
// The same panel serves all three, because they are the same act.

export function Content({
  kind,
  uid,
  title,
}: {
  kind: 'entity' | 'type' | 'label'
  uid: string
  title?: string
}) {
  const qc = useQueryClient()
  const content = useQuery({
    queryKey: ['content', kind, uid],
    queryFn: () => fetchContent(kind, uid),
  })
  const labels = useQuery({ queryKey: ['labels'], queryFn: () => fetchLabels(false) })
  const slotLabels = (labels.data ?? []).filter((l) => l.label_kind === 'slot')

  const refresh = () => void qc.invalidateQueries({ queryKey: ['content', kind, uid] })

  return (
    <Card withBorder padding="md">
      <Title order={5} mb="xs">
        {title ?? `What belongs to this ${kind}`}
      </Title>

      <Stack gap="sm" mb="md">
        {(content.data?.notes ?? []).map((n) => (
          <div key={n.note_uid}>
            <Group gap="xs" mb={2}>
              <Badge size="xs" variant="light">
                {n.label}
              </Badge>
              <Text size="xs" c="dimmed">
                {n.author_kind ?? '?'}
                {n.via_uid ? ` as ${n.via_uid}` : ''}
              </Text>
              {n.parent_uid && (
                <Text size="xs" c="dimmed">
                  ↳ answering {n.parent_uid.slice(0, 8)}
                </Text>
              )}
            </Group>
            <MarkdownView markdown={n.content} />
          </div>
        ))}
        {(content.data?.notes ?? []).length === 0 && (
          <Text size="sm" c="dimmed">
            Nothing written yet.
          </Text>
        )}
      </Stack>

      <Files
        files={content.data?.files ?? []}
        kind={kind}
        uid={uid}
        labels={slotLabels.map((l) => l.key)}
        onDone={refresh}
      />

      <WriteNote
        kind={kind}
        uid={uid}
        labels={slotLabels.map((l) => ({ value: l.key, label: `${l.key} — ${l.display}` }))}
        onDone={refresh}
      />
    </Card>
  )
}

/// A file IS its label: a spec sheet as a PDF is still a spec (§5.4).
function Files({
  files,
  kind,
  uid,
  labels,
  onDone,
}: {
  files: Array<{
    uid: string
    label: string
    filename: string
    mime: string
    size: bigint
    author_kind: string | null
  }>
  kind: string
  uid: string
  labels: string[]
  onDone: () => void
}) {
  const picker = useRef<HTMLInputElement>(null)
  const [label, setLabel] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const upload = useMutation({
    mutationFn: (file: File) => uploadFile(kind, uid, label ?? '', file),
    onSuccess: () => {
      setError(null)
      onDone()
    },
    onError: (e) => setError(String(e).replace(/^Error:\s*/, '')),
  })

  return (
    <>
      {files.length > 0 && (
        <Stack gap={4} mb="sm">
          {files.map((f) => (
            <Group key={f.uid} gap="xs" wrap="nowrap">
              {f.label ? (
                <Badge size="xs" color="pelican" variant="light">
                  {f.label}
                </Badge>
              ) : (
                <Badge size="xs" color="gray" variant="outline">
                  unlabelled
                </Badge>
              )}
              <Anchor size="sm" href={`/api/files/${f.uid}/download`} target="_blank">
                {f.filename}
              </Anchor>
              <Text size="xs" c="dimmed" ff="monospace">
                {f.mime} · {String(f.size)} bytes
              </Text>
            </Group>
          ))}
        </Stack>
      )}

      <Group gap="xs" mb="sm" align="flex-end">
        <Select
          size="xs"
          label="Label — optional"
          description="a label makes it actionable; without one it is attached for reference"
          placeholder="none"
          data={labels}
          value={label}
          onChange={setLabel}
          clearable
          searchable
          w={260}
        />
        <input
          ref={picker}
          type="file"
          style={{ display: 'none' }}
          onChange={(e) => {
            const file = e.currentTarget.files?.[0]
            if (file) upload.mutate(file)
            e.currentTarget.value = ''
          }}
        />
        {/* NOT gated on the label. A file has a name and bytes; the
            label is the optional third thing, and requiring it meant
            you could not attach anything without first deciding what it
            MEANS — §5.4's "a PDF labelled mandate IS the mandate" is
            what a label BUYS you, not the price of attaching. */}
        <Button
          size="xs"
          variant="light"
          loading={upload.isPending}
          onClick={() => picker.current?.click()}
        >
          Choose file
        </Button>
      </Group>
      {error && (
        <Alert color="red" mb="sm" p="xs">
          <Text size="xs">{error}</Text>
        </Alert>
      )}
    </>
  )
}

function WriteNote({
  kind,
  uid,
  labels,
  onDone,
}: {
  kind: string
  uid: string
  labels: Array<{ value: string; label: string }>
  onDone: () => void
}) {
  const [label, setLabel] = useState<string | null>(null)
  const [body, setBody] = useState('')
  const [error, setError] = useState<string | null>(null)

  const write = useMutation({
    mutationFn: () => writeContentNote(kind, uid, { label: label as string, body }),
    onSuccess: () => {
      setBody('')
      setError(null)
      onDone()
    },
    onError: (e) => setError(String(e).replace(/^Error:\s*/, '')),
  })

  return (
    <>
      <Select
        size="xs"
        label="Write as"
        placeholder="pick a label"
        data={labels}
        value={label}
        onChange={setLabel}
        searchable
        mb="xs"
      />
      <Textarea
        placeholder={`what this ${kind} is, or the argument about what it should be`}
        value={body}
        onChange={(e) => setBody(e.currentTarget.value)}
        autosize
        minRows={2}
        mb="xs"
      />
      {error && (
        <Alert color="red" mb="xs" p="xs">
          <Text size="xs">{error}</Text>
        </Alert>
      )}
      <Button
        size="xs"
        disabled={!label || !body.trim()}
        loading={write.isPending}
        onClick={() => write.mutate()}
      >
        Write
      </Button>
    </>
  )
}
