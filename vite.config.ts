import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://vite.dev/config/
// COOP/COEP mirror vercel.json's production headers — needed locally too,
// since the Lazy-SMP engine thread pool requires a cross-origin-isolated
// page (SharedArrayBuffer) in dev the same as in prod.
export default defineConfig({
  plugins: [react()],
  server: {
    headers: {
      'Cross-Origin-Opener-Policy': 'same-origin',
      'Cross-Origin-Embedder-Policy': 'credentialless',
    },
  },
})
