import { useEffect, useRef, useState } from 'react'
import { Button, Group, Text } from '@mantine/core'
import { MarkdownView } from './Markdown'

// A text node's content has no length limit — a product's description
// is a whole build spec (35k chars and climbing). Rendered in full it
// buries everything under it: the operator scrolled past a spec looking
// for the comments and concluded they had been lost (#261).
//
// So long content is clipped to a preview and opens on click. Short
// content renders exactly as before, with no chrome at all — a
// two-line comment must not grow an expand button.

/** Clip height in px before a preview + toggle takes over. */
const CLIP = 180
/** Slack above the clip: not worth a toggle for a few stray pixels. */
const SLACK = 24

export function LongText({
  markdown,
  collapsedHeight = CLIP,
  defaultExpanded = false,
  compact = false,
}: {
  markdown: string
  collapsedHeight?: number
  defaultExpanded?: boolean
  /** Shrink headings, for a preview whose title is already on a header line. */
  compact?: boolean
}) {
  const [expanded, setExpanded] = useState(defaultExpanded)
  const [overflows, setOverflows] = useState(false)
  const inner = useRef<HTMLDivElement>(null)

  // MarkdownView parses the markdown in an effect, so the content lands
  // AFTER mount and keeps growing as blocks render. A one-shot measure
  // reads an empty editor and hides the toggle on a huge body — hence
  // the observer rather than a single read.
  //
  // The verdict LATCHES, and that is load-bearing: clipping changes the
  // styles of the very element being measured (the compact class below
  // shrinks headings), so an unlatched measure could clip, shrink, fit,
  // unclip, grow, and oscillate. Content that ever exceeded the clip
  // has earned its toggle. A new body re-opens the question.
  useEffect(() => {
    const el = inner.current
    if (!el) return
    let latched = false
    const measure = () => {
      if (latched || el.scrollHeight <= collapsedHeight + SLACK) return
      latched = true
      setOverflows(true)
    }
    setOverflows(false)
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [collapsedHeight, markdown])

  const clipped = overflows && !expanded

  return (
    <div>
      <div
        style={{
          maxHeight: clipped ? collapsedHeight : undefined,
          overflow: clipped ? 'hidden' : undefined,
          // Fade the content itself rather than laying a matching
          // gradient over it: a mask needs no background colour, so it
          // stays correct on a card, in a modal, or on any surface.
          maskImage: clipped ? 'linear-gradient(to bottom, black 55%, transparent 100%)' : undefined,
          WebkitMaskImage: clipped
            ? 'linear-gradient(to bottom, black 55%, transparent 100%)'
            : undefined,
        }}
      >
        <div ref={inner} className={compact && clipped ? 'sx-compact' : undefined}>
          <MarkdownView markdown={markdown} />
        </div>
      </div>
      {overflows && (
        <Group gap="xs" mt={6}>
          <Button size="compact-xs" variant="subtle" onClick={() => setExpanded((e) => !e)}>
            {expanded ? 'collapse ▴' : 'expand ▾'}
          </Button>
          <Text size="xs" c="dimmed">
            {markdown.length.toLocaleString()} chars
          </Text>
        </Group>
      )}
    </div>
  )
}

/**
 * A one-line label for a collapsed body: its first heading if it has
 * one, else its first sentence of prose. Markdown syntax is stripped so
 * a comment's header reads as a title and not as `## Proposal — …`.
 */
export function previewLine(markdown: string, max = 72): string {
  const lines = markdown.split('\n').map((l) => l.trim())
  const heading = lines.find((l) => l.startsWith('#'))
  const prose = lines.find((l) => l.length > 0 && !l.startsWith('#') && !l.startsWith('```'))
  const raw = (heading ?? prose ?? '').replace(/^#+\s*/, '').replace(/[*_`]/g, '')
  return raw.length > max ? `${raw.slice(0, max - 1)}…` : raw
}
