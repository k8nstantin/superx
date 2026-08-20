import { forwardRef, useEffect, useImperativeHandle } from 'react'
import { BlockNoteViewEditor, FormattingToolbar, useCreateBlockNote } from '@blocknote/react'
import { BlockNoteView, type Theme } from '@blocknote/mantine'
import '@blocknote/mantine/style.css'
import './editor.css'

// The standard text editor (D-UI5): BlockNote with the markdown
// round-trip — the operator's proven openpraxis pattern
// (tryParseMarkdownToBlocks / blocksToMarkdownLossy). Markdown is the
// stored form: structured for agents, readable for humans and the CLI.
//
// The toolbar is STATIC — rendered above the writing area, always
// visible (renderEditor={false} hands us the layout; BlockNoteViewEditor
// puts the content back). The library's default only floats on
// selection, which reads as "no editor here at all".

const SWINDEX: Theme = {
  colors: {
    editor: { text: '#EDE4F4', background: '#150420' },
    menu: { text: '#CFC2DB', background: '#2A1235' },
    tooltip: { text: '#CFC2DB', background: '#2A1235' },
    hovered: { text: '#EDE4F4', background: '#3B2449' },
    selected: { text: '#FFFFFF', background: '#9500BF' },
    disabled: { text: '#5C4470', background: '#2A1235' },
    shadow: '#0D0212',
    border: '#3B2449',
    sideMenu: '#8F7BA5',
  },
  borderRadius: 6,
  fontFamily: 'system-ui, -apple-system, Segoe UI, Arial, sans-serif',
}

export type MarkdownEditorHandle = { getMarkdown: () => Promise<string> }

export const MarkdownEditor = forwardRef<
  MarkdownEditorHandle,
  { initial?: string; minHeight?: number; maxHeight?: string }
>(function MarkdownEditor({ initial, minHeight = 220, maxHeight = '60vh' }, ref) {
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
    <div
      className="sx-editor"
      style={
        {
          '--sx-editor-min-h': `${minHeight}px`,
          '--sx-editor-max-h': maxHeight,
        } as React.CSSProperties
      }
    >
      <BlockNoteView
        editor={editor}
        theme={SWINDEX}
        renderEditor={false}
        formattingToolbar={false}
      >
        <FormattingToolbar />
        <BlockNoteViewEditor />
      </BlockNoteView>
    </div>
  )
})

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
  return (
    <div className="sx-view">
      <BlockNoteView editor={editor} editable={false} theme={SWINDEX} sideMenu={false} />
    </div>
  )
}
