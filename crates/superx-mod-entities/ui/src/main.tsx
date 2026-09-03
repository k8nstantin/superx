import React from 'react'
import ReactDOM from 'react-dom/client'
import { createTheme, MantineProvider } from '@mantine/core'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import '@mantine/core/styles.css'
import App from './App'

// ONE RETRY, QUICKLY. With the library's default (three, backing off to
// seven seconds) a module that had gone away showed an empty box for the
// whole wait and, in practice, never reached the error state the page
// knows how to draw. A second attempt half a second later is enough to
// ride out a blip; after that, say what happened.
const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: 1, retryDelay: 400 } },
})

// The swindex design system — the same tokens as the core dashboard
// (superx-mod-ui/ui/src/main.tsx): midnight surfaces, pelican-purple
// accents, Space Grotesk headings, JetBrains Mono code. Per-module
// UIs match by convention (epic #216, D-UI3).
const theme = createTheme({
  fontFamily: 'system-ui, -apple-system, Segoe UI, Arial, sans-serif',
  fontFamilyMonospace: "'JetBrains Mono', 'Fira Code', ui-monospace, SFMono-Regular, Menlo, monospace",
  headings: { fontFamily: "'Space Grotesk', system-ui, -apple-system, Segoe UI, sans-serif" },
  primaryColor: 'pelican',
  primaryShade: 5,
  colors: {
    pelican: [
      '#F6E8FF', '#EBD0FF', '#DDA9FF', '#CC66FF', '#B833E8',
      '#9500BF', '#8300A8', '#630080', '#4C0063', '#38004A',
    ],
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
