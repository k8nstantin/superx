import React from 'react'
import ReactDOM from 'react-dom/client'
import { createTheme, MantineProvider } from '@mantine/core'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import '@mantine/core/styles.css'
import App from './App'

const queryClient = new QueryClient()

// The swindex design system — the same tokens as the SuperX website
// (superx-mod-website/style.css): midnight surfaces, pelican-purple
// accents, Space Grotesk headings, JetBrains Mono code.
const theme = createTheme({
  fontFamily: 'system-ui, -apple-system, Segoe UI, Arial, sans-serif',
  fontFamilyMonospace: "'JetBrains Mono', 'Fira Code', ui-monospace, SFMono-Regular, Menlo, monospace",
  headings: { fontFamily: "'Space Grotesk', system-ui, -apple-system, Segoe UI, sans-serif" },
  primaryColor: 'pelican',
  primaryShade: 5,
  colors: {
    // #9500BF (accent) → #CC66FF (accent-bright) family.
    pelican: [
      '#F6E8FF', '#EBD0FF', '#DDA9FF', '#CC66FF', '#B833E8',
      '#9500BF', '#8300A8', '#630080', '#4C0063', '#38004A',
    ],
    // Mantine's dark scale re-tuned to the site's midnight purple:
    // dark[7] = --bg #1F062A (body), dark[8] = --bg-alt #150420.
    dark: [
      '#EDE4F4', '#CFC2DB', '#B0A0C2', '#8F7BA5', '#5C4470',
      '#3B2449', '#2A1235', '#1F062A', '#150420', '#0E0216',
    ],
  },
})

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <MantineProvider theme={theme} defaultColorScheme="dark">
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>
    </MantineProvider>
  </React.StrictMode>,
)
