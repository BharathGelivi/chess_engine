// UCI-lite protocol handler: `position fen ...` / `position startpos moves
// ...`, `go depth N` / `go movetime N`, `stop`, `uci`, `isready`,
// `ucinewgame`. Emits `info depth N score cp N nodes N time N pv ...` and
// `bestmove ...` — same line shapes the bundled Stockfish build emits, so
// App.tsx's existing regex-based info-line parser needs no changes to
// handle this engine as a second Worker.

use crate::board::{Move, Position};
use crate::eval::evaluate;
use crate::movegen::parse_uci_move;
use crate::perft::perft;
use crate::search::{DepthInfo, Search, SearchLimits, SearchOptions};

pub struct Engine {
    pos: Position,
    search: Search,
    multipv: usize,
}

impl Engine {
    pub fn new() -> Self {
        Engine { pos: Position::startpos(), search: Search::new(SearchOptions::default()), multipv: 1 }
    }

    pub fn handle_line(&mut self, line: &str, emit: &mut (dyn FnMut(String) + Send)) {
        let line = line.trim();
        let mut it = line.split_whitespace();
        match it.next() {
            Some("uci") => {
                emit("id name BOARDROOM-Engine".to_string());
                emit("id author BOARDROOM".to_string());
                emit("uciok".to_string());
            }
            Some("isready") => emit("readyok".to_string()),
            Some("ucinewgame") => { self.pos = Position::startpos(); self.search = Search::new(SearchOptions::default()); }
            Some("setoption") => self.handle_setoption(&mut it),
            Some("position") => self.handle_position(&mut it),
            Some("go") => self.handle_go(&mut it, emit),
            Some("stop") => self.search.stop_now(),
            Some("perft") => {
                if let Some(d) = it.next().and_then(|s| s.parse::<u32>().ok()) {
                    let n = perft(&self.pos, d);
                    emit(format!("perft {d} nodes {n}"));
                }
            }
            Some("quit") => {}
            _ => {}
        }
    }

    fn handle_position(&mut self, it: &mut std::str::SplitWhitespace) {
        match it.next() {
            Some("startpos") => {
                self.pos = Position::startpos();
                match it.next() {
                    Some("moves") => self.apply_moves(it),
                    _ => {}
                }
            }
            Some("fen") => {
                let mut fen_tokens = Vec::new();
                let mut hit_moves = false;
                for tok in it.by_ref() {
                    if tok == "moves" { hit_moves = true; break; }
                    fen_tokens.push(tok);
                }
                let fen = fen_tokens.join(" ");
                if let Some(p) = Position::from_fen(&fen) { self.pos = p; }
                if hit_moves { self.apply_moves(it); }
            }
            _ => {}
        }
    }

    /// Only `name MultiPV value N` is recognized (clamped 1-5, matching
    /// what the frontend ever asks for); every other option is accepted
    /// and ignored — no other tunables are exposed yet.
    fn handle_setoption(&mut self, it: &mut std::str::SplitWhitespace) {
        let mut name = String::new();
        for tok in it.by_ref() {
            if tok == "value" { break; }
            if tok != "name" { name.push_str(tok); }
        }
        if name.eq_ignore_ascii_case("MultiPV") {
            if let Some(n) = it.next().and_then(|s| s.parse::<usize>().ok()) {
                self.multipv = n.clamp(1, 5);
            }
        }
    }

    fn apply_moves(&mut self, it: &mut std::str::SplitWhitespace) {
        for mv_str in it {
            if let Some(mv) = parse_uci_move(&self.pos, mv_str) {
                self.pos = crate::movegen::make_move(&self.pos, &mv);
            }
        }
    }

    fn handle_go(&mut self, it: &mut std::str::SplitWhitespace, emit: &mut (dyn FnMut(String) + Send)) {
        let mut limits = SearchLimits { depth: None, movetime_ms: None };
        while let Some(tok) = it.next() {
            match tok {
                "depth" => limits.depth = it.next().and_then(|s| s.parse().ok()),
                "movetime" => limits.movetime_ms = it.next().and_then(|s| s.parse().ok()),
                _ => {}
            }
        }
        if limits.depth.is_none() && limits.movetime_ms.is_none() {
            limits.depth = Some(6);
        }

        let pos = self.pos;
        // Lazy-SMP: capped at 4 threads — a shared-TT search sees diminishing
        // (and eventually negative, from lock contention) returns well
        // before using every core on a big machine.
        let num_threads = rayon::current_num_threads().min(4);

        // MultiPV: one search per requested line, each excluding whatever
        // moves earlier lines already reported (Search::root_exclude) —
        // there's no shared-PV-window search here, so this is N searches,
        // not one. Known theory plays instantly as line 1 (see book.rs);
        // the first *searched* line gets the full requested budget and
        // streams every completed depth live, same as the old single-PV
        // behavior — every line after that gets a smaller fixed budget,
        // since giving all `multipv` lines the full budget would multiply
        // total analysis time by `multipv`, and lines 2-5 are "what else is
        // reasonable", not the move that's actually going to get played.
        const ALT_LINE_MOVETIME_MS: u64 = 750;
        let mut exclude: Vec<Move> = Vec::new();
        let mut best_move: Option<Move> = None;

        if let Some(mv) = crate::book::book_move(&pos) {
            let score_cp = evaluate(&pos);
            emit(format!("info depth 0 score cp {score_cp} multipv 1 nodes 0 nps 0 time 0 pv {}", mv.to_uci()));
            exclude.push(mv);
            best_move = Some(mv);
        }

        for i in exclude.len()..self.multipv {
            let sub_limits = if i == 0 {
                SearchLimits { depth: limits.depth, movetime_ms: limits.movetime_ms }
            } else {
                SearchLimits { depth: limits.depth, movetime_ms: limits.movetime_ms.map(|_| ALT_LINE_MOVETIME_MS) }
            };
            self.search.set_root_exclude(exclude.clone());
            let multipv_idx = i + 1;
            let mv = self.search.iterative_deepening_mt(&pos, &sub_limits, num_threads, |info: &DepthInfo| {
                emit(format_info(info, multipv_idx));
            });
            self.search.set_root_exclude(Vec::new());
            match mv {
                Some(mv) => {
                    if best_move.is_none() { best_move = Some(mv); }
                    exclude.push(mv);
                }
                None => break, // fewer legal moves than requested lines
            }
        }

        match best_move {
            Some(mv) => emit(format!("bestmove {}", mv.to_uci())),
            None => emit("bestmove 0000".to_string()),
        }
    }
}

fn format_info(info: &DepthInfo, multipv: usize) -> String {
    let score = match info.mate {
        Some(m) => format!("mate {m}"),
        None => format!("cp {}", info.score_cp),
    };
    let pv: Vec<String> = info.pv.iter().map(|m| m.to_uci()).collect();
    let nps = if info.time_ms > 0 { info.nodes * 1000 / info.time_ms } else { info.nodes };
    format!(
        "info depth {} score {} multipv {} nodes {} nps {} time {} pv {}",
        info.depth, score, multipv, info.nodes, nps, info.time_ms, pv.join(" ")
    )
}
