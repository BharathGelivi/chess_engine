import { useEffect, useMemo, useRef, useState } from 'react'
import { Activity, ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight, Download, Gauge, Play, RotateCcw, Square, Upload, Zap } from 'lucide-react'
import { Chess, Move } from 'chess.js'
import './App.css'

// Solid glyph set for both colors — the outline "white" chess glyphs render hollow in most fonts.
const pieceGlyphs: Record<string, string> = {
  wK: '♚', wQ: '♛', wR: '♜', wB: '♝', wN: '♞', wP: '♟',
  bK: '♚', bQ: '♛', bR: '♜', bB: '♝', bN: '♞', bP: '♟',
}
const files = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h']
const PIECE_VALUES: Record<string, number> = { p: 1, n: 3, b: 3, r: 5, q: 9, k: 0 }
// Positive = White up material, negative = Black up.
function materialDiff(g: Chess): number {
  let diff = 0
  for (const row of g.board()) for (const sq of row) if (sq) diff += (sq.color === 'w' ? 1 : -1) * PIECE_VALUES[sq.type]
  return diff
}
const demoPgn = `[Event "Rapid review"]\n[White "You"]\n[Black "Training partner"]\n[Result "*"]\n\n1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 *`

// Two engines can back the "live analysis" Worker: the bundled Stockfish 18
// build, and the from-scratch engine in engine/ (compiled to WASM, see
// public/engine/). Both speak the same UCI-lite line shape, so this picker
// just swaps which script the Worker loads.
type EngineKind = 'stockfish' | 'ours'
const engineNames: Record<EngineKind, string> = { stockfish: 'Stockfish 18', ours: 'Overboard Engine' }
// Our engine's copy-make search is far slower (nodes/sec) than Stockfish's
// hand-tuned C++. Stockfish gets a fixed depth (it reaches 18 in well under
// a second). Ours gets a time budget instead of a depth cap — fixed depth 7
// was leaving real playing strength on the table (shallow enough to miss
// tactics bots above ~1200 punish); movetime lets iterative deepening go as
// far as the budget allows, and the `stop` sent on every position change
// (below) cancels a stale search rather than letting it queue up.
const analysisBudget: Record<EngineKind, string> = { stockfish: 'depth 18', ours: 'movetime 3000' }
// Fixed per-move search budget for engine-vs-engine autoplay — movetime (not
// depth) keeps a full game to a playable pace regardless of which engine or
// position complexity is in play.
const AUTOPLAY_MOVETIME_MS = 500
const AUTOPLAY_MAX_PLIES = 200

function makeEngineWorker(kind: EngineKind): Worker {
  return kind === 'stockfish'
    ? new Worker('/stockfish/stockfish-18-single.js')
    : new Worker('/engine/worker.js', { type: 'module' })
}

/** Spawns a worker and resolves once it answers `isready` with `readyok`. */
function spawnReadyEngine(kind: EngineKind): Promise<Worker> {
  return new Promise((resolve) => {
    const worker = makeEngineWorker(kind)
    const onMessage = (event: MessageEvent) => {
      if (event.data === 'readyok') {
        worker.removeEventListener('message', onMessage)
        resolve(worker)
      }
    }
    worker.addEventListener('message', onMessage)
    worker.postMessage('uci')
    worker.postMessage('isready')
  })
}

/** Asks a ready engine worker for its best move from `fen`, UCI square notation (e.g. "e2e4"). */
function requestBestMove(worker: Worker, fen: string, movetimeMs: number): Promise<string | null> {
  return new Promise((resolve) => {
    const onMessage = (event: MessageEvent) => {
      const line = typeof event.data === 'string' ? event.data : ''
      const match = line.match(/^bestmove (\S+)/)
      if (match) {
        worker.removeEventListener('message', onMessage)
        resolve(match[1] === '0000' ? null : match[1])
      }
    }
    worker.addEventListener('message', onMessage)
    worker.postMessage(`position fen ${fen}`)
    worker.postMessage(`go movetime ${movetimeMs}`)
  })
}

function gameResultLabel(game: Chess, white: EngineKind, black: EngineKind, cappedOut: boolean, stopped: boolean): string {
  if (game.isCheckmate()) {
    const winner = game.turn() === 'w' ? 'Black' : 'White'
    const winnerEngine = game.turn() === 'w' ? black : white
    return `Checkmate — ${winner} wins (${engineNames[winnerEngine]})`
  }
  if (game.isStalemate()) return 'Draw — stalemate'
  if (game.isThreefoldRepetition()) return 'Draw — threefold repetition'
  if (game.isInsufficientMaterial()) return 'Draw — insufficient material'
  if (game.isDraw()) return 'Draw — 50-move rule'
  if (stopped) return 'Stopped'
  if (cappedOut) return `Move-count cap reached (${AUTOPLAY_MAX_PLIES} plies)`
  return 'Game in progress'
}

function App() {
  const [, setGame] = useState(() => new Chess())
  const [history, setHistory] = useState<Move[]>([])
  const [viewIndex, setViewIndex] = useState(0)
  const [pgnText, setPgnText] = useState('')
  const [status, setStatus] = useState('Ready for a game')
  const [engineState, setEngineState] = useState({ depth: 0, time: 0, evaluation: '0.00', lines: [] as Array<{ move: string; san: string; score: string; label: string; multipv: number }> })
  // Stockfish's WASM binary is ~110MB — a cold load can take a minute or
  // more before the worker responds at all, during which the UI would
  // otherwise look identically "calculating" whether it's thinking or just
  // still downloading. Tracked separately so we can tell the user which.
  const [engineReady, setEngineReady] = useState(false)
  const [mode, setMode] = useState<'analysis' | 'autoplay'>('analysis')
  const [engineChoice, setEngineChoice] = useState<EngineKind>('stockfish')
  const [autoplay, setAutoplay] = useState({ running: false, loading: false, white: 'stockfish' as EngineKind, black: 'ours' as EngineKind, result: '', plies: 0 })
  const fileRef = useRef<HTMLInputElement>(null)
  const engineRef = useRef<Worker | null>(null)
  const currentFenRef = useRef('')
  const autoplayStopRef = useRef(false)
  const autoplayWorkersRef = useRef<{ white?: Worker; black?: Worker }>({})

  const viewGame = useMemo(() => {
    const replay = new Chess()
    history.slice(0, viewIndex).forEach((move) => replay.move(move))
    return replay
  }, [history, viewIndex])
  currentFenRef.current = viewGame.fen()
  const material = useMemo(() => materialDiff(viewGame), [viewGame])

  const suggestions = engineState.lines

  useEffect(() => {
    setEngineReady(false)
    const engine = makeEngineWorker(engineChoice)
    engineRef.current = engine
    engine.postMessage('uci')
    if (engineChoice === 'stockfish') engine.postMessage('setoption name MultiPV value 3')
    engine.postMessage('isready')
    engine.onmessage = (event) => {
      const line = typeof event.data === 'string' ? event.data : ''
      if (line === 'readyok') setEngineReady(true)
      const depth = line.match(/\bdepth (\d+)/)?.[1]
      const time = line.match(/\btime (\d+)/)?.[1]
      const score = line.match(/score (cp|mate) (-?\d+)/)
      const pv = line.match(/\bpv (.+)$/)?.[1]
      const multipv = Number(line.match(/\bmultipv (\d+)/)?.[1] ?? 1)
      if (depth && score && pv) {
        const moves = pv.split(' ')
        // The engine's info line is async and can arrive after the live
        // position has already moved on (esp. during autoplay, where the
        // position changes every ~1s); chess.js throws rather than
        // returning null for a move that no longer applies, so this is
        // speculative and falls back to the raw UCI move string below.
        let first = null
        try {
          first = new Chess(currentFenRef.current).move({ from: moves[0].slice(0, 2), to: moves[0].slice(2, 4), promotion: moves[0][4] as any })
        } catch { /* stale PV for a position we've since moved past */ }
        const centipawns = score[1] === 'mate' ? `M${score[2]}` : `${Number(score[2]) >= 0 ? '+' : ''}${(Number(score[2]) / 100).toFixed(2)}`
        setEngineState((current) => {
          // MultiPV lines arrive independently per PV slot and out of order across
          // depths — `multipv N` is the engine's own rank (1 = best), so key/sort
          // by that instead of arrival order, or the panel can show a worse-looking
          // move above a better one just because its info line landed first.
          const lines = [...current.lines.filter((item) => item.multipv !== multipv), { move: `${moves[0].slice(0, 2)}-${moves[0].slice(2, 4)}`, san: first?.san ?? moves[0], score: centipawns, label: '', multipv }]
            .sort((a, b) => a.multipv - b.multipv)
            .slice(0, 3)
            .map((item, index) => ({ ...item, label: index === 0 ? 'Best move' : 'Alternative' }))
          return { ...current, depth: Number(depth), time: Number(time ?? current.time), evaluation: centipawns, lines }
        })
      }
    }
    return () => { engine.postMessage('quit'); engine.terminate(); engineRef.current = null }
  }, [engineChoice])

  useEffect(() => {
    const engine = engineRef.current
    if (!engine) return
    setEngineState((current) => ({ ...current, depth: 0, time: 0, lines: [] }))
    engine.postMessage('stop')
    engine.postMessage(`position fen ${viewGame.fen()}`)
    engine.postMessage(`go ${analysisBudget[engineChoice]}`)
  }, [viewGame, engineChoice])

  // Engine-vs-engine autoplay: drives two independent engine Workers in
  // turn, appending each ply to the same `history`/`viewIndex` state the
  // manual board uses, so the existing board rendering just follows along.
  async function runAutoplayLoop(white: Worker, black: Worker, startingGame: Chess) {
    const liveGame = startingGame
    let plies = 0
    while (!autoplayStopRef.current && plies < AUTOPLAY_MAX_PLIES && !liveGame.isGameOver()) {
      const mover = liveGame.turn() === 'w' ? white : black
      const bestMove = await requestBestMove(mover, liveGame.fen(), AUTOPLAY_MOVETIME_MS)
      if (autoplayStopRef.current || !bestMove) break
      let move
      try {
        move = liveGame.move({ from: bestMove.slice(0, 2), to: bestMove.slice(2, 4), promotion: (bestMove[4] as any) ?? 'q' })
      } catch {
        break // engine returned a move that's no longer legal (shouldn't happen; defensive)
      }
      if (!move) break
      plies += 1
      setHistory((h) => [...h, move])
      setViewIndex((v) => v + 1)
      setAutoplay((a) => ({ ...a, plies }))
    }
    setAutoplay((a) => ({
      ...a,
      running: false,
      result: gameResultLabel(liveGame, a.white, a.black, plies >= AUTOPLAY_MAX_PLIES, autoplayStopRef.current),
    }))
    white.postMessage('quit'); white.terminate()
    black.postMessage('quit'); black.terminate()
    autoplayWorkersRef.current = {}
  }

  async function startAutoplay() {
    if (autoplay.running) return
    autoplayStopRef.current = false
    const fresh = new Chess()
    setGame(fresh); setHistory([]); setViewIndex(0); setStatus('Engine vs engine in progress')
    setAutoplay((a) => ({ ...a, running: true, loading: true, result: '', plies: 0 }))
    const [white, black] = await Promise.all([spawnReadyEngine(autoplay.white), spawnReadyEngine(autoplay.black)])
    if (autoplayStopRef.current) { white.terminate(); black.terminate(); return }
    autoplayWorkersRef.current = { white, black }
    setAutoplay((a) => ({ ...a, loading: false }))
    runAutoplayLoop(white, black, fresh)
  }

  function stopAutoplay() {
    autoplayStopRef.current = true
    setAutoplay((a) => ({ ...a, running: false, loading: false }))
  }

  function resetAutoplay() {
    autoplayStopRef.current = true
    const workers = autoplayWorkersRef.current
    workers.white?.terminate(); workers.black?.terminate()
    autoplayWorkersRef.current = {}
    setGame(new Chess()); setHistory([]); setViewIndex(0); setStatus('Ready for a game')
    setAutoplay((a) => ({ ...a, running: false, loading: false, result: '', plies: 0 }))
  }

  function loadPgn(text: string) {
    const next = new Chess()
    try {
      next.loadPgn(text)
      const moves = next.history({ verbose: true }) as Move[]
      setGame(next)
      setHistory(moves)
      setViewIndex(moves.length)
      setStatus(`Imported ${moves.length} half-moves`)
      setPgnText(text)
    } catch {
      setStatus('Could not read that PGN. Check the export format.')
    }
  }

  function makeMove(from: string, to: string) {
    if (mode === 'autoplay') return
    const next = new Chess(viewGame.fen())
    try {
      const move = next.move({ from, to, promotion: 'q' })
      if (!move) return
      const branched = viewIndex !== history.length
      const branchHistory = [...history.slice(0, viewIndex), move]
      setGame(next)
      setHistory(branchHistory)
      setViewIndex(branchHistory.length)
      setStatus(branched ? `What-if line from move ${viewIndex}: ${next.isCheck() ? 'Check' : 'exploring alternate line'}` : next.isCheck() ? 'Check' : 'Position updated')
    } catch { /* Illegal square selections are simply ignored. */ }
  }

  function handleSquareClick(square: string) {
    const selected = document.querySelector(`[data-selected="true"]`) as HTMLElement | null
    if (selected) {
      makeMove(selected.dataset.square!, square)
      selected.dataset.selected = 'false'
      return
    }
    const piece = viewGame.get(square as any)
    if (piece?.color === viewGame.turn()) {
      document.querySelectorAll('[data-selected="true"]').forEach((el) => (el as HTMLElement).dataset.selected = 'false')
      const element = document.querySelector(`[data-square="${square}"]`) as HTMLElement | null
      if (element) element.dataset.selected = 'true'
    }
  }

  function reset() {
    setGame(new Chess()); setHistory([]); setViewIndex(0); setStatus('Ready for a game'); setPgnText('')
  }

  const board = Array.from({ length: 8 }, (_, row) => files.map((file) => `${file}${8 - row}`)).flat()

  return (
    <main className="app-shell">
      <header className="topbar"><svg className="brand-mark" width="34" height="34" viewBox="0 0 34 34"><defs><linearGradient id="logoGrad" x1="0" y1="0" x2="34" y2="34"><stop offset="0%" stopColor="var(--accent-pink)" /><stop offset="100%" stopColor="var(--accent-orange)" /></linearGradient></defs><rect x="1" y="1" width="32" height="32" rx="9" fill="none" stroke="url(#logoGrad)" strokeWidth="2" /><path d="M11 23 L17 10 L23 23" fill="none" stroke="url(#logoGrad)" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" /><circle cx="17" cy="23" r="1.8" fill="var(--accent-orange)" /></svg><div><strong>OVERBOARD</strong><span>Personal chess intelligence</span></div><div className="top-actions"><button className="ghost" onClick={reset}><RotateCcw size={16} /> New game</button><button className="primary" onClick={() => fileRef.current?.click()}><Upload size={16} /> Import PGN</button><input ref={fileRef} hidden type="file" accept=".pgn,.txt" onChange={(event) => { const file = event.target.files?.[0]; if (file) file.text().then(loadPgn) }} /></div></header>
      <div className="mode-tabs"><button className={mode === 'analysis' ? 'active-move' : ''} onClick={() => setMode('analysis')}><Gauge size={14} /> Analysis</button><button className={mode === 'autoplay' ? 'active-move' : ''} onClick={() => setMode('autoplay')}><Play size={14} /> Engine vs Engine</button></div>
      <div className="workspace">
        <section className="board-column"><div className="eyebrow"><span className="live-dot" /> {mode === 'autoplay' ? 'AUTOPLAY' : 'LIVE ANALYSIS'} <span className="muted">{status}</span></div><div className="board-frame"><div className="board">{board.map((square, index) => { const piece = viewGame.get(square as any); return <button key={square} className={`square ${(Math.floor(index / 8) + index) % 2 ? 'dark' : 'light'}`} data-square={square} data-selected="false" onClick={() => handleSquareClick(square)} aria-label={square}>{piece && <span className={`piece ${piece.color === 'w' ? 'white-piece' : 'black-piece'}`}>{pieceGlyphs[`${piece.color}${piece.type.toUpperCase()}`]}</span>}{index % 8 === 0 && <small>{square[1]}</small>}{index >= 56 && <i>{square[0]}</i>}</button> })}</div></div><div className="board-footer"><div className="nav-buttons"><button title="First move" disabled={viewIndex === 0} onClick={() => setViewIndex(0)}><ChevronsLeft size={20} /></button><button title="Previous move" disabled={viewIndex === 0} onClick={() => setViewIndex(Math.max(0, viewIndex - 1))}><ChevronLeft size={20} /></button></div><span>{viewIndex ? `${Math.ceil(viewIndex / 2)}${viewIndex % 2 ? '...' : '.'}` : 'Start'} <b>{viewGame.turn() === 'w' ? 'White to move' : 'Black to move'}</b>{material !== 0 && <span className={`material-badge ${material > 0 ? 'white-up' : 'black-up'}`}>{material > 0 ? `White +${material}` : `Black +${-material}`}</span>}</span><div className="nav-buttons"><button title="Next move" disabled={viewIndex === history.length} onClick={() => setViewIndex(Math.min(history.length, viewIndex + 1))}><ChevronRight size={20} /></button><button title="Last move" disabled={viewIndex === history.length} onClick={() => setViewIndex(history.length)}><ChevronsRight size={20} /></button></div></div></section>
        <aside className="analysis-panel">
          {mode === 'analysis' ? (
            <>
              <div className="panel-heading"><div><span className="label">POSITION</span><h1>{viewGame.isCheckmate() ? 'Checkmate' : viewGame.isCheck() ? 'King in check' : 'Quiet position'}</h1></div><div className="eval">{engineState.evaluation}</div></div>
              <div className="meter"><span /></div>
              <div className="engine-picker"><span className="label">ENGINE</span><div className="engine-picker-buttons"><button className={engineChoice === 'stockfish' ? 'active-move' : ''} onClick={() => setEngineChoice('stockfish')}>Stockfish 18</button><button className={engineChoice === 'ours' ? 'active-move' : ''} onClick={() => setEngineChoice('ours')}>Overboard Engine</button></div></div>
              <div className="engine-status"><Zap size={15} /> {engineNames[engineChoice]} · depth {engineState.depth || '...'} <span>{(engineState.time / 1000).toFixed(1)}s</span></div>
              <div className="section-title"><span>TOP LINES</span><span className="tiny-badge">{engineChoice === 'stockfish' ? '3 suggestions' : '1 suggestion'}</span></div>
              <div className="lines">{suggestions.length ? suggestions.map((item) => <button className="line" key={item.move} onClick={() => makeMove(item.move.slice(0, 2), item.move.slice(3))}><span className="line-rank">{item.label}</span><strong>{item.san}</strong><span>{item.score}</span></button>) : <p className="empty">{engineReady ? `${engineNames[engineChoice]} is calculating...` : `Downloading ${engineNames[engineChoice]}... this can take a minute on first load.`}</p>}</div>
              <div className="section-title"><span>GAME MOVES</span><span className="muted">{history.length} ply</span></div>
              <div className="move-list">{history.length ? history.map((move, index) => <button key={`${move.san}-${index}`} className={index === viewIndex - 1 ? 'active-move' : ''} onClick={() => setViewIndex(index + 1)}><small>{index % 2 === 0 ? `${Math.floor(index / 2) + 1}.` : ''}</small>{move.san}</button>) : <p className="empty">Import a Chess.com PGN to review your game.</p>}</div>
              <div className="panel-footer"><button className="ghost" onClick={() => loadPgn(demoPgn)}><Download size={15} /> Load sample game</button><span><Gauge size={15} /> Full WASM engine</span></div>
            </>
          ) : (
            <>
              <div className="panel-heading"><div><span className="label">ENGINE VS ENGINE</span><h1>{autoplay.result || (autoplay.loading ? 'Loading engines...' : autoplay.running ? 'Playing...' : 'Ready')}</h1>{autoplay.loading && <p className="empty">Downloading {engineNames[autoplay.white]} and {engineNames[autoplay.black]}... first load can take a couple of minutes.</p>}</div></div>
              <div className="autoplay-sides">
                <div className="autoplay-side"><span className="label">WHITE</span><div className="engine-picker-buttons"><button disabled={autoplay.running} className={autoplay.white === 'stockfish' ? 'active-move' : ''} onClick={() => setAutoplay((a) => ({ ...a, white: 'stockfish' }))}>Stockfish 18</button><button disabled={autoplay.running} className={autoplay.white === 'ours' ? 'active-move' : ''} onClick={() => setAutoplay((a) => ({ ...a, white: 'ours' }))}>Overboard</button></div></div>
                <div className="autoplay-side"><span className="label">BLACK</span><div className="engine-picker-buttons"><button disabled={autoplay.running} className={autoplay.black === 'stockfish' ? 'active-move' : ''} onClick={() => setAutoplay((a) => ({ ...a, black: 'stockfish' }))}>Stockfish 18</button><button disabled={autoplay.running} className={autoplay.black === 'ours' ? 'active-move' : ''} onClick={() => setAutoplay((a) => ({ ...a, black: 'ours' }))}>Overboard</button></div></div>
              </div>
              <div className="autoplay-controls">
                <button className="primary" disabled={autoplay.running} onClick={startAutoplay}><Play size={15} /> Start</button>
                <button className="ghost" disabled={!autoplay.running} onClick={stopAutoplay}><Square size={15} /> Stop</button>
                <button className="ghost" onClick={resetAutoplay}><RotateCcw size={15} /> Reset</button>
              </div>
              <div className="engine-status"><Activity size={15} /> {autoplay.plies} plies played <span>{AUTOPLAY_MOVETIME_MS}ms/move budget</span></div>
              <div className="section-title"><span>GAME MOVES</span><span className="muted">{history.length} ply</span></div>
              <div className="move-list">{history.length ? history.map((move, index) => <button key={`${move.san}-${index}`} className={index === viewIndex - 1 ? 'active-move' : ''} onClick={() => setViewIndex(index + 1)}><small>{index % 2 === 0 ? `${Math.floor(index / 2) + 1}.` : ''}</small>{move.san}</button>) : <p className="empty">Press Start to watch two engines play.</p>}</div>
            </>
          )}
        </aside>
      </div>
      <section className="import-strip"><div><span className="label">YOUR GAMES</span><h2>Turn your archive into a training loop.</h2><p>Import Chess.com PGN exports to review blunders, compare candidate moves, and build a personal opening profile.</p></div><textarea value={pgnText} onChange={(event) => setPgnText(event.target.value)} placeholder="Paste a PGN export here..." /><button className="primary" onClick={() => loadPgn(pgnText)} disabled={!pgnText}><Activity size={16} /> Analyze game</button></section>
    </main>
  )
}

export default App
