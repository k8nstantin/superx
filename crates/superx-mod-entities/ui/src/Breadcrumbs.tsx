import { createContext, useContext, useEffect, useMemo, useState } from 'react'
import { Anchor, Breadcrumbs, Text } from '@mantine/core'

// The header trail (issue #253). The 52px header had a brand on the
// left, a caption on the right, and nothing between — this fills it
// with where you are. Only the mounted page publishes, so switching
// pages replaces the trail instead of merging two of them.

export type Crumb = { label: string; onClick?: () => void }

type Ctx = { crumbs: Crumb[]; publish: (c: Crumb[]) => void }
const BreadcrumbCtx = createContext<Ctx>({ crumbs: [], publish: () => {} })

export function BreadcrumbProvider({ children }: { children: React.ReactNode }) {
  const [crumbs, publish] = useState<Crumb[]>([])
  const value = useMemo(() => ({ crumbs, publish }), [crumbs])
  return <BreadcrumbCtx.Provider value={value}>{children}</BreadcrumbCtx.Provider>
}

/** Publish this page's trail. Labels are the dependency, so a
 *  re-render with the same labels does not re-publish. */
export function useBreadcrumb(crumbs: Crumb[]) {
  const { publish } = useContext(BreadcrumbCtx)
  const key = crumbs.map((c) => c.label).join('␟')
  useEffect(() => {
    publish(crumbs)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key])
}

/** The rendered trail — home glyph first, current step unlinked. */
export function BreadcrumbTrail({ onHome }: { onHome?: () => void }) {
  const { crumbs } = useContext(BreadcrumbCtx)
  const items: Crumb[] = [{ label: '⌂', onClick: onHome }, ...crumbs]
  return (
    <Breadcrumbs
      separator="›"
      separatorMargin={8}
      styles={{
        root: { flexWrap: 'nowrap', overflow: 'hidden' },
        separator: { color: 'var(--mantine-color-dimmed)' },
      }}
    >
      {items.map((c, i) => {
        const last = i === items.length - 1
        if (last || !c.onClick)
          return (
            <Text
              key={`${c.label}-${i}`}
              size="sm"
              c={last ? undefined : 'dimmed'}
              fw={last ? 600 : 400}
              style={{ whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', maxWidth: 320 }}
              title={c.label}
            >
              {c.label}
            </Text>
          )
        return (
          <Anchor
            key={`${c.label}-${i}`}
            size="sm"
            c="dimmed"
            onClick={c.onClick}
            style={{ whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', maxWidth: 320 }}
            title={c.label}
          >
            {c.label}
          </Anchor>
        )
      })}
    </Breadcrumbs>
  )
}
