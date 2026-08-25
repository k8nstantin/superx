import { useMemo, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Anchor,
  Badge,
  Button,
  Card,
  Collapse,
  Divider,
  Grid,
  FileInput,
  Group,
  Modal,
  ScrollArea,
  Select,
  Table,
  Text,
  TextInput,
  Textarea,
  Title,
  Tooltip,
} from '@mantine/core'
import {
  attachFile,
  commentEntity,
  createEntity,
  downloadUrl,
  describeEntity,
  fetchDetail,
  setArchived,
  fetchEntities,
  fetchHistory,
  fetchRelTypes,
  fetchTypes,
  linkEntity,
  typeColor,
  unlinkEntity,
  updateEntity,
} from '../api'
import { MarkdownEditor, MarkdownView, type MarkdownEditorHandle } from '../Markdown'
import { LongText, previewLine } from '../LongText'
import type { EntityDetail } from '../generated/EntityDetail'
import type { AnnotationView } from '../generated/AnnotationView'
import { useBreadcrumb } from '../Breadcrumbs'
import { Fields } from '../Fields'
import { Content } from '../Content'

// The Entities page (issue #231, approved design): list with a type
// DROPDOWN + search + New entity; click a row → the entity's detail
// with description/instructions/comments (BlockNote), edges, history.

export default function EntitiesPage() {
  // `?entity=<frag>` opens straight onto that entity — how the graph
  // window links back to what it is rooted at (#250).
  const [selected, setSelected] = useState<string | null>(
    () => new URLSearchParams(window.location.search).get('entity'),
  )
  return selected ? (
    <DetailView frag={selected} onBack={() => setSelected(null)} onOpen={setSelected} />
  ) : (
    <ListView onOpen={setSelected} />
  )
}

function TypeBadge({ type, outline }: { type: string; outline?: boolean }) {
  return (
    <Badge
      variant={outline ? 'outline' : 'filled'}
      autoContrast
      color={typeColor(type)}
      style={{ flexShrink: 0 }}
    >
      {type}
    </Badge>
  )
}

function ListView({ onOpen }: { onOpen: (frag: string) => void }) {
  useBreadcrumb([{ label: 'Entities' }])
  const [typeFilter, setTypeFilter] = useState<string | null>(null)
  const [search, setSearch] = useState('')
  const [createOpen, setCreateOpen] = useState(false)
  const types = useQuery({ queryKey: ['types'], queryFn: fetchTypes })
  const list = useQuery({
    queryKey: ['entities', typeFilter],
    queryFn: () => fetchEntities(typeFilter ?? undefined),
    refetchInterval: 15000,
  })
  const rows = (list.data ?? []).filter(
    (e) =>
      search.trim() === '' ||
      e.name.toLowerCase().includes(search.toLowerCase()) ||
      e.id.includes(search),
  )
  return (
    <Card withBorder>
      <Group justify="space-between" mb="sm">
        <Title order={5}>The product graph — what your agents build and execute</Title>
        <Group gap="xs">
          <Select
            placeholder="Type: all"
            clearable
            data={(types.data ?? []).map((t) => ({
              value: t.name,
              label: t.system ? `${t.name} — comments & descriptions` : t.name,
            }))}
            value={typeFilter}
            onChange={setTypeFilter}
            w={170}
          />
          <TextInput
            placeholder="search name or uuid fragment…"
            value={search}
            onChange={(e) => setSearch(e.currentTarget.value)}
            w={240}
          />
          <Button onClick={() => setCreateOpen(true)}>+ New entity</Button>
        </Group>
      </Group>
      {list.isError && (
        <Text c="red.4" size="sm">
          {String(list.error)} — is the module provisioned? (superx modules provision entities)
        </Text>
      )}
      <ScrollArea h={620}>
        <Table striped highlightOnHover>
          <Table.Thead>
            <Table.Tr>
              <Table.Th w={130}>Type</Table.Th>
              <Table.Th>Name</Table.Th>
              <Table.Th>Entity id</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {rows.map((e) => (
              <Table.Tr key={e.id} onClick={() => onOpen(e.id)} style={{ cursor: 'pointer' }}>
                <Table.Td>
                  <TypeBadge type={e.entity_type} />
                </Table.Td>
                <Table.Td>
                  <Text fw={600}>{e.name}</Text>
                </Table.Td>
                <Table.Td>
                  <Text size="xs" ff="monospace" c="dimmed">
                    {e.id}
                  </Text>
                </Table.Td>
              </Table.Tr>
            ))}
          </Table.Tbody>
        </Table>
        {!list.isLoading && rows.length === 0 && (
          <Text c="dimmed" size="sm" mt="md">
            nothing here yet — create the first entity
          </Text>
        )}
      </ScrollArea>
      <CreateModal opened={createOpen} onClose={() => setCreateOpen(false)} onCreated={onOpen} />
    </Card>
  )
}

function CreateModal({
  opened,
  onClose,
  onCreated,
}: {
  opened: boolean
  onClose: () => void
  onCreated: (id: string) => void
}) {
  const qc = useQueryClient()
  const types = useQuery({ queryKey: ['types'], queryFn: fetchTypes })
  const [type, setType] = useState<string | null>(null)
  const [name, setName] = useState('')
  const [attrs, setAttrs] = useState('')
  const [error, setError] = useState<string | null>(null)
  const desc = useRef<MarkdownEditorHandle>(null)
  const create = useMutation({
    mutationFn: async () => {
      const description = (await desc.current?.getMarkdown()) ?? ''
      return createEntity({
        entity_type: type ?? '',
        name,
        description: description.trim() ? description : null,
        content: null,
        attributes_json: attrs.trim() ? attrs : null,
      })
    },
    onSuccess: (r) => {
      void qc.invalidateQueries({ queryKey: ['entities'] })
      onClose()
      onCreated(r.id)
    },
    onError: (e) => setError(String(e)),
  })
  return (
    <Modal opened={opened} onClose={onClose} title="New entity" size="xl">
      <Group grow mb="sm">
        <Select
          label="Type — from the type registry"
          placeholder="pick a type"
          data={(types.data ?? [])
            .filter((t) => !t.system)
            .map((t) => ({
              value: t.name,
              label: t.description ? `${t.name} — ${t.description}` : t.name,
            }))}
          value={type}
          onChange={setType}
          searchable
        />
        <TextInput label="Name" value={name} onChange={(e) => setName(e.currentTarget.value)} />
      </Group>
      <Text size="sm" fw={600} mb={4}>
        Description <Text span size="xs" c="dimmed">— becomes a text node linked describes→</Text>
      </Text>
      {opened && <MarkdownEditor ref={desc} minHeight={300} maxHeight="45vh" />}
      <Textarea
        label="Attributes — optional JSON, schema-free per type"
        mt="sm"
        rows={3}
        value={attrs}
        onChange={(e) => setAttrs(e.currentTarget.value)}
        styles={{ input: { fontFamily: "'JetBrains Mono', ui-monospace, monospace" } }}
      />
      {error && (
        <Text c="red.4" size="sm" mt="xs">
          {error}
        </Text>
      )}
      <Group justify="flex-end" mt="md">
        <Button variant="default" onClick={onClose}>
          Cancel
        </Button>
        <Button
          onClick={() => create.mutate()}
          loading={create.isPending}
          disabled={!type || !name.trim()}
        >
          Create entity
        </Button>
      </Group>
    </Modal>
  )
}

/**
 * A text node's content, with its own version chain on show.
 *
 * A description is a text ENTITY (D22), so it has an SCD-2 chain of its
 * own — the runner spec's description is on its third version. The page
 * used to render the current body with no hint that earlier ones exist,
 * while the History button showed the *anchor's* chain, which is a
 * different thing (#261). `fetchHistory` works on any fragment and
 * VersionView carries `content`, so browsing needs no new route.
 */
function TextNodeCard({
  title,
  annotation,
  onEdit,
  sectionRef,
}: {
  title: string
  annotation: AnnotationView
  onEdit?: () => void
  sectionRef?: React.Ref<HTMLDivElement>
}) {
  const [at, setAt] = useState<string | null>(null)
  const history = useQuery({
    queryKey: ['history', annotation.note_uid],
    queryFn: () => fetchHistory(annotation.note_uid),
  })
  // Ascending — oldest first (nodes.rs:220), so the last row is current.
  const versions = history.data ?? []
  const current = versions[versions.length - 1]
  const chosen = at ? versions.find((v) => v.valid_from === at) : undefined
  const historical = Boolean(chosen) && chosen?.valid_from !== current?.valid_from
  const shown = chosen ? versions.indexOf(chosen) + 1 : versions.length
  // The annotation always carries the live body; history supplies the
  // older ones. Never fall back silently to the wrong version.
  const body = (historical ? chosen?.content : annotation.content) ?? annotation.content

  return (
    <Card withBorder mb="md" ref={sectionRef}>
      <Group justify="space-between" mb="xs" wrap="nowrap">
        <Group gap="xs" wrap="nowrap">
          <Title order={6} tt="uppercase" c="dimmed">
            {title}
          </Title>
          {versions.length > 0 && (
            <Badge size="xs" variant="light" color={historical ? 'yellow' : 'pelican'}>
              v{shown} of {versions.length}
            </Badge>
          )}
        </Group>
        <Group gap={6} wrap="nowrap">
          {versions.length > 1 && (
            <Select
              size="xs"
              w={215}
              value={at ?? current?.valid_from ?? null}
              onChange={setAt}
              comboboxProps={{ withinPortal: true }}
              data={[...versions].reverse().map((v, i) => ({
                value: v.valid_from,
                label:
                  'v' +
                  String(versions.length - i) +
                  ' · ' +
                  v.valid_from.slice(0, 16).replace('T', ' ') +
                  (i === 0 ? ' · current' : ''),
              }))}
            />
          )}
          {onEdit && (
            <Button size="compact-xs" variant="subtle" disabled={historical} onClick={onEdit}>
              edit
            </Button>
          )}
        </Group>
      </Group>
      {historical && (
        <Text size="xs" c="yellow.4" mb={6}>
          viewing v{shown} of {versions.length} · historical, not current — editing is disabled here
        </Text>
      )}
      <LongText markdown={body} />
    </Card>
  )
}

function DetailView({
  frag,
  onBack,
  onOpen,
}: {
  frag: string
  onBack: () => void
  onOpen: (frag: string) => void
}) {
  const qc = useQueryClient()
  // §14: ONE instant reaching every chain — state, notes, attachments
  // and edges resolved at the same moment. A picker per field answers
  // "how did this text change"; this answers "what did the agent see
  // when it did that", which is the question after a bad run.
  const [asOf, setAsOf] = useState<string | null>(null)
  const detail = useQuery({
    queryKey: ['entity', frag, asOf],
    queryFn: () => fetchDetail(frag, asOf ?? undefined),
  })
  const [historyOpen, setHistoryOpen] = useState(false)
  const [editOpen, setEditOpen] = useState(false)
  const [linkOpen, setLinkOpen] = useState(false)
  const [describeOpen, setDescribeOpen] = useState(false)
  const [attachOpen, setAttachOpen] = useState(false)
  const history = useQuery({
    queryKey: ['history', frag],
    queryFn: () => fetchHistory(frag),
    enabled: historyOpen,
  })
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['entity', frag] })
    void qc.invalidateQueries({ queryKey: ['history', frag] })
    void qc.invalidateQueries({ queryKey: ['entities'] })
  }
  const d = detail.data
  // The ancestor path from the server (#253) — root first, each step
  // clickable, this entity last.
  useBreadcrumb([
    { label: 'Entities', onClick: onBack },
    ...(d?.ancestors ?? []).map((a) => ({ label: a.name || a.id.slice(0, 8), onClick: () => onOpen(a.id) })),
    ...(d ? [{ label: d.name }] : []),
  ])
  // Prose is a note carrying a dictionary label (#278) — `description`,
  // `instructions`, `comments` — not an edge's rel_type.
  const description = d?.annotations.find((a) => a.label === 'description')
  const instructions = d?.annotations.find((a) => a.label === 'instructions')
  const comments = (d?.annotations ?? []).filter((a) => a.label === 'comments')

  // A description can be a whole build spec, so the sections below it
  // are a scroll away even when collapsed. Jump to them (#261).
  const descRef = useRef<HTMLDivElement>(null)
  const instrRef = useRef<HTMLDivElement>(null)
  const commentsRef = useRef<HTMLDivElement>(null)
  const edgesRef = useRef<HTMLDivElement>(null)
  const fieldsRef = useRef<HTMLDivElement>(null)
  const attachRef = useRef<HTMLDivElement>(null)
  const jump = (r: React.RefObject<HTMLDivElement | null>) =>
    r.current?.scrollIntoView({ behavior: 'smooth', block: 'start' })

  if (detail.isError)
    return (
      <Card withBorder>
        <Text c="red.4">{String(detail.error)}</Text>
        <Button mt="sm" variant="default" onClick={onBack}>
          ← entities
        </Button>
      </Card>
    )
  if (!d) return <Card withBorder>loading…</Card>

  return (
    <>
      <Group justify="space-between" mb="sm">
        <Group gap="sm">
          <Button size="compact-sm" variant="default" onClick={onBack}>
            ← entities
          </Button>
          <TypeBadge type={d.entity_type} />
          <Title order={4}>{d.name}</Title>
          <Text size="xs" ff="monospace" c="dimmed">
            {d.id}
          </Text>
        </Group>
        <Group gap="xs">
          {/* One control for the whole entity, not one per field: each
              chain moves independently, so field-by-field pickers can
              never show a moment that actually happened. */}
          <TextInput
            size="xs"
            w={230}
            placeholder="as of… 2026-08-24T17:00:00Z"
            value={asOf ?? ''}
            onChange={(e) => setAsOf(e.currentTarget.value || null)}
          />
          {asOf && (
            <Button size="compact-sm" variant="subtle" onClick={() => setAsOf(null)}>
              now
            </Button>
          )}
          <Button size="compact-sm" onClick={() => setEditOpen(true)} disabled={!!asOf}>
            Edit
          </Button>
          {d?.archived ? (
            <Button
              size="compact-sm"
              variant="default"
              onClick={() => {
                void setArchived(frag, false).then(refresh)
              }}
            >
              Restore
            </Button>
          ) : (
            <Tooltip label="Hide it from the lists. Nothing is erased — its history, notes and edges all stay.">
              <Button
                size="compact-sm"
                variant="default"
                onClick={() => {
                  void setArchived(frag, true).then(refresh)
                }}
              >
                Archive
              </Button>
            </Tooltip>
          )}
          <Button size="compact-sm" variant="default" onClick={() => setLinkOpen(true)}>
            + Link
          </Button>
          <Button size="compact-sm" variant="default" onClick={() => setAttachOpen(true)}>
            + Attach
          </Button>
          {/* Opens in its own window: a force-directed graph needs the
              room, and a panel between the cards gives it none (#250). */}
          <Button
            size="compact-sm"
            variant="default"
            component="a"
            href={`/graph/${d.id}`}
            target="_blank"
            rel="noopener"
          >
            Graph ↗
          </Button>
          <Button size="compact-sm" variant="default" onClick={() => setHistoryOpen((o) => !o)}>
            History {historyOpen ? '▴' : '▾'}
          </Button>
        </Group>
      </Group>

      <Group gap={6} mb="sm" wrap="wrap">
        <Text size="xs" c="dimmed" tt="uppercase">
          jump to
        </Text>
        {description && (
          <Button size="compact-xs" variant="light" onClick={() => jump(descRef)}>
            description
          </Button>
        )}
        {instructions && (
          <Button size="compact-xs" variant="light" onClick={() => jump(instrRef)}>
            instructions
          </Button>
        )}
        <Button size="compact-xs" variant="light" onClick={() => jump(fieldsRef)}>
          fields
        </Button>
        <Button size="compact-xs" variant="light" onClick={() => jump(commentsRef)}>
          comments {comments.length > 0 ? `(${comments.length})` : ''}
        </Button>
        <Button size="compact-xs" variant="light" onClick={() => jump(edgesRef)}>
          edges {d.edges.length > 0 ? `(${d.edges.length})` : ''}
        </Button>
        <Button size="compact-xs" variant="light" onClick={() => jump(attachRef)}>
          attachments {d.attachments.length > 0 ? `(${d.attachments.length})` : ''}
        </Button>
      </Group>

      <Collapse expanded={historyOpen}>
        <Card withBorder mb="sm">
          <Title order={6} mb="xs">
            History — append-only, latest wins
          </Title>
          {(history.data ?? []).map((v, i, all) => (
            <Group key={v.valid_from} gap="sm" mb={4}>
              {/* state_history orders ASC (nodes.rs:220), so index 0 is
                  the OLDEST — it is v1. Counting down from the length
                  labelled the first version as the last one (#261). */}
              <Text size="xs" ff="monospace" c="dimmed" w={44}>
                v{i + 1}
                {i === all.length - 1 ? '*' : ''}
              </Text>
              <Text size="xs" c="dimmed" w={190} ff="monospace">
                {v.valid_from}
              </Text>
              <Text size="sm">{v.name}</Text>
            </Group>
          ))}
          <Text size="xs" c="dimmed" mt={6}>
            * current
          </Text>
        </Card>
      </Collapse>

      <Grid gap="md">
        <Grid.Col span={{ base: 12, lg: 6 }}>
          {description ? (
            <TextNodeCard
              title="Description · text node"
              annotation={description}
              onEdit={() => setDescribeOpen(true)}
              sectionRef={descRef}
            />
          ) : (
            <Card withBorder mb="md" ref={descRef}>
              <Group justify="space-between" mb="xs">
                <Title order={6} tt="uppercase" c="dimmed">
                  Description · text node
                </Title>
                <Button size="compact-xs" variant="subtle" onClick={() => setDescribeOpen(true)}>
                  add
                </Button>
              </Group>
              <Text size="sm" c="dimmed">
                no description yet
              </Text>
            </Card>
          )}
          {instructions && (
            <TextNodeCard
              title="Instructions · instructs → text node"
              annotation={instructions}
              sectionRef={instrRef}
            />
          )}
          {/* The discussion starts here, and it has to LOOK like it
              does: a 35k-char description above an unmarked comment
              reads as one continuous document (#261). */}
          <Divider my="md" labelPosition="left" label="fields" color="dark.5" />
          {/* Beside the description and the comments, not in a column
              on the far side of the page. These are the three things an
              entity says about itself, and §6 designs them together:
              "seed, then design — add fields and label them". */}
          <Card withBorder ref={fieldsRef}>
            <Title order={6} tt="uppercase" c="dimmed" mb="xs">
              Fields
            </Title>
            <Fields entityId={d.id} />
          </Card>
          <Divider
            my="md"
            labelPosition="left"
            label={`comments · ${comments.length}`}
            color="dark.5"
          />
          <Card withBorder ref={commentsRef}>
            <Title order={6} tt="uppercase" c="dimmed" mb="xs">
              Comments · {comments.length} text node{comments.length === 1 ? '' : 's'}
            </Title>
            {comments.length === 0 && (
              <Text size="sm" c="dimmed" mb="xs">
                no comments yet — the box below adds one to this entity
              </Text>
            )}
            {comments.map((c, i) => (
              <div
                key={c.note_uid}
                style={{
                  border: '1px solid #3B2449',
                  borderRadius: 6,
                  padding: '8px 10px',
                  marginBottom: 10,
                }}
              >
                <Group gap="xs" mb={4} wrap="nowrap">
                  <Badge size="xs" variant="light" color="pelican">
                    #{i + 1}
                  </Badge>
                  <Text
                    size="xs"
                    fw={600}
                    style={{
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                      whiteSpace: 'nowrap',
                    }}
                  >
                    {previewLine(c.content)}
                  </Text>
                  <Text size="xs" c="dimmed" ff="monospace" style={{ flexShrink: 0 }}>
                    {c.note_uid.slice(0, 8)}
                  </Text>
                </Group>
                <LongText markdown={c.content} collapsedHeight={110} compact />
              </div>
            ))}
            <CommentComposer frag={frag} onPosted={refresh} />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 6 }}>
          <Card withBorder mb="md" ref={edgesRef}>
            <Title order={6} tt="uppercase" c="dimmed" mb="xs">
              Edges · {d.edges.length}
            </Title>
            {d.edges.length === 0 && (
              <Text size="sm" c="dimmed">
                no links yet — + Link connects this entity to others (depends_on orders execution)
              </Text>
            )}
            {d.edges.map((e) => (
              <Group key={`${e.edge_uid}-${e.outbound}`} gap="xs" mb={6}>
                <TypeBadge type={e.rel_type} outline />
                <Text size="sm" c="dimmed">
                  {e.outbound ? '→' : '←'}
                </Text>
                <TypeBadge type={e.other_type} />
                <Text
                  size="sm"
                  c="pelican.3"
                  style={{ cursor: 'pointer' }}
                  onClick={() => onOpen(e.other_id)}
                >
                  {e.other_name || e.other_id.slice(0, 8)}
                </Text>
                <Button
                  size="compact-xs"
                  variant="subtle"
                  color="red"
                  onClick={() =>
                    void unlinkEntity(frag, {
                      to: e.other_id,
                      rel: e.rel_type,
                    }).then(refresh)
                  }
                >
                  unlink
                </Button>
              </Group>
            ))}
          </Card>
          {d.content && (
            <Card withBorder mb="md">
              <Title order={6} tt="uppercase" c="dimmed" mb="xs">
                Content
              </Title>
              <MarkdownView markdown={d.content} />
            </Card>
          )}
          <Card withBorder mb="md" ref={attachRef}>
            <Group justify="space-between" mb="xs">
              <Title order={6} tt="uppercase" c="dimmed">
                Attachments · {d.attachments.length}
              </Title>
              <Button size="compact-xs" variant="subtle" onClick={() => setAttachOpen(true)}>
                + attach
              </Button>
            </Group>
            {d.attachments.length === 0 ? (
              <Text size="sm" c="dimmed">
                no files yet — attachments become document entities, linked attached→
              </Text>
            ) : (
              d.attachments.map((a) => (
                <Group key={a.id} justify="space-between" wrap="nowrap" mb={4}>
                  <Anchor href={downloadUrl(a.id)} size="sm" download>
                    {a.name}
                  </Anchor>
                  <Text size="xs" c="dimmed" ff="monospace">
                    {fmtBytes(Number(a.size))} · {a.mime}
                  </Text>
                </Group>
              ))
            )}
          </Card>
          <Content kind="entity" uid={d.id} title="Files" />
          <Card withBorder>
            <Title order={6} tt="uppercase" c="dimmed" mb="xs">
              Attributes (raw)
            </Title>
            <Text size="sm" ff="monospace" style={{ whiteSpace: 'pre-wrap' }}>
              {d.attributes_json ?? '—'}
            </Text>
          </Card>
        </Grid.Col>
      </Grid>

      <EditModal
        opened={editOpen}
        onClose={() => setEditOpen(false)}
        detail={d}
        frag={frag}
        onSaved={refresh}
      />
      <LinkModal
        opened={linkOpen}
        onClose={() => setLinkOpen(false)}
        frag={frag}
        selfId={d.id}
        onLinked={refresh}
      />
      <DescribeModal
        opened={describeOpen}
        onClose={() => setDescribeOpen(false)}
        frag={frag}
        initial={description?.content ?? ''}
        onSaved={refresh}
      />
      <AttachModal
        opened={attachOpen}
        onClose={() => setAttachOpen(false)}
        frag={frag}
        onAttached={refresh}
      />
    </>
  )
}

function fmtBytes(n: number): string {
  if (n >= 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`
  if (n >= 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${n} B`
}

// EU4 — one file at a time: the bytes go up as the request body, and
// the module stores them under its own dir as a document entity.
function AttachModal({
  opened,
  onClose,
  frag,
  onAttached,
}: {
  opened: boolean
  onClose: () => void
  frag: string
  onAttached: () => void
}) {
  const [file, setFile] = useState<File | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  return (
    <Modal opened={opened} onClose={onClose} title="Attach a file" size="md">
      <FileInput
        label="File"
        placeholder="choose a file…"
        value={file}
        onChange={setFile}
        clearable
      />
      <Text size="xs" c="dimmed" mt="xs">
        the file is copied into the entities module&apos;s own directory and recorded as a
        document entity, linked attached→ from this one
      </Text>
      {error && (
        <Text c="red.4" size="sm" mt="xs">
          {error}
        </Text>
      )}
      <Group justify="flex-end" mt="md">
        <Button variant="default" onClick={onClose}>
          Cancel
        </Button>
        <Button
          loading={busy}
          disabled={!file}
          onClick={() => {
            if (!file) return
            setBusy(true)
            setError(null)
            void attachFile(frag, file)
              .then(() => {
                setFile(null)
                onClose()
                onAttached()
              })
              .catch((e) => setError(String(e)))
              .finally(() => setBusy(false))
          }}
        >
          Attach
        </Button>
      </Group>
    </Modal>
  )
}

function CommentComposer({ frag, onPosted }: { frag: string; onPosted: () => void }) {
  const editor = useRef<MarkdownEditorHandle>(null)
  const [key, setKey] = useState(0)
  const [busy, setBusy] = useState(false)
  return (
    <div>
      <MarkdownEditor key={key} ref={editor} minHeight={160} maxHeight="40vh" />
      <Group justify="flex-end" mt="xs">
        <Button
          size="compact-sm"
          loading={busy}
          onClick={() => {
            setBusy(true)
            void (async () => {
              const md = (await editor.current?.getMarkdown()) ?? ''
              if (md.trim()) {
                await commentEntity(frag, md)
                setKey((k) => k + 1)
                onPosted()
              }
              setBusy(false)
            })()
          }}
        >
          Post
        </Button>
      </Group>
    </div>
  )
}

function DescribeModal({
  opened,
  onClose,
  frag,
  initial,
  onSaved,
}: {
  opened: boolean
  onClose: () => void
  frag: string
  initial: string
  onSaved: () => void
}) {
  const editor = useRef<MarkdownEditorHandle>(null)
  const [busy, setBusy] = useState(false)
  return (
    <Modal opened={opened} onClose={onClose} title="Description" size="xl">
      {opened && <MarkdownEditor ref={editor} initial={initial} minHeight={420} maxHeight="60vh" />}
      <Group justify="flex-end" mt="md">
        <Button variant="default" onClick={onClose}>
          Cancel
        </Button>
        <Button
          loading={busy}
          onClick={() => {
            setBusy(true)
            void (async () => {
              const md = (await editor.current?.getMarkdown()) ?? ''
              await describeEntity(frag, md)
              setBusy(false)
              onClose()
              onSaved()
            })()
          }}
        >
          Save — new version
        </Button>
      </Group>
    </Modal>
  )
}

function EditModal({
  opened,
  onClose,
  detail,
  frag,
  onSaved,
}: {
  opened: boolean
  onClose: () => void
  detail: EntityDetail
  frag: string
  onSaved: () => void
}) {
  const [name, setName] = useState(detail.name)
  const [attrs, setAttrs] = useState(detail.attributes_json ?? '')
  const content = useRef<MarkdownEditorHandle>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  return (
    <Modal opened={opened} onClose={onClose} title={`Edit ${detail.name} — a new version`} size="xl">
      <TextInput label="Name" value={name} onChange={(e) => setName(e.currentTarget.value)} mb="sm" />
      <Text size="sm" fw={600} mb={4}>
        Content
      </Text>
      {opened && (
        <MarkdownEditor ref={content} initial={detail.content ?? ''} minHeight={340} maxHeight="45vh" />
      )}
      <Textarea
        label="Attributes — REPLACES the whole object; leave as-is to keep"
        mt="sm"
        rows={3}
        value={attrs}
        onChange={(e) => setAttrs(e.currentTarget.value)}
        styles={{ input: { fontFamily: "'JetBrains Mono', ui-monospace, monospace" } }}
      />
      {error && (
        <Text c="red.4" size="sm" mt="xs">
          {error}
        </Text>
      )}
      <Group justify="flex-end" mt="md">
        <Button variant="default" onClick={onClose}>
          Cancel
        </Button>
        <Button
          loading={busy}
          onClick={() => {
            setBusy(true)
            setError(null)
            void (async () => {
              try {
                const md = (await content.current?.getMarkdown()) ?? ''
                // §6 compare-and-append: send the version this form
                // was opened on. If somebody else wrote in between the
                // save is refused and names their version, instead of
                // silently overwriting an edit nobody is told about.
                await updateEntity(frag, {
                  name: name.trim() ? name : null,
                  content: md.trim() ? md : null,
                  attributes_json: attrs.trim() ? attrs : null,
                  based_on: detail.version,
                })
                onClose()
                onSaved()
              } catch (e) {
                setError(String(e))
              } finally {
                setBusy(false)
              }
            })()
          }}
        >
          Save — new version
        </Button>
      </Group>
    </Modal>
  )
}

function LinkModal({
  opened,
  onClose,
  frag,
  selfId,
  onLinked,
}: {
  opened: boolean
  onClose: () => void
  frag: string
  selfId: string
  onLinked: () => void
}) {
  const entities = useQuery({ queryKey: ['entities', null], queryFn: () => fetchEntities() })
  const rels = useQuery({ queryKey: ['rel-types'], queryFn: fetchRelTypes })
  const [to, setTo] = useState<string | null>(null)
  const [rel, setRel] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  return (
    <Modal opened={opened} onClose={onClose} title="Link this entity" size="md">
      <Select
        label="Target entity"
        placeholder="pick an entity"
        searchable
        data={(entities.data ?? [])
          .filter((e) => e.id !== selfId)
          .map((e) => ({ value: e.id, label: `${e.name} (${e.entity_type})` }))}
        value={to}
        onChange={setTo}
        mb="sm"
      />
      <Select
        label="Relation"
        placeholder="pick a relation"
        data={rels.data ?? []}
        value={rel}
        onChange={setRel}
        description="depends_on = execution order: ALL dependencies complete before a task fires"
      />
      {error && (
        <Text c="red.4" size="sm" mt="xs">
          {error}
        </Text>
      )}
      <Group justify="flex-end" mt="md">
        <Button variant="default" onClick={onClose}>
          Cancel
        </Button>
        <Button
          disabled={!to || !rel}
          loading={busy}
          onClick={() => {
            setBusy(true)
            setError(null)
            void linkEntity(frag, { to: to ?? '', rel: rel ?? '' })
              .then(() => {
                onClose()
                onLinked()
              })
              .catch((e) => setError(String(e)))
              .finally(() => setBusy(false))
          }}
        >
          Link
        </Button>
      </Group>
    </Modal>
  )
}
