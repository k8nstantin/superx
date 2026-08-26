import { Tooltip } from '@mantine/core'

// Liveness as a dot, shared by the Sessions list and Status's live
// panel (#343). It lived inside Sessions.tsx, and its keyframes were
// injected by that page's render — so the same dot drawn anywhere else
// resolved `sx-glow` to nothing and sat there static. The component
// now carries its own keyframes and travels.
export type Liveness = 'active' | 'paused' | 'ended' | 'unknown'

const GLOW = '@keyframes sx-glow { 0%, 100% { box-shadow: 0 0 4px 1px rgba(48,209,88,0.5); } 50% { box-shadow: 0 0 10px 4px rgba(48,209,88,0.9); } }'

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
      <style>{GLOW}</style>
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
