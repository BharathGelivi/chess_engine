# CLAUDE.md

Read this first, every session, before grepping/exploring the codebase. It
should answer "where does X live" without a search. Update it whenever a
change moves/renames/adds something this file describes — keep it current,
not historical (git log is the history).

## What this is
"BOARDROOM" — a single-page React chess analysis board. Import a PGN, step
through it, and get live Stockfish suggestions at any position. Vite + React
19 + TypeScript, no backend, no router, no state library.

## Where everything lives
- [src/App.tsx](src/App.tsx) — the entire app. One component, no
  sub-components, no other src files besides the two below. All state
  (game, history, viewIndex, engine lines) lives here in `useState`.
- [src/App.css](src/App.css) — all app styling (dark theme, board/piece/panel
  layout). `.hero`/`#next-steps`/`#docs`/etc. at the bottom are unused
  Vite-template leftovers, not part of the app.
- [src/index.css](src/index.css), [src/main.tsx](src/main.tsx) — Vite
  boilerplate entry point, rarely relevant.
- [public/stockfish/](public/stockfish/) — Stockfish 18, single-threaded WASM
  build (`stockfish-18-single.js` + `.wasm`), loaded as a Web Worker at
  `/stockfish/stockfish-18-single.js`. This is the only engine dependency;
  the `stockfish` npm package in package.json is unused at runtime (the
  worker file is fetched directly from `public/`, not imported).
- [docs/BEATING_STOCKFISH.md](docs/BEATING_STOCKFISH.md) — reference doc on
  Stockfish internals (search/NNUE) and realistic paths to build something
  stronger. Not app code — read only if asked about engine internals.
- [docs/ENGINE_ARCHITECTURE.md](docs/ENGINE_ARCHITECTURE.md) and
  [docs/ENGINE_IMPLEMENTATION_PLAN.md](docs/ENGINE_IMPLEMENTATION_PLAN.md)
  — design doc and phased build plan for a from-scratch engine. Phases 0-4
  (classical search + handcrafted eval, everything below) are implemented.
  Phases 5-7 (NNUE training on an RTX 4060, `training/`) are **not**
  implemented yet — `eval.rs` is the handcrafted eval described in
  ENGINE_ARCHITECTURE.md §4 and is the final eval for now, not a stub.
- [engine/](engine/) — the from-scratch chess engine, a separate Rust
  crate (Phases 0-4 of the implementation plan, plus an opening book and
  Lazy-SMP threading beyond the plan — see below). `cargo test` from
  `engine/` runs the perft correctness suite; `cargo run --release`
  prints perft + search benchmark numbers; `cargo run --release -- uci`
  is a stdin/stdout UCI-lite loop for manual poking. Module layout:
  `board.rs` (bitboards, FEN, Zobrist — copy-make position, no
  make/unmake), `movegen.rs` (magic bitboards generated at process
  startup via trial-and-error search, not hardcoded constants; pseudo-legal
  gen + brute-force legality via make-move-and-check-test; also
  `parse_uci_move`, shared by `uci.rs` and `book.rs`), `book.rs` (opening
  book — a small hardcoded set of well-known lines, keyed by each
  position's Zobrist hash rather than move sequence so transpositions still
  hit; `uci.rs`'s `go` handler checks it before searching and returns the
  book move instantly, at whatever `evaluate()` reports with no search),
  `perft.rs`, `eval.rs` (material + PST + mobility + king safety),
  `search.rs` (negamax/alpha-beta, iterative deepening, TT, MVV-LVA/
  killers/history ordering, quiescence, PVS, null-move, LMR, aspiration
  windows — every Phase-3/4 technique is a bool in `SearchOptions` for
  isolated benchmarking — plus Lazy-SMP: `iterative_deepening_mt` spawns
  `num_threads - 1` helper `Search` instances via `rayon::scope`, all
  sharing one transposition table (`Arc<Vec<Mutex<Option<TTEntry>>>>`, one
  mutex per slot) and one `time_up` flag (`Arc<AtomicBool>`, fresh per `go`
  call); only the calling thread's result is reported, helpers exist to
  seed the shared TT with transposition hits. `iterative_deepening`
  (single-threaded) is kept as-is and still used directly by the
  Phase3-vs-Phase4 benchmark harness, so those comparisons stay
  apples-to-apples), `uci.rs` (UCI-lite handler, same `info`/`bestmove`
  line shape as Stockfish; `handle_go` picks `num_threads =
  rayon::current_num_threads().min(4)` — capped since shared-TT lock
  contention makes more threads a net loss well before using a big
  machine's full core count), `lib.rs` (wasm-bindgen `WasmEngine` export,
  plus `pub use wasm_bindgen_rayon::init_thread_pool` on wasm32 — exposes
  `initThreadPool(n)` in the generated JS glue, see `public/engine/
  worker.js`), `main.rs` (native `[[bin]]`, not built to WASM — fast
  iteration target; also gets Lazy-SMP for free via the same `Engine`,
  same 4-thread cap from `handle_go`).
  `engine/.cargo/config.toml` sets the `wasm32-unknown-unknown`-only
  rustflags threading needs (atomics/shared-memory/TLS-export linker args
  — see its comments for what each does); it's scoped to that one target
  triple, so native `cargo build`/`cargo test` are unaffected and need no
  special toolchain. Build to WASM with **`./build-wasm.sh`** from inside
  `engine/` (not a bare `wasm-pack build` — see below).
- [engine/build-wasm.sh](engine/build-wasm.sh) — wraps the WASM build
  (nightly + `-Z build-std`, needed because stable's prebuilt std has no
  atomics) and two post-build fixups wasm-pack redoes from scratch every
  run, both bitten this project already: (1) deletes the nested
  `pkg/.gitignore` wasm-pack regenerates (just `*` — silently excludes the
  whole engine from git, which is what 404'd "Overboard Engine" in prod
  once already); (2) patches wasm-bindgen-rayon's generated
  `workerHelpers.js`, which does a bundler-style directory import
  (`import('../../..')`) to reach `engine.js` — resolves fine under a
  bundler's package resolution, but this app loads `pkg/` as raw static
  files with no bundler in the loop, so that import 404s in a real
  browser; patched to the explicit `../../../engine.js` path. Always use
  this script, never call `wasm-pack build` directly, or both gotchas come
  back on the next build.
- [public/engine/](public/engine/) — WASM engine deployment: `worker.js`
  (hand-written module-Worker glue, loaded as `new Worker('/engine/worker.js',
  { type: 'module' })`; imports the wasm-pack output from `./pkg/` and
  forwards UCI-lite lines exactly like the Stockfish Worker does; also
  calls and awaits `initThreadPool(...)`, capped at 4, before constructing
  `WasmEngine` — this is what actually spins up the Lazy-SMP search's
  helper-thread Web Workers) plus `pkg/` (wasm-pack's generated output —
  `.wasm` + JS glue, produced by `build-wasm.sh`). Unlike a typical build
  artifact, `pkg/` **is** committed: Vercel's build has no Rust/cargo
  toolchain, so if it's not in git it's simply absent from the deployment
  (this bit the project once — see `build-wasm.sh` above). Re-run
  `build-wasm.sh` and commit the changed files under `pkg/` whenever
  `engine/` changes.
- [vercel.json](vercel.json) — sets `Cache-Control: public, max-age=31536000,
  immutable` on `/stockfish/*` and `/engine/*`. Vercel's default for
  `public/` static files is `max-age=0, must-revalidate`, which forces a
  network round-trip on every load even when the ~110MB Stockfish binary
  hasn't changed — the immutable header lets the browser skip that entirely
  after the first load. Trade-off: if either binary is ever updated, the
  filename must change too (cache-busting), since browsers holding the old
  immutable response will never revalidate it. Also sets
  `Cross-Origin-Opener-Policy: same-origin` +
  `Cross-Origin-Embedder-Policy: credentialless` on every route — required
  for `crossOriginIsolated`/`SharedArrayBuffer`, which the Lazy-SMP thread
  pool needs. `credentialless` (not `require-corp`) deliberately: it
  cross-origin-isolates the page without requiring every cross-origin
  resource (the Google Fonts stylesheet in `index.html`) to opt in with
  CORP/CORS headers of its own — lower risk of silently breaking an
  unrelated resource load. `vite.config.ts` sets the same two headers on
  the dev server for local parity.

## How the app works (src/App.tsx)
- **Chess state**: `chess.js` `Chess` instance. `game` = the live/imported
  game; `history` = flat array of `Move` objects (the *current* line, which
  can be a branch — see below); `viewIndex` = which ply is currently
  displayed. `viewGame` (a `useMemo`) replays `history` up to `viewIndex`
  from scratch each render — this is the position actually shown/analyzed.
- **Branching / what-if moves**: `makeMove(from, to)` plays from `viewGame`
  (the currently *viewed* position, not necessarily the end of history). If
  `viewIndex !== history.length`, this truncates history at `viewIndex` and
  appends the new move — i.e. clicking a move mid-history starts a new line
  from that point (standard analysis-board branching, à la Lichess). There
  is no tree of saved branches; branching overwrites the discarded future.
- **Engine wiring**: one `useEffect` spins up the Stockfish Worker once
  (`MultiPV 3`), parses UCI `info` lines via regex (`depth`, `score`,
  `pv`) into `engineState.lines`. A second `useEffect` re-sends
  `position fen ...` + `go depth 18` whenever `viewGame` changes. No
  cleanup/cancellation between analyses beyond `stop` — the parser is
  keyed by first-move so stale lines get replaced, not appended.
- **Board rendering**: `board` array is built **row-major**,
  `rank 8→1 outer, file a→h inner` (`Array.from({length:8},(_,row)=>
  files.map(file=>...)).flat()`). This order must match the CSS grid
  (`grid-template-columns: repeat(8, 1fr)`, default row-flow) or square
  labels silently land on the wrong cells — this bit the project once
  already (file-major array + row-major grid = every label showed "8"/"h").
  Rank label = first column of each row (`index % 8 === 0`); file label =
  bottom row (`index >= 56`).
- **Piece rendering**: both colors use the *solid* unicode chess glyph set
  (`♚♛♜♝♞♟`) — the "white" glyph set (`♔♕♖...`) renders hollow/outlined in
  most fonts, so it's deliberately not used. Color/fill comes from CSS
  (`.white-piece`/`.black-piece` with `-webkit-text-stroke`), not from
  picking a different glyph per color.
- **Square selection**: click-to-move via DOM `dataset.selected`
  (`document.querySelector('[data-selected="true"]')`), not React state —
  intentional for this single-file scope, not an oversight.
- **Engine picker (analysis mode)**: `engineChoice` state (`'stockfish' |
  'ours'`) selects which Worker script the single analysis Worker loads —
  `/stockfish/stockfish-18-single.js` (classic Worker) or
  `/engine/worker.js` (module Worker, wraps the WASM engine in
  `engine/`). The Worker-creation `useEffect` now depends on
  `engineChoice` and recreates the Worker on switch; the existing
  `info`-line regex parser is unchanged since both engines emit the same
  line shape. Our engine only emits one PV line (no MultiPV). Live
  analysis sends Stockfish a fixed `go depth 18` but our engine a time
  budget (`go movetime 3000`, `analysisBudget` map in App.tsx) — depth is
  a bad knob for our slower copy-make search since a fixed depth was
  either too shallow (weak tactics) or too slow depending on position;
  movetime lets iterative deepening go as deep as the budget allows, and
  the `stop` sent on every position change cancels a stale search instead
  of queuing one up.
- **Engine vs engine autoplay**: `mode` state (`'analysis' | 'autoplay'`)
  toggles between the existing single-engine analysis panel and a second
  panel. Autoplay spawns two independent engine Workers (`spawnReadyEngine`,
  one per color, any Stockfish/ours combination) and drives them in turn
  via `runAutoplayLoop`, appending each ply to the same `history`/
  `viewIndex` state the manual board already uses — so the existing board
  rendering just follows along, no separate board state. Fixed
  `AUTOPLAY_MOVETIME_MS` (500ms) per move and an `AUTOPLAY_MAX_PLIES` (200)
  safety cap. Start/Stop/Reset control an `autoplayStopRef` flag (checked
  between plies) plus worker teardown; result text covers checkmate/
  stalemate/repetition/insufficient-material/50-move-draw/cap/stopped.
  Manual board clicks (`makeMove`) are disabled while `mode === 'autoplay'`.
  The analysis-mode Worker keeps running during autoplay (separate Worker
  instances, no conflict) — coexistence, not replacement, per the
  ENGINE_ARCHITECTURE.md §5 "second Worker option" decision.

## Conventions already established here
- Everything is one file by design (small app) — don't split `App.tsx` into
  components/hooks unless a change genuinely requires it.
- No comments explaining *what* code does; comments only mark non-obvious
  *why* (see the board row-major note above as the template).
- Dark theme only, no light-mode variant.
- No test suite currently exists.

## Commands
`npm run dev` (Vite dev server) · `npm run build` (`tsc -b && vite build`) ·
`npm run lint` (oxlint) · `npm run preview`.
