# Beating Stockfish: how it works, and what it'd actually take

This project bundles **Stockfish 18 (single-threaded WASM)** in
[`public/stockfish/`](../public/stockfish/). This doc explains how that engine
works internally, where its real weaknesses are, and what a serious attempt at
building something stronger looks like.

Blunt framing first: outright out-searching current Stockfish from a cold
start is not a weekend project — it's the product of ~10 years and millions
of self-play games from a global open-source effort. But you don't need to
beat it in general to beat *this instance* of it, and understanding its
architecture is what tells you which is realistic.

## 1. How Stockfish actually works

Two halves: a **search** that walks the game tree, and an **evaluation**
(NNUE) that scores a position when the search stops.

### Search

- **Iterative deepening**: searches depth 1, then 2, then 3... reusing
  results to order moves better each pass, stopping when the time budget or
  target depth runs out.
- **Alpha-beta / PVS (Principal Variation Search)**: prunes branches that
  can't beat the best line already found. Move ordering quality determines
  how much gets pruned — killer moves, history heuristic, and the
  transposition table's stored best move all feed this.
- **Transposition table**: a hash table keyed by Zobrist hash, caching
  score/depth/best-move for positions already searched, since the same
  position is reached via different move orders constantly.
- **Selective pruning/extensions**: null-move pruning, late move reductions
  (LMR), futility pruning, check extensions — heuristics that cut or extend
  branches based on "this is probably not worth searching deeply." These are
  where most of Stockfish's search strength (and its blind spots) live.
- **Aspiration windows**: re-searches around the previous iteration's score
  with a narrow alpha-beta window for speed, widening on fail-high/fail-low.
- **Quiescence search**: at leaf nodes, keeps searching captures/checks only,
  so it doesn't misjudge a position mid-capture-sequence.

### Evaluation — NNUE

Since Stockfish 12 (and exclusively since ~16), position evaluation is a
small neural net, not hand-written heuristics:

- **HalfKAv2/HalfKP-style input features**: sparse binary features encoding
  "king position × piece position × piece type," one perspective per side.
- **Incremental accumulator**: because most moves change few features, the
  first layer's activations are updated incrementally per move instead of
  recomputed — this is *the* reason NNUE is fast enough to run inside a
  full-depth alpha-beta search.
- **Quantized int8/int16 weights**: trained in float, then quantized for
  integer SIMD inference speed. This introduces small, structured precision
  loss.
- **Trained on self-play data**: labeled with search results from prior
  Stockfish versions (a bootstrapping loop), not human games.

### Everything ships through SPRT testing

Every change to Stockfish — search tweak or net retrain — is validated on
[Fishtest](https://tests.stockfishchess.org/tests), a distributed
infrastructure running an **SPRT** (Sequential Probability Ratio Test):
tens of thousands of games against the previous version, accepted only if
it's statistically stronger. This is why "one clever idea" rarely moves the
needle — the low-hanging fruit has been tested and merged or rejected over
thousands of iterations already.

## 2. Where the real weaknesses are

Not "it's bad at chess" — it's superhuman. The exploitable gaps are narrower:

| Weakness                                                        | Why it exists                                                                                                                       | How it's exploitable                                                                                                                                                     |
| --------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Fixed time/depth budget**                               | Your`go depth 18` call (see [App.tsx](../src/App.tsx)) caps search; under blitz/bullet time controls the engine trims aggressively | Positions requiring deep tactical sequences beyond the depth budget get misjudged — this is the entire basis of "engine traps"                                          |
| **NNUE training distribution**                            | Net is trained on positions from self-play games at normal strength                                                                 | Highly unusual/artificial positions (fortress structures, some closed positions, certain material imbalances) are outside its training distribution and get misevaluated |
| **No explicit long-horizon planning**                     | Search depth is finite; NNUE has no symbolic "plan" representation                                                                  | Maneuvering, multi-move-non-forcing plans (e.g., slow king marches, zugzwang setups) can be underweighted relative to tactics                                            |
| **Deterministic given fixed settings**                    | Same FEN + same Hash/Threads/depth → same move, always                                                                             | Prep: if you know it's running with fixed settings, you can prepare a specific refutation line offline and repeat it                                                     |
| **MultiPV / eval scale is a heuristic, not ground truth** | The`+0.34` centipawn score is a learned proxy, not a solved value                                                                 | Near-equal evaluations can hide practical winning chances a pure eval score won't reflect                                                                                |
| **Endgame tablebase gap**                                 | Without Syzygy tablebases wired in, exact endgames beyond ~7 pieces are searched, not looked up                                     | A tablebase-backed opponent (even a weak overall engine) can outplay a tablebase-less Stockfish in exact endgames                                                        |

None of these mean "Stockfish is beatable in a fair full-strength game" —
they mean the exploitable surface is *constraints and configuration*, not
raw chess understanding.

## 3. What "build your own Stockfish" actually looks like

Three tiers, cheapest to hardest:

### Tier 1 — Beat *this deployment's* constraints, not the algorithm

The realistic near-term project, given what's already in this repo:

- Increase search depth/time (`engine.postMessage('go depth 18')` in
  [App.tsx:62](../src/App.tsx)) — a shallower opponent config is a
  legitimately different (weaker) opponent.
- Bolt on a Syzygy tablebase probe for endgames.
- Build an opening book targeting known engine-vs-engine drawish lines to
  steer toward positions statistically harder for the NNUE (closed,
  fortress-like structures).
  This is "beat your Stockfish instance," achieved through configuration and
  prep, not by writing a stronger search/eval.

### Tier 2 — Contribute to Stockfish itself

Actually the fastest way to end up with code that plays better than current
Stockfish, because you inherit the existing ~3500-Elo baseline instead of
starting at zero:

1. Clone [github.com/official-stockfish/Stockfish](https://github.com/official-stockfish/Stockfish),
   read `src/search.cpp` and `src/evaluate.cpp` end to end.
2. Read the [Chess Programming Wiki](https://www.chessprogramming.org/Main_Page)
   for the vocabulary (LMR, null-move, aspiration windows, Zobrist hashing).
3. Try a search-side idea (pruning condition, extension trigger, move
   ordering tweak), build it, and submit it through Fishtest's SPRT pipeline.
4. For NNUE work specifically, use
   [nnue-pytorch](https://github.com/official-stockfish/nnue-pytorch) — the
   actual training pipeline Stockfish uses — and experiment with
   architecture/feature-set changes, retraining on the public data sets
   linked from that repo.

### Tier 3 — Independent engine, from scratch

The AlphaZero/Leela Chess Zero path: MCTS + a much larger value/policy net,
trained via self-play reinforcement learning rather than supervised
labels from a prior search. This beats Stockfish on raw playing strength
in some conditions (and loses in others) but costs GPU-cluster-scale
training compute — not comparable to Tier 1/2 effort.
[github.com/LeelaChessZero/lc0](https://github.com/LeelaChessZero/lc0) is
the reference implementation if you want to study that approach instead.

## 4. Recommended path

Given this repo already has a working UCI bridge to Stockfish
([App.tsx](../src/App.tsx)), Tier 1 is the natural next step and reuses
everything here: it's a config/prep project, not a search-and-eval rewrite.
Tier 2 is the honest answer to "build my own Stockfish that beats current
Stockfish" — real chance of success, but measured in months of focused work
plus Fishtest compute, not a single build session.
