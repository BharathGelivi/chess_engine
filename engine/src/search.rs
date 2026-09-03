// negamax + alpha-beta search: iterative deepening, Zobrist-keyed
// transposition table, move ordering (TT move / MVV-LVA / killers /
// history), quiescence search, PVS, null-move pruning, LMR, aspiration
// windows. Every technique after plain alpha-beta is gated by a bool in
// `SearchOptions` so its contribution can be benchmarked in isolation
// (see engine/BENCHMARKS.md).

use crate::board::{Move, Position, KING, PAWN};
use crate::eval::{evaluate, MATE_SCORE};
use crate::movegen::{gen_legal, in_check, make_move};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const MAX_PLY: usize = 64;

#[cfg(target_arch = "wasm32")]
pub fn now_ms() -> f64 { js_sys::Date::now() }
#[cfg(not(target_arch = "wasm32"))]
pub fn now_ms() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64() * 1000.0
}

#[derive(Clone, Copy)]
pub struct SearchOptions {
    pub use_tt: bool,
    pub use_ordering: bool,
    pub use_quiescence: bool,
    pub use_pvs: bool,
    pub use_null_move: bool,
    pub use_lmr: bool,
    pub use_aspiration: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions { use_tt: true, use_ordering: true, use_quiescence: true, use_pvs: true, use_null_move: true, use_lmr: true, use_aspiration: true }
    }
}

impl SearchOptions {
    /// Phase-3-only baseline (no Phase-4 selective-search additions) —
    /// used by the benchmark harness to isolate Phase 3 vs Phase 4 gains.
    pub fn phase3_only() -> Self {
        SearchOptions { use_tt: true, use_ordering: true, use_quiescence: true, use_pvs: true, use_null_move: false, use_lmr: false, use_aspiration: false }
    }
    /// Plain alpha-beta + iterative deepening only, nothing else.
    pub fn minimal() -> Self {
        SearchOptions { use_tt: false, use_ordering: false, use_quiescence: false, use_pvs: false, use_null_move: false, use_lmr: false, use_aspiration: false }
    }
}

const TT_FLAG_EXACT: u8 = 0;
const TT_FLAG_LOWER: u8 = 1;
const TT_FLAG_UPPER: u8 = 2;

#[derive(Clone, Copy)]
struct TTEntry {
    hash: u64,
    depth: i8,
    score: i32,
    flag: u8,
    best: Option<Move>,
}

pub struct Search {
    // Shared across every helper-thread Search for a Lazy-SMP search: all
    // threads probe/store into the same table (behind a per-slot mutex, so
    // one thread's work speeds up the others via transposition hits) —
    // killers/history stay per-thread since they're move-ordering heuristics
    // tied to that thread's own search path, not shared position data.
    tt: Arc<Vec<Mutex<Option<TTEntry>>>>,
    tt_mask: usize,
    killers: Vec<[Option<Move>; 2]>,
    history: [[[i32; 64]; 64]; 2],
    pub nodes: u64,
    stop: bool,
    // Fresh per `go` call, shared with that call's helper threads: lets any
    // thread hitting the deadline signal the others to stop promptly.
    time_up: Arc<AtomicBool>,
    deadline_ms: Option<f64>,
    start_ms: f64,
    opts: SearchOptions,
    pub last_pv: Vec<Move>,
}

pub struct SearchLimits {
    pub depth: Option<u32>,
    pub movetime_ms: Option<u64>,
}

pub struct DepthInfo {
    pub depth: u32,
    pub score_cp: i32,
    pub mate: Option<i32>,
    pub nodes: u64,
    pub time_ms: u64,
    pub pv: Vec<Move>,
}

impl Search {
    pub fn new(opts: SearchOptions) -> Self {
        let tt_size = 1 << 20; // ~1M entries, power of two for masking
        Search {
            tt: Arc::new((0..tt_size).map(|_| Mutex::new(None)).collect()),
            tt_mask: tt_size - 1,
            killers: vec![[None, None]; MAX_PLY + 1],
            history: [[[0; 64]; 64]; 2],
            nodes: 0,
            stop: false,
            time_up: Arc::new(AtomicBool::new(false)),
            deadline_ms: None,
            start_ms: 0.0,
            opts,
            last_pv: Vec::new(),
        }
    }

    /// A helper-thread Search for Lazy-SMP: shares this instance's
    /// transposition table and time-up flag, but gets its own killers/
    /// history and starts from a clean node count.
    fn spawn_helper(&self) -> Search {
        Search {
            tt: self.tt.clone(),
            tt_mask: self.tt_mask,
            killers: vec![[None, None]; MAX_PLY + 1],
            history: [[[0; 64]; 64]; 2],
            nodes: 0,
            stop: false,
            time_up: self.time_up.clone(),
            deadline_ms: self.deadline_ms,
            start_ms: self.start_ms,
            opts: self.opts,
            last_pv: Vec::new(),
        }
    }

    pub fn set_options(&mut self, opts: SearchOptions) { self.opts = opts; }

    fn tt_probe(&self, hash: u64) -> Option<TTEntry> {
        if !self.opts.use_tt { return None; }
        let idx = (hash as usize) & self.tt_mask;
        let guard = self.tt[idx].lock().unwrap();
        guard.filter(|e| e.hash == hash)
    }

    fn tt_store(&self, hash: u64, depth: i8, score: i32, flag: u8, best: Option<Move>) {
        if !self.opts.use_tt { return; }
        let idx = (hash as usize) & self.tt_mask;
        let mut guard = self.tt[idx].lock().unwrap();
        let replace = match &*guard {
            None => true,
            Some(old) => old.depth <= depth || old.hash == hash,
        };
        if replace {
            *guard = Some(TTEntry { hash, depth, score, flag, best });
        }
    }

    fn check_time(&mut self) {
        if self.nodes % 4096 != 0 { return; }
        if self.time_up.load(Ordering::Relaxed) { self.stop = true; return; }
        if let Some(dl) = self.deadline_ms {
            if now_ms() >= dl {
                self.stop = true;
                self.time_up.store(true, Ordering::Relaxed);
            }
        }
    }

    fn is_capture(pos: &Position, mv: &Move) -> bool {
        mv.flag == crate::board::FLAG_EP || pos.occ[1 - pos.side] & (1u64 << mv.to) != 0
    }

    fn mvv_lva(pos: &Position, mv: &Move) -> i32 {
        let victim = if mv.flag == crate::board::FLAG_EP {
            PAWN
        } else {
            pos.piece_at(mv.to).map(|(_, p)| p).unwrap_or(PAWN)
        };
        let attacker = pos.piece_at(mv.from).map(|(_, p)| p).unwrap_or(PAWN);
        const VALUE: [i32; 6] = [100, 320, 330, 500, 900, 10000];
        VALUE[victim] * 10 - VALUE[attacker]
    }

    fn order_moves(&self, pos: &Position, mut moves: Vec<Move>, tt_move: Option<Move>, ply: usize) -> Vec<Move> {
        if !self.opts.use_ordering { return moves; }
        let killers = self.killers[ply];
        let us = pos.side;
        let score_of = |mv: &Move| -> i32 {
            if Some(*mv) == tt_move { return 1_000_000; }
            if Self::is_capture(pos, mv) { return 500_000 + Self::mvv_lva(pos, mv); }
            if Some(*mv) == killers[0] { return 90_000; }
            if Some(*mv) == killers[1] { return 80_000; }
            self.history[us][mv.from as usize][mv.to as usize]
        };
        moves.sort_by_key(|mv| std::cmp::Reverse(score_of(mv)));
        moves
    }

    fn null_move_ok(pos: &Position) -> bool {
        // zugzwang guard: only try null-move when the side to move has
        // pieces beyond king+pawns
        let us = pos.side;
        let non_pawn_king = pos.occ[us] & !(pos.pieces[us][PAWN] | pos.pieces[us][KING]);
        non_pawn_king != 0
    }

    fn make_null_move(pos: &Position) -> Position {
        let mut np = *pos;
        np.side = 1 - pos.side;
        np.ep_square = None;
        np.hash = np.compute_hash();
        np
    }

    fn quiescence(&mut self, pos: &Position, mut alpha: i32, beta: i32, ply: usize) -> i32 {
        self.nodes += 1;
        self.check_time();
        if self.stop { return 0; }
        let stand_pat = evaluate(pos);
        if !self.opts.use_quiescence { return stand_pat; }
        if stand_pat >= beta { return beta; }
        if stand_pat > alpha { alpha = stand_pat; }
        if ply >= MAX_PLY { return alpha; }

        let mut moves: Vec<Move> = gen_legal(pos).into_iter().filter(|mv| Self::is_capture(pos, mv)).collect();
        moves = self.order_moves(pos, moves, None, ply.min(MAX_PLY));
        for mv in moves {
            let np = make_move(pos, &mv);
            let score = -self.quiescence(&np, -beta, -alpha, ply + 1);
            if self.stop { return 0; }
            if score >= beta { return beta; }
            if score > alpha { alpha = score; }
        }
        alpha
    }

    fn negamax(&mut self, pos: &Position, depth: i32, ply: usize, mut alpha: i32, mut beta: i32) -> i32 {
        self.nodes += 1;
        self.check_time();
        if self.stop { return 0; }

        let in_chk = in_check(pos, pos.side);
        if depth <= 0 {
            return self.quiescence(pos, alpha, beta, ply);
        }

        let orig_alpha = alpha;
        let tt_hit = self.tt_probe(pos.hash);
        let tt_move = tt_hit.and_then(|e| e.best);
        if let Some(e) = tt_hit {
            if e.depth as i32 >= depth {
                match e.flag {
                    TT_FLAG_EXACT => return e.score,
                    TT_FLAG_LOWER => alpha = alpha.max(e.score),
                    TT_FLAG_UPPER => beta = beta.min(e.score),
                    _ => {}
                }
                if alpha >= beta { return e.score; }
            }
        }

        // Null-move pruning
        if self.opts.use_null_move && depth >= 3 && !in_chk && ply > 0 && Self::null_move_ok(pos) {
            let np = Self::make_null_move(pos);
            let r = 2;
            let score = -self.negamax(&np, depth - 1 - r, ply + 1, -beta, -beta + 1);
            if self.stop { return 0; }
            if score >= beta {
                return beta;
            }
        }

        let moves = gen_legal(pos);
        if moves.is_empty() {
            return if in_chk { -MATE_SCORE + ply as i32 } else { 0 };
        }
        let moves = self.order_moves(pos, moves, tt_move, ply.min(MAX_PLY));

        let mut best_score = -MATE_SCORE - 1;
        let mut best_move: Option<Move> = None;
        for (i, mv) in moves.iter().enumerate() {
            let np = make_move(pos, mv);
            let is_cap = Self::is_capture(pos, mv);
            let gives_check = in_check(&np, np.side);

            let mut score;
            if self.opts.use_pvs && i > 0 {
                // Late move reduction for quiet, late, non-checking moves
                let mut reduction = 0;
                if self.opts.use_lmr && depth >= 3 && i >= 3 && !is_cap && !gives_check && !in_chk {
                    reduction = 1 + (i as i32 >= 8) as i32;
                }
                score = -self.negamax(&np, depth - 1 - reduction, ply + 1, -alpha - 1, -alpha);
                if !self.stop && score > alpha && (reduction > 0 || score < beta) {
                    score = -self.negamax(&np, depth - 1, ply + 1, -beta, -alpha);
                }
            } else {
                score = -self.negamax(&np, depth - 1, ply + 1, -beta, -alpha);
            }
            if self.stop { return 0; }

            if score > best_score {
                best_score = score;
                best_move = Some(*mv);
            }
            if score > alpha { alpha = score; }
            if alpha >= beta {
                if !is_cap {
                    let ply_idx = ply.min(MAX_PLY);
                    if self.killers[ply_idx][0] != Some(*mv) {
                        self.killers[ply_idx][1] = self.killers[ply_idx][0];
                        self.killers[ply_idx][0] = Some(*mv);
                    }
                    self.history[pos.side][mv.from as usize][mv.to as usize] += depth * depth;
                }
                break;
            }
        }

        let flag = if best_score <= orig_alpha {
            TT_FLAG_UPPER
        } else if best_score >= beta {
            TT_FLAG_LOWER
        } else {
            TT_FLAG_EXACT
        };
        self.tt_store(pos.hash, depth as i8, best_score, flag, best_move);
        best_score
    }

    fn extract_pv(&self, pos: &Position, max_len: usize) -> Vec<Move> {
        let mut pv = Vec::new();
        let mut cur = *pos;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..max_len {
            if !seen.insert(cur.hash) { break; }
            let entry = match self.tt_probe(cur.hash) { Some(e) => e, None => break };
            let mv = match entry.best { Some(m) => m, None => break };
            if !gen_legal(&cur).contains(&mv) { break; }
            pv.push(mv);
            cur = make_move(&cur, &mv);
        }
        pv
    }

    /// Iterative deepening driver. Calls `on_depth` after each completed
    /// depth (used by the UCI handler to emit `info` lines).
    pub fn iterative_deepening(&mut self, pos: &Position, limits: &SearchLimits, mut on_depth: impl FnMut(&DepthInfo)) -> Option<Move> {
        self.nodes = 0;
        self.stop = false;
        self.start_ms = now_ms();
        self.deadline_ms = limits.movetime_ms.map(|ms| self.start_ms + ms as f64);
        let max_depth = limits.depth.unwrap_or(64).min(64) as i32;

        let mut best_move = None;
        let mut prev_score = 0;
        for depth in 1..=max_depth {
            let (mut alpha, mut beta) = (-MATE_SCORE - 1, MATE_SCORE + 1);
            if self.opts.use_aspiration && depth >= 4 {
                alpha = prev_score - 50;
                beta = prev_score + 50;
            }
            let mut score;
            loop {
                score = self.negamax(pos, depth, 0, alpha, beta);
                if self.stop { break; }
                if score <= alpha {
                    alpha = (alpha - 100).max(-MATE_SCORE - 1);
                } else if score >= beta {
                    beta = (beta + 100).min(MATE_SCORE + 1);
                } else {
                    break;
                }
            }
            if self.stop && depth > 1 { break; }

            prev_score = score;
            let pv = self.extract_pv(pos, depth as usize);
            if let Some(&mv) = pv.first() {
                best_move = Some(mv);
            }
            self.last_pv = pv.clone();
            let mate = if score.abs() > MATE_SCORE - 1000 {
                let plies = MATE_SCORE - score.abs();
                Some(if score > 0 { (plies + 1) / 2 } else { -((plies + 1) / 2) })
            } else {
                None
            };
            on_depth(&DepthInfo {
                depth: depth as u32,
                score_cp: score,
                mate,
                nodes: self.nodes,
                time_ms: (now_ms() - self.start_ms) as u64,
                pv,
            });
            if self.stop { break; }
        }
        // Emergency fallback: an aborted depth-1 search (pathologically tiny
        // movetime) can leave best_move unset. Never hand back "no move".
        best_move.or_else(|| gen_legal(pos).into_iter().next())
    }

    pub fn stop_now(&mut self) { self.stop = true; }

    /// Lazy-SMP: run `num_threads` searches of the same position in
    /// parallel, all reading/writing the shared transposition table, so
    /// each thread's discoveries speed up the others via TT hits on
    /// transposed paths. Only this (the calling) thread's result is
    /// reported — the helper threads exist purely to seed the shared TT,
    /// same as every other production Lazy-SMP implementation.
    pub fn iterative_deepening_mt(&mut self, pos: &Position, limits: &SearchLimits, num_threads: usize, on_depth: impl FnMut(&DepthInfo) + Send) -> Option<Move> {
        if num_threads <= 1 {
            return self.iterative_deepening(pos, limits, on_depth);
        }
        self.start_ms = now_ms();
        self.deadline_ms = limits.movetime_ms.map(|ms| self.start_ms + ms as f64);
        self.time_up = Arc::new(AtomicBool::new(false));

        let mut helpers: Vec<Search> = (1..num_threads).map(|_| self.spawn_helper()).collect();
        let mut best_move = None;
        rayon::scope(|s| {
            for h in helpers.iter_mut() {
                let p = *pos;
                let hlimits = SearchLimits { depth: limits.depth, movetime_ms: limits.movetime_ms };
                s.spawn(move |_| { h.iterative_deepening(&p, &hlimits, |_| {}); });
            }
            best_move = self.iterative_deepening(pos, limits, on_depth);
        });
        best_move
    }
}
