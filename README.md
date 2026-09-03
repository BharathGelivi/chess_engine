# Boardroom Chess Analysis

Boardroom is a local chess analysis UI powered by Stockfish 18 in WebAssembly. It analyzes live positions and Chess.com PGN exports.

## Requirements

- Node.js 20 or newer
- npm
- A modern browser

## Run

```powershell
npm install
npm run dev
```

Open the URL printed by Vite, normally `http://127.0.0.1:5173/`.

```powershell
npm run build    # Production build
npm run lint     # Oxlint
npm run preview  # Preview the production build
```

## Use

1. Click squares to make a legal move. Stockfish analyzes the position automatically.
2. Use the arrows or click a move in the timeline to inspect earlier positions.
3. Click **Import PGN** and select a `.pgn` or `.txt` Chess.com export.
4. Or paste PGN into the lower text area and click **Analyze game**.
5. Click a Stockfish suggestion to play that candidate move.

Chess.com PGN can be exported from a game through **Share > PGN**. Boardroom does not require a Chess.com login and does not upload games.

## Stockfish

The app uses the full single-threaded Stockfish 18 WASM build:

```text
public/stockfish/stockfish-18-single-v2.js
public/stockfish/stockfish-18-single-v2.wasm
```

The WASM binary is about 108 MB. It is not loaded by default — the app starts with the small (~170KB) from-scratch "Overboard Engine" and only fetches Stockfish if you switch to it in the engine picker. Once fetched, the browser caches it for later visits. Stockfish runs in a Web Worker so it does not block the UI. The current search limit is depth 18; change `go depth 18` in `src/App.tsx` to tune response time and strength.

The single-threaded build avoids requiring cross-origin isolation headers. A multi-threaded build can be used later with COOP/COEP-enabled hosting.

## Architecture

- `src/App.tsx`: board state, PGN parsing, UI, and UCI parsing
- `chess.js`: legal moves and FEN/PGN state
- `public/stockfish/`: Stockfish browser assets
- UCI commands connect the UI to Stockfish: `uci`, `position fen ...`, and `go depth ...`

## Licensing

Stockfish is GPLv3 software. The bundled engine retains its upstream license and attribution. See `node_modules/stockfish/Copying.txt` and the [Stockfish.js project](https://github.com/nmrugg/stockfish.js). Review GPLv3 obligations before publishing a hosted or modified distribution.
