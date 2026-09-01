// wasm-bindgen entry point. Mirrors how the bundled Stockfish Worker talks
// UCI: one text command in, UCI-lite text lines out.
//
// ponytail: `send_command` runs the whole command synchronously and returns
// all output lines newline-joined, rather than streaming `info` lines out
// via postMessage as they're produced (which would need web-sys's
// DedicatedWorkerGlobalScope bindings). The public/engine/worker.js glue
// splits the return value on '\n' and posts each line, so App.tsx's
// existing per-line parser still works — the only user-visible difference
// from Stockfish is that intermediate `info depth N` lines for a slow `go`
// all arrive at once when the search finishes, instead of trickling in.
// Upgrade path: add web-sys + call self.post_message() from inside the
// iterative-deepening callback if live mid-search depth updates are wanted.

pub mod board;
pub mod eval;
pub mod movegen;
pub mod perft;
pub mod search;
pub mod uci;

use uci::Engine;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct WasmEngine {
    inner: Engine,
}

#[wasm_bindgen]
impl WasmEngine {
    #[wasm_bindgen(constructor)]
    pub fn new() -> WasmEngine {
        WasmEngine { inner: Engine::new() }
    }

    /// Send one UCI-lite command line; returns all resulting output lines
    /// joined with '\n' (empty string if the command produced no output).
    #[wasm_bindgen(js_name = sendCommand)]
    pub fn send_command(&mut self, line: &str) -> String {
        let mut out = Vec::new();
        self.inner.handle_line(line, &mut |s| out.push(s));
        out.join("\n")
    }
}

impl Default for WasmEngine {
    fn default() -> Self { WasmEngine::new() }
}
