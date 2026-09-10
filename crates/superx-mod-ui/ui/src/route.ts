// Where the dashboard is, as the URL hash: `#page=sessions&session=<uuid>`
// is one session's feed, `#page=status&range=24h` the cockpit at a range.
// Four pages and two parameters need no router — a hash survives a
// reload, walks with Back and Forward, and is a link a pilot can paste.
// The Status range lived here first (#369); the page and the session
// joined it so a session on the cockpit can be a link (#378).

export const PAGES = ['Status', 'Activity', 'Sessions', 'Console'] as const
export type Page = (typeof PAGES)[number]

export function readHash(): Record<string, string> {
  const out: Record<string, string> = {}
  const h = window.location.hash.replace(/^#/, '')
  if (!h) return out
  for (const part of h.split('&')) {
    const [k, ...rest] = part.split('=')
    if (k) out[decodeURIComponent(k)] = decodeURIComponent(rest.join('='))
  }
  return out
}

/** Write the hash. `push` makes a history entry (a navigation); without
 *  it the current entry is rewritten (a refinement, like the range).
 *  Either way every listener hears `hashchange`. */
export function writeHash(params: Record<string, string | null | undefined>, push = false): void {
  const pairs = Object.entries(params)
    .filter(([, v]) => v != null && v !== '')
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`)
  const next = pairs.length ? `#${pairs.join('&')}` : ''
  if (next === window.location.hash) return
  if (push && next) {
    window.location.hash = next
    return
  }
  window.history.replaceState(null, '', next || window.location.pathname)
  window.dispatchEvent(new HashChangeEvent('hashchange'))
}

export function pageFromHash(): Page {
  const p = (readHash().page ?? '').toLowerCase()
  return PAGES.find((x) => x.toLowerCase() === p) ?? 'Status'
}

export function sessionFromHash(): string | null {
  return readHash().session ?? null
}

/** Go to a page. The range rides along so Status lands where it was;
 *  a session does not — a page is a fresh place. */
export function goToPage(page: Page): void {
  const { range } = readHash()
  writeHash({ page: page.toLowerCase(), range }, true)
}

/** Open one session's feed — the same place the Sessions menu opens
 *  it, reached from anywhere the session's uuid is shown (#378). */
export function openSession(sessionId: string): void {
  const { range } = readHash()
  writeHash({ page: 'sessions', session: sessionId, range }, true)
}
