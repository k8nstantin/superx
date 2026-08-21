import { useMemo, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Anchor,
  Badge,
  Button,
  Card,
  Collapse,
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
import type { EntityDetail } from '../generated/EntityDetail'

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
  const detail = useQuery({ queryKey: ['entity', frag], queryFn: () => fetchDetail(frag) })
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
  const description = d?.annotations.find((a) => a.rel_type === 'describes')
  const instructions = d?.annotations.find((a) => a.rel_type === 'instructs')
  const comments = (d?.annotations ?? []).filter((a) => a.rel_type === 'comments')

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
          <Button size="compact-sm" onClick={() => setEditOpen(true)}>
            Edit
          </Button>
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

      <Collapse expanded={historyOpen}>
        <Card withBorder mb="sm">
          <Title order={6} mb="xs">
            History — append-only, latest wins
          </Title>
          {(history.data ?? []).map((v, i, all) => (
            <Group key={v.valid_from} gap="sm" mb={4}>
              <Text size="xs" ff="monospace" c="dimmed" w={30}>
                v{all.length - i}
              </Text>
              <Text size="xs" c="dimmed" w={190} ff="monospace">
                {v.valid_from}
              </Text>
              <Text size="sm">{v.name}</Text>
            </Group>
          ))}
        </Card>
      </Collapse>

      <Grid gap="md">
        <Grid.Col span={{ base: 12, lg: 6 }}>
          <Card withBorder mb="md">
            <Group justify="space-between" mb="xs">
              <Title order={6} tt="uppercase" c="dimmed">
                Description · text node
              </Title>
              <Button size="compact-xs" variant="subtle" onClick={() => setDescribeOpen(true)}>
                {description ? 'edit' : 'add'}
              </Button>
            </Group>
            {description ? (
              <MarkdownView markdown={description.content} />
            ) : (
              <Text size="sm" c="dimmed">
                no description yet
              </Text>
            )}
          </Card>
          {instructions && (
            <Card withBorder mb="md">
              <Title order={6} tt="uppercase" c="dimmed" mb="xs">
                Instructions · instructs → text node
              </Title>
              <MarkdownView markdown={instructions.content} />
            </Card>
          )}
          <Card withBorder>
            <Title order={6} tt="uppercase" c="dimmed" mb="xs">
              Comments · text nodes
            </Title>
            {comments.map((c) => (
              <div
                key={c.text_id}
                style={{ borderLeft: '2px solid #3B2449', paddingLeft: 12, marginBottom: 10 }}
              >
                <MarkdownView markdown={c.content} />
              </div>
            ))}
            <CommentComposer frag={frag} onPosted={refresh} />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 6 }}>
          <Card withBorder mb="md">
            <Title order={6} tt="uppercase" c="dimmed" mb="xs">
              Edges
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
          <Card withBorder mb="md">
            <Group justify="space-between" mb="xs">
              <Title order={6} tt="uppercase" c="dimmed">
                Attachments
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
          <Card withBorder>
            <Title order={6} tt="uppercase" c="dimmed" mb="xs">
              Attributes
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
                await updateEntity(frag, {
                  name: name.trim() ? name : null,
                  content: md.trim() ? md : null,
                  attributes_json: attrs.trim() ? attrs : null,
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
