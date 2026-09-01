// Module Worker glue for the from-scratch Rust/WASM engine. Loaded as
// `new Worker('/engine/worker.js', { type: 'module' })` — App.tsx talks to
// it exactly like the Stockfish worker: postMessage(string) in, one or more
// string `info`/`bestmove` lines out via postMessage.
import init, { WasmEngine } from './pkg/engine.js'

let engine = null
const ready = init().then(() => { engine = new WasmEngine() })

self.onmessage = async (event) => {
  await ready
  const line = typeof event.data === 'string' ? event.data : ''
  if (!line || !engine) return
  const output = engine.sendCommand(line)
  if (output) {
    output.split('\n').forEach((l) => { if (l) self.postMessage(l) })
  }
}
