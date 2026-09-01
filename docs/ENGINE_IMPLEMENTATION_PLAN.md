# BOARDROOM Engine: implementation plan

Read [ENGINE_ARCHITECTURE.md](ENGINE_ARCHITECTURE.md) first — this doc is
the phase-by-phase build sequence for the decisions made there. Each phase
has a goal, concrete deliverables, and a verification step; do not start a
phase until the previous one's verification passes. This ordering is
deliberate: correctness (perft) before speed (search), speed before
learned eval (NNUE), and every phase produces a benchmarkable artifact
rather than a pile of unverified code.

Project layout (new, separate from the React app per the "second Worker
option" decision):

```
chess engine/
├── src/App.tsx, App.css, ...        # existing React app, untouched until Phase 2
├── engine/                          # new: Rust crate, the search engine
│   ├── Cargo.toml
│   ├── src/
│   │   ├── board.rs                 # bitboard position representation
│   │   ├── movegen.rs               # move generation (magic bitboards)
│   │   ├── perft.rs                 # perft test harness
│   │   ├── search.rs                # negamax/alpha-beta/PVS/TT/etc.
│   │   ├── eval.rs                  # handcrafted eval (Phase 2-4), NNUE inference (Phase 7+)
│   │   ├── uci.rs                   # UCI-lite protocol handler
│   │   └── lib.rs                   # wasm-bindgen entry point
│   └── tests/
├── training/                        # new: Python, NNUE training (Phase 5+)
│   ├── generate_data.py             # self-play data generation (calls engine binary)
│   ├── train.py                     # PyTorch training loop
│   ├── quantize.py                  # float32 -> int8/int16 export
│   └── datasets/
└── docs/
    ├── ENGINE_ARCHITECTURE.md
    └── ENGINE_IMPLEMENTATION_PLAN.md  (this file)
```

## Phase 0 — Toolchain and skeleton

**Goal**: a Rust crate that builds to both a native binary (fast local
iteration/testing) and a WASM module (deployment target), doing nothing
but returning "hello" over both.

- Install Rust toolchain + `wasm-pack`.
- `cargo new engine --lib`, add `wasm-bindgen` dependency, confirm
  `wasm-pack build --target web` produces a loadable `.wasm` + glue JS.
- Add a native `[[bin]]` target for fast `cargo test`/`cargo run` iteration
  without going through WASM on every change — this matters a lot for
  perft testing in Phase 1, which is CPU-heavy and slow to iterate on
  through a browser Worker.
- Set up `cargo test` in CI-equivalent form (even if just a local script)
  so later phases have a place to add tests.

**Verification**: native binary runs and prints, `wasm-pack build`
succeeds, a trivial HTML/Worker smoke test loads the WASM module and calls
the hello function from JS.

## Phase 1 — Board representation and move generation (correctness)

**Goal**: bitboard position representation and fully correct move
generation, proven via perft — no search, no eval yet. This is the phase
where subtle bugs (en passant, castling through check, pinned-piece
legality, promotion) get caught, and it's much cheaper to catch them here
than after search is layered on top.

- `board.rs`: bitboard struct (one `u64` per piece-type×color, plus
  occupancy boards), FEN parsing/serialization, Zobrist hash keys
  (precomputed random numbers per piece/square/castling/en-passant/side,
  XORed to form the position hash — needed later for the TT, but generate
  once now).
- `movegen.rs`: magic bitboard tables for sliding pieces (bishop/rook/
  queen) — reuse well-known published magic numbers rather than spending
  time rediscovering them, since the magics themselves aren't the
  interesting part of this project. Pseudo-legal move generation for all
  piece types, then legality filtering (king not left in check).
- `perft.rs`: perft(depth) node counter.

**Verification**: perft results match published values for the standard
test positions (starting position, Kiwipete, and the other positions on
the [Chess Programming Wiki perft results
page](https://www.chessprogramming.org/Perft_Results)) to at least depth 5
(depth 6 for the starting position if time allows — this is the test that
actually stresses castling/en-passant/promotion edge cases, since they're
rare in shallow perft). Record NPS of pure move generation as a baseline
number for later comparison.

## Phase 2 — Naive search + handcrafted eval + first WASM integration

**Goal**: an engine that plays legal, not-embarrassing chess end to end,
wired into the app as a second engine option. Deliberately naive search
here (no TT, no advanced pruning yet) — the point of this phase is
plumbing correctness (search ↔ eval ↔ WASM ↔ Worker ↔ `App.tsx`) with the
simplest possible search, before adding search sophistication in Phase 3.

- `eval.rs`: handcrafted eval — material, piece-square tables, mobility,
  basic king safety (see [ENGINE_ARCHITECTURE.md
  §4](ENGINE_ARCHITECTURE.md#4-evaluation--handcrafted-first-then-nnue)).
- `search.rs`: plain negamax with alpha-beta pruning, fixed depth (no
  iterative deepening yet), no quiescence search yet (accept the horizon
  effect for now — it'll be visibly bad, that's expected and motivates
  Phase 3).
- `uci.rs`: minimal UCI-lite handler — `position fen`, `go depth N`,
  `bestmove`.
- `lib.rs`: `wasm-bindgen` exports wired to receive UCI-lite text commands
  and return responses via a JS callback, matching the message-passing
  shape of the existing Stockfish Worker.
- `App.tsx`: add an engine picker (Stockfish vs. this engine) and a second
  Worker pointed at the new WASM module; reuse the existing `info`-line
  parsing `useEffect` structure, adapted for the new engine's output.

**Verification**: play a full game against it manually in the app and
confirm it doesn't blunder pieces for free at low depth (material eval
alone should avoid one-move hangs); confirm switching the engine picker
correctly swaps which Worker receives `position`/`go` commands.

## Phase 3 — Search quality: TT, iterative deepening, ordering, quiescence

**Goal**: turn the naive Phase 2 search into a real search — this phase
does the most for playing strength per unit effort.

- Transposition table: Zobrist-keyed hash table (fixed-size, replace-by-
  depth or always-replace scheme — pick one, document why, revisit if
  benchmarking shows collisions hurting), storing depth/score/best-move/
  node-type (exact/lower-bound/upper-bound).
- Iterative deepening driving the search loop, using the TT's stored move
  from the previous depth for move ordering at the start of each new depth.
- Move ordering: TT move first, then MVV-LVA for captures, killer moves,
  history heuristic.
- Quiescence search at leaf nodes (captures only initially; checks can be
  added later if benchmarking shows tactical test suite failures trace to
  missing check extensions).
- Principal Variation Search (PVS): null-window search for non-first moves,
  full re-search on fail-high.

**Verification**: NPS and effective branching factor measured with each
feature toggled on/off individually (see [ENGINE_ARCHITECTURE.md
§6](ENGINE_ARCHITECTURE.md#6-benchmarking--how-we-know-any-of-this-actually-works))
— record the table, this is direct interview material. Run the STS
tactical test suite and record score as a baseline before Phase 4's
pruning additions (pruning can regress tactical accuracy if too aggressive,
so this baseline is what Phase 4 gets checked against).

## Phase 4 — Selective search: null-move, LMR, aspiration windows

**Goal**: the pruning techniques that trade a small correctness risk for
large speedup — added after Phase 3's exact search is solid, so any
strength regression from over-aggressive pruning is visible by comparing
against the Phase 3 baseline.

- Null-move pruning (with zugzwang-safety: disable/reduce in low-material
  or check positions).
- Late move reductions.
- Aspiration windows around the previous iteration's score.
- Futility pruning near leaf nodes (optional, add only if benchmarking
  shows remaining time budget is still the binding constraint after the
  above).

**Verification**: re-run the same NPS/branching-factor/STS-suite
benchmarks from Phase 3. Expect large NPS/branching-factor gains; watch
specifically for STS score regression, which would indicate pruning is too
aggressive somewhere — tune reduction/pruning margins if so. This is the
first point where a real Elo estimate is worth running: self-play gauntlet
(engine at depth N vs. itself at depth N-2, or vs. depth-capped Stockfish)
via `cutechess-cli`, establishing a baseline Elo number before the eval
function changes in later phases.

## Phase 5 — Training data generation

**Goal**: a labeled dataset for NNUE training, generated by the engine
built in Phases 1-4 playing itself.

- `training/generate_data.py`: drives the native engine binary (not the
  WASM build — native is faster for bulk generation) through many self-
  play games at a fixed depth, recording `(FEN, search score, game
  result)` for a sample of positions per game (not every position — sample
  to reduce correlation between adjacent positions in the same game, and
  bias sampling away from the opening/very early game where eval is least
  informative).
- Add random opening variation (e.g., a few random legal moves before the
  engine takes over each game) so self-play games don't collapse into
  repeating the same few lines — without this, the dataset badly
  under-covers the position space.
- Optionally blend in an external labeled dataset (e.g., positions from the
  [Lichess evaluation database](https://database.lichess.org/#evals)) if
  self-play volume after a reasonable generation run (order of magnitude:
  aim for a few hundred thousand to low millions of labeled positions,
  scaled to what's practical in available wall-clock time) is too thin for
  stable training — document in the training README which was used and why.

**Verification**: dataset size, label distribution (score histogram —
should be roughly centered near 0 with a spread, not collapsed to a few
values), and a train/validation/test split with no positions from the same
game crossing the split boundary (avoids leakage from near-duplicate
positions across the split).

## Phase 6 — NNUE architecture and training

**Goal**: a trained evaluation net, using the RTX 4060 for the part of this
project that's genuinely GPU-bound.

- `training/train.py`: PyTorch. Implement the HalfKP-style sparse feature
  encoder, the accumulator-first architecture described in
  [ENGINE_ARCHITECTURE.md
  §4](ENGINE_ARCHITECTURE.md#4-evaluation--handcrafted-first-then-nnue),
  and a training loop (batched, GPU-resident, standard regression loss
  against the labeled scores from Phase 5).
- Track training/validation loss curves; watch for overfitting (validation
  loss diverging from training loss) given the comparatively small dataset
  relative to Stockfish's own multi-billion-position training sets — if it
  shows up, the mitigations are more data (extend Phase 5 generation),
  a smaller net, or standard regularization (dropout/weight decay) —
  document whichever is used and why.
- Checkpoint the best validation-loss model.

**Verification**: validation-set prediction error (report the actual
number, e.g., mean squared error in centipawns, not just "it trained") and
a plot of predicted vs. actual score on a held-out sample. This is the
metric that answers "does the net actually work" before it ever touches
the engine.

## Phase 7 — Quantization and Rust inference integration

**Goal**: the trained net running inside the Rust engine, replacing the
Phase 2 handcrafted eval, fast enough for full-depth search.

- `training/quantize.py`: convert trained float32 weights to int8/int16,
  export as a flat binary file (documented format — offsets/sizes for each
  layer).
- `eval.rs`: NNUE inference path — load the exported weight file, implement
  the incremental accumulator update (the operation that makes this fast:
  most moves change only 1-2 pieces, so update the accumulator by
  adding/removing those features rather than recomputing from scratch),
  int8/int16 SIMD-friendly forward pass through the small dense layers.
- Keep the Phase 2 handcrafted eval in the codebase behind a build flag —
  it's the baseline the benchmarking in Phase 8 compares against, not dead
  code to delete.

**Verification**: inference NPS (should be materially faster per position
than a naive recompute-from-scratch forward pass — measure and report the
delta from the incremental accumulator specifically, e.g., by benchmarking
with it on vs. off); confirm quantized-net predictions are close to the
Phase 6 float32 validation numbers (small degradation expected and
acceptable, large degradation means a quantization bug).

## Phase 8 — Benchmarking and tuning

**Goal**: the headline numbers — this phase produces what actually gets
shown in an interview.

- Full benchmark suite run end to end: perft (Phase 1 regression check),
  NPS and branching factor with all search features (Phase 3-4 regression
  check), STS/Bratko-Kopec tactical suite score, and an Elo ladder via
  `cutechess-cli`:
  - this engine (handcrafted eval) vs. this engine (NNUE eval) — isolates
    the trained net's contribution.
  - this engine (final) vs. depth/time-capped Stockfish at a few different
    caps — gives a calibrated "where does this sit" answer instead of "it
    lost to Stockfish" (expected) or an unsupported strength claim.
- Write up the results as a benchmarking report (numbers + the
  before/after table for each search feature from Phases 3-4, plus the
  eval comparison) — this is the artifact worth having polished, more than
  any single code file.

**Verification**: this phase *is* the verification step for the whole
project — if a number here is missing or hand-wavy, that's the signal to
go back and instrument whatever step produced it.

## Phase 9 — Optional extensions (only after Phase 8 numbers exist)

Documented in [ENGINE_ARCHITECTURE.md
§7](ENGINE_ARCHITECTURE.md#7-recent-advancements-in-the-field-what-we-adopt-what-we-skip-and-why)
as deliberately deferred, not skipped-forever:

- Syzygy tablebase probing for endgames.
- Opening book (Polyglot format).
- Multithreaded search (would need `SharedArrayBuffer` + cross-origin-
  isolation headers on the deployed app — a real infrastructure change,
  not just an engine change).
- Self-play RL fine-tuning on top of the supervised NNUE net.

Pick these up only if there's a specific reason to (an interview question
you want a stronger answer for, or a specific benchmark weakness Phase 8
exposed) — this project's scope is already complete at Phase 8 in terms of
the stated goals in [ENGINE_ARCHITECTURE.md §0](ENGINE_ARCHITECTURE.md#0-goals-and-non-goals).
