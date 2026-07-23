import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'
import path from 'path'

// Generic vendor bucketing: keyed off the package path under node_modules.
// Do NOT name dead Base44 deps here (lane 1 may delete them → build error).
// Only packages that actually resolve into the graph land in a chunk.
// No catch-all "vendor" bucket: dumping leftovers there created a charts↔vendor
// cycle (recharts deps ↔ shared utils). Unbucketed packages stay with importers.
function manualChunks(id) {
  const norm = id.replace(/\\/g, '/')
  if (!norm.includes('node_modules/')) return
  const after = norm.split('node_modules/').pop()
  if (!after) return
  const parts = after.split('/')
  const pkg = parts[0].startsWith('@') ? `${parts[0]}/${parts[1]}` : parts[0]

  if (pkg === 'react' || pkg === 'react-dom' || pkg === 'scheduler') return 'react'
  if (pkg.startsWith('@radix-ui/')) return 'radix'
  if (pkg.startsWith('@xterm/')) return 'xterm'
  // recharts + its runtime graph (Home PerformanceWidget + Monitoring charts).
  // lodash is only pulled by recharts here (zero direct importers in ui/src).
  if (
    pkg === 'recharts' ||
    pkg === 'victory-vendor' ||
    pkg === 'decimal.js-light' ||
    pkg === 'internmap' ||
    pkg === 'react-smooth' ||
    pkg === 'recharts-scale' ||
    pkg === 'eventemitter3' ||
    pkg === 'lodash' ||
    pkg === 'lodash-es' ||
    pkg.startsWith('d3-')
  ) {
    return 'charts'
  }
  if (pkg === 'lucide-react') return 'icons'
  if (pkg === 'react-router' || pkg === 'react-router-dom' || pkg === '@remix-run/router') {
    return 'router'
  }
  if (pkg.startsWith('@tanstack/')) return 'query'
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    react(),
  ],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  build: {
    // DISABLE minification (root-cause fix for the "frozen terminal / can't see what
    // I type" bug in installed builds). esbuild's minified output breaks xterm.js's
    // requestAnimationFrame render loop in the production bundle: the terminal paints
    // its first backlog frame (the welcome banner) then stops repainting, so keystroke
    // echo + streaming output never appear even though the backend delivers every byte
    // and term.write runs (proven by send_input/read/delta traces + advancing `since`).
    // Vite dev serves xterm UNminified, which is why `tauri dev` always typed fine and
    // made the backend-only PR #9 look sufficient. Re-enable later via a tuned minifier
    // (esbuild keepNames / terser mangle:false) once bundle size matters.
    minify: false,
    rollupOptions: {
      output: {
        manualChunks,
      },
    },
  },
});
