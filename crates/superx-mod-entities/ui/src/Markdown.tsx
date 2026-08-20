import { forwardRef, useEffect, useImperativeHandle } from 'react'
import { useCreateBlockNote } from '@blocknote/react'
import { BlockNoteView } from '@blocknote/mantine'
import '@blocknote/mantine/style.css'

// The standard text editor (D-UI5): BlockNote with the markdown
// round-trip — the operator's proven openpraxis pattern
// (tryParseMarkdownToBlocks / blocksToMarkdownLossy). Markdown is the
// stored form: structured for agents, readable for humans and the CLI.

export type MarkdownEditorHandle = { getMarkdown: () => Promise<string> }

export const MarkdownEditor = forwardRef<MarkdownEditorHandle, { initial?: string }>(
  function MarkdownEditor({ initial }, ref) {
    const editor = useCreateBlockNote()
    useEffect(() => {
      let cancelled = false
      void (async () => {
        const blocks = await editor.tryParseMarkdownToBlocks(initial ?? '')
        if (!cancelled) editor.replaceBlocks(editor.document, blocks)
      })()
      return () => {
        cancelled = true
      }
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [])
    useImperativeHandle(ref, () => ({
      getMarkdown: async () => editor.blocksToMarkdownLossy(editor.document),
    }))
    return (
      <div style={{ border: '1px solid #3B2449', borderRadius: 4, background: '#150420' }}>
        <BlockNoteView editor={editor} theme="dark" />
      </div>
    )
  },
)

export function MarkdownView({ markdown }: { markdown: string }) {
  const editor = useCreateBlockNote()
  useEffect(() => {
    let cancelled = false
    void (async () => {
      const blocks = await editor.tryParseMarkdownToBlocks(markdown)
      if (!cancelled) editor.replaceBlocks(editor.document, blocks)
    })()
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [markdown])
  return <BlockNoteView editor={editor} editable={false} theme="dark" />
}
