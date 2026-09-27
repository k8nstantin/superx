import { Tooltip } from '@mantine/core'

// Liveness as a dot, shared by the Sessions list and Status's live
// panel (#343). It lived inside Sessions.tsx, and its keyframes were
// injected by that page's render — so the same dot drawn anywhere else
// resolved `sx-glow` to nothing and sat there static. The component
// now carries its own keyframes and travels.
export type Liveness = 'active' | 'paused' | 'ended' | 'unknown'

// Liveness thresholds (render-layer presentation, issue #162):
// a session is ACTIVE while its newest message is fresher than this…
const ACTIVE_SECS = 5 * 60
// …PAUSED until this, ENDED after.
const PAUSED_SECS = 24 * 60 * 60

// For callers holding a timestamp. A caller whose row the SERVER has
// already cut to the activity window should pass `active` outright
// rather than re-deriving: re-deriving means two thresholds that must
// agree, and they did not — the server keeps `idle <= active_secs`
// while this returns `paused` at exactly the boundary (#344 review).
export function liveness(lastActive: string | null): Liveness {
  if (!lastActive) return 'unknown'
  const idleSecs = (Date.now() - new Date(lastActive).getTime()) / 1000
  if (idleSecs < ACTIVE_SECS) return 'active'
  if (idleSecs < PAUSED_SECS) return 'paused'
  return 'ended'
}

const STYLES: Record<Liveness, React.CSSProperties> = {
  // Alive: green, pulsing glow.
  active: {
    background: '#30d158',
    boxShadow: '0 0 6px 2px rgba(48,209,88,0.7)',
    animation: 'sx-glow 1.4s ease-in-out infinite',
  },
  paused: { background: '#fdd835' },
  // Stopped: flat red, no glow.
  ended: { background: '#e03131' },
  unknown: { background: '#555' },
}

export function LivenessDot({ state, size = 10 }: { state: Liveness; size?: number }) {
  return (
    <>
      {/* The `sx-glow` pulse is in global.css (#349). */}
      <Tooltip label={state} withArrow>
        <span
          style={{
            display: 'inline-block',
            width: size,
            height: size,
            borderRadius: '50%',
            verticalAlign: 'middle',
            flexShrink: 0,
            ...STYLES[state],
          }}
        />
      </Tooltip>
    </>
  )
}
