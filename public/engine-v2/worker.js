// Module Worker glue for the from-scratch Rust/WASM engine. Loaded as
// `new Worker('/engine-v2/worker.js', { type: 'module' })` — App.tsx talks to
// it exactly like the Stockfish worker: postMessage(string) in, one or more
// string `info`/`bestmove` lines out via postMessage.
//
// initThreadPool spins up the Web Workers rayon's Lazy-SMP search (see
// engine/src/search.rs) schedules onto — it needs SharedArrayBuffer, which
// needs the page itself cross-origin isolated (COOP/COEP headers, see
// vercel.json). Capped at 4: matches the thread cap search.rs applies on
// its own side, no point spinning up workers past that.
import init, { WasmEngine, initThreadPool } from './pkg/engine.js'

let engine = null
const ready = init().then(async () => {
  await initThreadPool(Math.min(navigator.hardwareConcurrency || 4, 4))
  engine = new WasmEngine()
})

self.onmessage = async (event) => {
  await ready
  const line = typeof event.data === 'string' ? event.data : ''
  if (!line || !engine) return
  const output = engine.sendCommand(line)
  if (output) {
    output.split('\n').forEach((l) => { if (l) self.postMessage(l) })
  }
}
