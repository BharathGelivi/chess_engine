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
  crate (Phases 0-4 of the implementation plan). `cargo test` from
  `engine/` runs the perft correctness suite; `cargo run --release`
  prints perft + search benchmark numbers; `cargo run --release -- uci`
  is a stdin/stdout UCI-lite loop for manual poking. Module layout:
  `board.rs` (bitboards, FEN, Zobrist — copy-make position, no
  make/unmake), `movegen.rs` (magic bitboards generated at process
  startup via trial-and-error search, not hardcoded constants; pseudo-legal
  gen + brute-force legality via make-move-and-check-test), `perft.rs`,
  `eval.rs` (material + PST + mobility + king safety), `search.rs`
  (negamax/alpha-beta, iterative deepening, TT, MVV-LVA/killers/history
  ordering, quiescence, PVS, null-move, LMR, aspiration windows — every
  Phase-3/4 technique is a bool in `SearchOptions` for isolated
  benchmarking), `uci.rs` (UCI-lite handler, same `info`/`bestmove` line
  shape as Stockfish), `lib.rs` (wasm-bindgen `WasmEngine` export),
  `main.rs` (native `[[bin]]`, not built to WASM — fast iteration target).
  Build to WASM with `wasm-pack build --target web --out-dir
  ../public/engine/pkg` from inside `engine/`.
- [public/engine/](public/engine/) — WASM engine deployment: `worker.js`
  (hand-written module-Worker glue, loaded as `new Worker('/engine/worker.js',
  { type: 'module' })`; imports the wasm-pack output from `./pkg/` and
  forwards UCI-lite lines exactly like the Stockfish Worker does) plus
  `pkg/` (wasm-pack's generated output — `.wasm` + JS glue — not checked
  in by hand, produced by the build command above).

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
  line shape. Our engine only emits one PV line (no MultiPV), and runs at
  a shallower live-analysis depth (`analysisDepth` map in App.tsx) since
  its copy-make search is far slower than Stockfish's tuned C++.
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
