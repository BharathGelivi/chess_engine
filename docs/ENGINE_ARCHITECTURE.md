# BOARDROOM Engine: architecture, rationale, and tradeoffs

This doc explains the engine we're building from scratch — not just how it
works, but *why* it's built this way, what we chose not to do and why, and
how recent chess-engine research informs the decisions. Read
[ENGINE_IMPLEMENTATION_PLAN.md](ENGINE_IMPLEMENTATION_PLAN.md) for the
phased build plan; read [BEATING_STOCKFISH.md](BEATING_STOCKFISH.md) first
if you haven't — it covers how the *bundled* Stockfish 18 works and is the
baseline this engine is designed against.

## 0. Goals and non-goals

**Goal**: a real engine you can defend in an interview — not a toy that
happens to make legal moves, and not a claim to have "beaten Stockfish."
Interview-defensible means: you can explain every component from first
principles, you made deliberate tradeoffs under real constraints (a laptop,
one RTX 4060 8GB, solo dev time), and you can show benchmark numbers, not
vibes.

**What "success" looks like**: an engine that
1. is provably correct (passes perft move-generation tests to depth 6+),
2. searches efficiently (measurable nodes/sec, effective branching factor
   reduction from pruning),
3. has a *trained* evaluation function, not just hand-tuned weights, and
4. has a quantified strength (Elo estimate from engine-vs-engine testing,
   not "seems strong").

**Non-goals**:
- Beating full-strength Stockfish 17+ outright. That engine is a ~10-year,
  multi-thousand-contributor, Fishtest-validated effort. Framing this
  project as "beat Stockfish" sets up a demo that fails; framing it as
  "build a real engine and understand exactly where it sits relative to
  Stockfish" is both honest and the more impressive interview story.
- A full self-play reinforcement learning loop (AlphaZero/Leela Zero style).
  That approach needs GPU-cluster-scale compute (Leela's training used
  thousands of GPU-years cumulatively) to reach strength competitive with
  handcrafted+NNUE engines. One 4060 8GB doing local training is real and
  useful for *supervised* NNUE training (small net, hundreds of thousands
  of positions) — it is not enough for self-play RL to converge to strong
  play in reasonable time. See §3 for the decision this drives.

## 1. High-level architecture

Two independent halves, deliberately decoupled:

```
┌─────────────────────────────┐        ┌──────────────────────────────┐
│  Search engine (Rust)        │        │  Eval net training (Python)   │
│  - board repr (bitboards)    │        │  - PyTorch, runs on RTX 4060  │
│  - move generation            │        │  - trains NNUE-style net on   │
│  - negamax + alpha-beta/PVS  │◄──export│    labeled positions          │
│  - transposition table       │ weights │  - quantizes to int8/int16    │
│  - quiescence search         │        │  - exports as flat weight file │
│  - iterative deepening       │        └──────────────────────────────┘
│  compiled → WASM             │
└───────────────┬───────────────┘
                │ Worker + UCI-lite protocol (same pattern as Stockfish)
                ▼
       src/App.tsx (existing React app, engine picker)
```

Why decoupled: the search engine needs to run in a browser Worker with no
GPU and no Python runtime. The eval net needs a GPU and a training loop that
has nothing to do with move generation. Coupling them (e.g., running
inference-only PyTorch in the browser via ONNX) would mean shipping a
much heavier runtime for no strength benefit — a small quantized NNUE
forward pass is faster in native/WASM code than in an ONNX interpreter.
This mirrors how Stockfish itself is structured: `nnue-pytorch` trains
completely separately from the C++ engine; only exported weights cross the
boundary.

## 2. Board representation — why bitboards

A chess position needs: piece placement, side to move, castling rights, en
passant target, halfmove clock. The two realistic representations:

| Representation | How it works | Tradeoff |
|---|---|---|
| **Array-of-squares** (what `chess.js`, used elsewhere in this app, does) | `board[64]` array of piece codes | Simple, easy to debug, but move generation means scanning arrays and slow "is this square attacked" checks — O(pieces) per query |
| **Bitboards** (what we're building) | Each piece-type×color is a `u64`, one bit per square. Sliding-piece attacks via magic bitboards (precomputed lookup tables indexed by occupancy) | Move generation and attack detection become bitwise ops (`AND`, `OR`, `POPCNT`) — orders of magnitude faster, at the cost of much less readable code and a nontrivial one-time cost to implement magic bitboard generation |

**Decision: bitboards.** This is the single highest-leverage architectural
choice for search speed, and it's what every competitive engine (Stockfish,
Leela, Ethereal, etc.) uses. `chess.js` in the existing React app is
array-based and that's the right choice *there* — it's a UI/rules layer,
not a search hot path, and correctness/readability matter more. A search
engine visits millions of positions per second; representation directly
gates how many.

Magic bitboards specifically (vs. simpler rotated bitboards or the
`u64`-per-direction ray approach): magics give O(1) sliding-piece attack
lookup via `(occupancy * magic) >> shift` into a precomputed table, at the
cost of a one-time magic-number search (well-documented, can reuse known
magic constants — no need to rediscover them) and ~2MB of lookup tables.
This is the current standard approach; PEXT bitboards (using the BMI2
`PEXT` instruction) are marginally faster on supporting CPUs but add
platform-specific code paths that don't help in WASM, which doesn't expose
PEXT — so plain magic bitboards are the right call here specifically
*because* the target is WASM.

## 3. Search — why classical alpha-beta, not neural MCTS

This is the decision the "what should the engine be" question in this
project comes down to, so it's worth stating the reasoning explicitly
rather than just asserting it.

| | Classical search (negamax + alpha-beta/PVS + NNUE eval) | Neural MCTS (AlphaZero/Leela-style) |
|---|---|---|
| **What guides the search** | Explicit tree search with pruning heuristics; eval net only scores leaf positions | A policy network proposes moves, a value network scores positions, MCTS balances explore/exploit — search *and* evaluation are both learned |
| **Training requirement** | Supervised: label ~10^5–10^7 positions with a target score (from existing engine evals or game outcomes), train a small net offline | Self-play reinforcement learning: the engine must play millions of games against itself to bootstrap from random play to competence — no shortcut with a small compute budget |
| **Compute for a useful result** | Hours on one RTX 4060 8GB (small net, supervised) | Leela Chess Zero's public training run took thousands of GPU-years cumulative, distributed across volunteers, to reach super-human strength; a solo 4060 self-play run would plateau at weak amateur strength after weeks of continuous training |
| **Interview story** | "I implemented the same search/eval split as Stockfish, understand exactly why each pruning technique is sound, and trained a small evaluation net myself" — concrete, falsifiable, matches how the dominant engine family actually works | "I implemented AlphaZero's algorithm" is a real accomplishment but at solo-4060 scale the honest end state is "it plays legal chess weakly," which is a much harder result to defend as *strong* |
| **Debuggability** | Alpha-beta search is deterministic given fixed settings — a bug produces a reproducible bad move you can step through | MCTS + a weak, undertrained net produces noisy, hard-to-diagnose behavior — is it a search bug or just an undertrained net? Much harder to isolate |

**Decision: classical search (negamax/alpha-beta/PVS) with a trained NNUE-
style evaluation function**, using the GPU specifically for *supervised*
training of that small net — not for self-play RL. This gets genuine,
defensible use of the 4060 (training on real GPU-accelerated backprop,
quantization for fast inference — a real ML engineering skill), while
keeping the search side classical, correct, and fully explainable.

This is not "the safe choice to avoid GPU work" — it's the choice that
uses the GPU for the part of the problem (function approximation from
labeled data) GPUs are actually good at on a compute budget of one card,
rather than the part (RL exploration at scale) that isn't feasible at that
budget. That reasoning — matching the tool to the actual constraint — is
itself the interview-defensible part.

### Search algorithm stack (in build order — see the implementation plan)

- **Negamax with alpha-beta pruning**: the base algorithm. Negamax is
  minimax simplified by exploiting `score(pos) = -score(pos, other side)`,
  so one function handles both sides instead of separate min/max branches.
- **Iterative deepening**: search depth 1, 2, 3, ... reusing the previous
  iteration's best move to order the next iteration's search — dramatically
  improves alpha-beta cutoff rate versus fixed-depth search from a cold
  start, and gives an any-time algorithm (can stop at a time budget and
  still have a legal best move from the last completed depth).
- **Principal Variation Search (PVS)**: after the first move at a node,
  search remaining moves with a zero-width "null window" (fast fail/pass
  check) and only re-search with a full window if one beats expectation —
  cuts search cost when move ordering is already good, which iterative
  deepening + TT move ordering provides.
- **Transposition table (Zobrist hashing)**: positions reached via
  different move orders (transpositions) are extremely common in chess;
  caching `(hash → depth, score, best move, node type)` avoids re-searching
  them and — critically — feeds move ordering (searching the previously-
  found best move first maximizes alpha-beta cutoffs).
- **Move ordering**: TT move first, then MVV-LVA (Most Valuable
  Victim–Least Valuable Attacker) for captures, then killer moves (quiet
  moves that caused a cutoff at the same ply in sibling nodes), then
  history heuristic (moves that have historically caused cutoffs,
  accumulated across the whole search). Move ordering quality is the
  single biggest lever on how much alpha-beta actually prunes — a perfectly
  ordered search prunes to `O(b^(d/2))` instead of `O(b^d)`.
- **Quiescence search**: at the depth-0 leaf, don't just call eval() —
  keep searching captures (and checks, in later phases) until the position
  is "quiet." Without this, the search stops mid-capture-sequence and
  wildly misjudges material that's about to be recaptured (the "horizon
  effect").
- **Null-move pruning**: if giving the opponent a free move still doesn't
  let them catch up, the position is probably winning enough to prune —
  unsound in zugzwang positions (disabled/reduced there), but a large
  practical speedup elsewhere.
- **Late Move Reductions (LMR)**: moves ordered late (i.e., ones the
  ordering heuristics rank as unlikely to be best) are searched to a
  reduced depth first, only extended back to full depth if they turn out
  to beat alpha — trades a small chance of missing a good late move for a
  large average speedup.
- **Aspiration windows**: after the first iterative-deepening pass, search
  the next depth with a narrow `[prev_score - margin, prev_score + margin]`
  window instead of `[-inf, +inf]`, re-searching wider only on fail-high/
  fail-low — narrower windows prune more.

Every one of these is a *heuristic that can be wrong* (that's the point —
they trade a small chance of a search error for large average speedup).
Each is toggleable in the implementation so its actual contribution can be
measured (see §6, benchmarking) rather than assumed.

## 4. Evaluation — handcrafted first, then NNUE

**Phase order matters here and is itself a decision worth defending**: we
build a handcrafted evaluation function *before* the trained NNUE net, even
though the trained net is the end goal. Reasons:

1. **Search needs *a* working eval to be testable at all.** Perft tests
   verify move generation; they say nothing about search or eval. A
   handcrafted eval (material + piece-square tables + mobility + king
   safety) lets every search feature above be built and benchmarked against
   a stable, fast, fully-understood eval — isolating search bugs from eval
   bugs.
2. **It's the fallback/baseline for the benchmarking story.** "Here's the
   Elo with handcrafted eval, here's the Elo with the trained net, here's
   the delta the net bought" is a concrete, quantified result — exactly
   what an interview question about "how do you know your ML component
   helped" wants answered.
3. **NNUE training needs *labeled* data**, and the highest-quality labels
   are `(position, search-based score)` pairs. The handcrafted-eval engine,
   once search is solid, can *generate* that training data itself via
   self-play games, rather than depending entirely on external datasets.

### Handcrafted eval terms (Phase 2 baseline)

- Material (standard piece values, or tuned values — see Texel tuning
  below)
- Piece-square tables (PSTs): positional bonus/penalty per piece type per
  square (e.g., knights penalized on the rim, king penalized in the center
  in the middlegame, rewarded there in the endgame — requires a
  middlegame/endgame PST blend keyed off a simple game-phase estimate from
  remaining material)
- Mobility (legal move count per piece, cheap proxy for piece activity)
- King safety (pawn shield presence, open files near the king)
- Pawn structure (doubled/isolated/passed pawns)

These weights can be hand-guessed initially, then tuned automatically via
**Texel tuning**: treat the weights as parameters, minimize prediction
error against a large set of `(position, game result)` pairs using local
search/gradient descent on the eval function itself — a small-scale,
CPU-only precursor to the NNUE training step, worth doing specifically
because it's a self-contained, well-understood optimization problem to
demonstrate before moving to a full neural net.

### NNUE-style trained eval (Phase 3+)

Same idea as Stockfish's NNUE (see
[BEATING_STOCKFISH.md §1](BEATING_STOCKFISH.md)), scaled to what one 4060
and a solo training run can actually support:

- **Input features**: sparse binary "king square × piece square × piece
  type" per side (HalfKP-style) — much smaller feature set than
  Stockfish's current HalfKAv2_hm to keep the net trainable from scratch on
  modest data volume.
- **Architecture**: a small MLP — sparse input → accumulator layer (the
  layer that gets updated incrementally per move rather than recomputed,
  which is *why* NNUE inference is fast enough for full-depth search) →
  1-2 small dense layers → scalar output. Deliberately far smaller than
  Stockfish's current net; the goal is a trainable-in-hours net that
  clearly improves on the handcrafted eval, not to match Stockfish's own
  net capacity.
- **Training data**: self-play games from the handcrafted-eval engine
  (Phase 2), each position labeled with the search's own score (a standard
  bootstrapping approach — the net learns to approximate what deeper search
  already knows, letting shallower search at inference time punch above
  its depth), optionally supplemented with public labeled datasets
  (e.g., positions from the [Lichess evaluation
  database](https://database.lichess.org/#evals) or the public
  `nnue-pytorch` training sets) if self-play volume is too thin.
- **Training on the RTX 4060 8GB**: PyTorch, standard supervised regression
  (predict the label score, minimize MSE or a scaled loss like Stockfish's
  own `sigmoid`-scaled loss), batched sparse-feature training — this is a
  genuinely GPU-bound step (matrix multiplies over batches of positions)
  and the concrete place the GPU is used, not decoration.
- **Quantization**: convert trained float32 weights to int8/int16 after
  training, matching how Stockfish ships its net — necessary for fast
  integer SIMD inference in the WASM build, and worth explicitly measuring
  the accuracy-vs-speed tradeoff it introduces (small quantization error is
  expected and acceptable; verify it doesn't regress playing strength via
  the benchmarking pipeline in §6).
- **Export**: flat binary weight file, loaded by the Rust engine and
  embedded/fetched at Worker startup, mirroring how Stockfish embeds its
  `.nnue` file.

## 5. WASM compilation and app integration

- **Rust → WASM via `wasm-pack`/`wasm-bindgen`**: produces a `.wasm` module
  plus a JS glue file, loaded from a Web Worker — same deployment shape as
  the existing `public/stockfish/stockfish-18-single.js` Worker.
- **Protocol**: a UCI-lite subset (`position fen ...`, `go depth N` /
  `go movetime N`, `info depth ... score cp ... pv ...`, `bestmove ...`) —
  reusing UCI rather than inventing a custom protocol means the *existing*
  parsing code in `App.tsx`'s Stockfish `useEffect` (regex over `info`
  lines) needs only minor adaptation, and the new engine could even later
  be tested against any other UCI-speaking tool (e.g., `cutechess-cli` for
  benchmarking — see §6).
- **App integration**: per the earlier scoping decision, this ships as a
  *second* Worker option in `App.tsx`, selectable alongside Stockfish, not
  a replacement — `App.tsx` stays the single source of UI state, and the
  new engine is just a different `postMessage` target behind the same
  `info`-line parser. This respects the existing "one file by design"
  convention in [CLAUDE.md](../CLAUDE.md): the engine's *implementation*
  lives entirely outside `App.tsx` (its own Rust crate + WASM artifact),
  and `App.tsx` only gains an engine-picker and a second Worker reference.
- **Single-threaded, initially**: matches the bundled Stockfish build
  (`stockfish-18-single.js`) and avoids the `SharedArrayBuffer` +
  cross-origin-isolation header requirements multithreaded WASM needs
  (Stockfish ships a separate multithreaded build that requires those
  headers). Revisit only if benchmarking shows search speed is the binding
  constraint after all algorithmic gains above are in.

## 6. Benchmarking — how we know any of this actually works

A claim without a number isn't a benchmark. Every phase in the
implementation plan has a corresponding measurable check:

| What | How | Why this method |
|---|---|---|
| **Move generation correctness** | `perft` (count leaf nodes at fixed depth from known positions, compare against published perft results, e.g. from the [Chess Programming Wiki perft results page](https://www.chessprogramming.org/Perft_Results)) | Standard, unambiguous ground truth — a single off-by-one in castling/en-passant/promotion logic shows up immediately as a wrong node count, long before it'd surface as a subtly wrong game |
| **Search speed** | Nodes searched per second (NPS) at fixed depth, measured before/after each search feature (TT, LMR, null-move, etc.) | Isolates each optimization's actual contribution instead of assuming "more heuristics = better" |
| **Pruning effectiveness** | Effective branching factor (`nodes^(1/depth)`) with vs. without a given heuristic | Directly shows how much a pruning technique reduces the tree, independent of raw NPS |
| **Eval quality (handcrafted vs. NNUE)** | Prediction error against a held-out labeled position set (positions never used in training) | Standard ML eval hygiene — measures generalization, not memorization of the training set |
| **Playing strength (Elo)** | Engine-vs-engine matches via [`cutechess-cli`](https://github.com/cutechess/cutechess) (or `python-chess`'s tournament tooling) — this engine vs. itself at different depths, vs. depth-limited Stockfish (e.g., Stockfish capped at low depth/time to create a fair-strength opponent, not full-strength), scored with a standard Elo estimator (e.g., [BayesElo](https://www.remi-coulom.fr/Bayesian-Elo/) or `ordo`) | This is *the* metric that answers "is it actually good," not proxy metrics; testing against depth-capped Stockfish (rather than full-strength) gives a meaningful, non-trivial-loss signal at each stage instead of "lost every game" noise |
| **Tactical test suites** | [STS (Strategic Test Suite)](https://www.chessprogramming.org/Strategic_Test_Suite) and Bratko-Kopec positions — known positions with a known best move, scored by whether/how fast the engine finds it | Fast, deterministic, catches specific weaknesses (e.g., "bad at passed pawns") that aggregate Elo numbers can hide |
| **Regression testing per change** | Re-run the fixed benchmark suite (perft + NPS + a short Elo gauntlet) after every meaningful change, before merging | The same discipline Stockfish's Fishtest enforces at large scale (see [BEATING_STOCKFISH.md §1](BEATING_STOCKFISH.md)) — a small self-hosted version of it, sized to a solo project: not full SPRT infrastructure, but the same "prove it didn't regress" principle |

This benchmarking section is what turns "I built a chess engine" into "I
built a chess engine, and here is the depth-vs-NPS curve, the Elo gain from
each search feature, and the Elo gain the trained eval net bought over the
handcrafted one" — the second is the version that survives interview
follow-up questions.

## 7. Recent advancements in the field (what we adopt, what we skip, and why)

| Advancement | What it is | Adopt / skip here |
|---|---|---|
| **NNUE** (2018–, mainstream in Stockfish since 2020) | Small, fast, incrementally-updated neural net for eval, as described in §4 | **Adopt** — this is the core of the project |
| **Efficiently updatable king-bucketed features (HalfKAv2_hm)** | Stockfish's current, larger input feature set with multiple net "buckets" by king position | **Skip / simplify** — HalfKP-scale is the right size for a from-scratch, solo-training-run net; the larger feature set needs proportionally more training data than a solo project can generate |
| **Multiple NNUE nets by game phase** | Recent Stockfish uses different small nets for material-heavy vs. simplified positions | **Skip for v1**, note as a documented future extension once a single net's training pipeline is proven |
| **Late move reductions / null-move / aspiration windows** | Standard modern pruning, covered in §3 | **Adopt** — these are decades-proven, not exotic |
| **Monte Carlo Tree Search + policy/value nets (AlphaZero/Leela)** | Covered in §3 | **Skip**, with the reasoning in §3 — compute-mismatched to this project's resources |
| **Syzygy endgame tablebases** | Perfect play lookup for ≤7-piece endgames | **Documented future extension** (Tier 1 idea from [BEATING_STOCKFISH.md](BEATING_STOCKFISH.md)) — a probing library integration, not a search/eval change, reasonable to bolt on after the core engine works |
| **Opening books / Polyglot** | Precomputed strong opening move tables | **Documented future extension**, same reasoning |
| **Reinforcement learning fine-tuning on top of supervised NNUE** (e.g., self-play refinement after initial supervised training) | A hybrid: bootstrap with supervised labels (cheap), then refine via limited self-play (expensive) | **Documented future extension**, explicitly flagged as the natural "if I had more compute" next step to raise in an interview — shows awareness of the technique without overclaiming it was done at solo-GPU scale |

## 8. Key decisions, summarized

| Decision | Alternative considered | Why this choice won |
|---|---|---|
| Bitboards + magic bitboards | Array-of-squares (like `chess.js`) | Search hot path needs bitwise-op speed; UI/rules layer (already `chess.js`) doesn't |
| Classical search + trained eval | Neural MCTS / AlphaZero-style | Matches available compute (supervised training, not self-play RL at scale) — see §3 |
| Rust → WASM | C++ → WASM, or pure TypeScript | Memory safety without a GC pause in a latency-sensitive search loop; WASM toolchain (`wasm-pack`) is more turnkey than Emscripten for a from-scratch project; pure TS would be materially slower for the search hot path (no manual memory layout control, no SIMD-friendly integer ops) |
| Handcrafted eval before NNUE | Jump straight to NNUE | Isolates search bugs from eval bugs; generates the engine's own training data via self-play once search works |
| Second Worker option, not a Stockfish replacement | Replace Stockfish integration | Preserves the working baseline for comparison and keeps `App.tsx` monolith convention intact |
| Single-threaded WASM | Multithreaded (SharedArrayBuffer) | Matches deployment simplicity of existing Stockfish build; revisit only if profiling shows it's the binding constraint |
| UCI-lite protocol | Custom protocol | Reuses existing `App.tsx` parsing logic; compatible with standard tooling (`cutechess-cli`) for benchmarking |
