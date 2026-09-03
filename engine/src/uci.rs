// UCI-lite protocol handler: `position fen ...` / `position startpos moves
// ...`, `go depth N` / `go movetime N`, `stop`, `uci`, `isready`,
// `ucinewgame`. Emits `info depth N score cp N nodes N time N pv ...` and
// `bestmove ...` — same line shapes the bundled Stockfish build emits, so
// App.tsx's existing regex-based info-line parser needs no changes to
// handle this engine as a second Worker.

use crate::board::Position;
use crate::eval::evaluate;
use crate::movegen::parse_uci_move;
use crate::perft::perft;
use crate::search::{DepthInfo, Search, SearchLimits, SearchOptions};

pub struct Engine {
    pos: Position,
    search: Search,
}

impl Engine {
    pub fn new() -> Self {
        Engine { pos: Position::startpos(), search: Search::new(SearchOptions::default()) }
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
            Some("setoption") => { /* accepted, no-op: no tunable options exposed yet */ }
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

        // Known theory: play it instantly rather than spending the search
        // budget re-deriving moves that are already well-established, and
        // sidestep the shallow search's occasional preference for a first
        // move that's fine tactically but poor by known theory.
        if let Some(mv) = crate::book::book_move(&self.pos) {
            let score_cp = evaluate(&self.pos);
            emit(format!("info depth 0 score cp {score_cp} nodes 0 nps 0 time 0 pv {}", mv.to_uci()));
            emit(format!("bestmove {}", mv.to_uci()));
            return;
        }

        let pos = self.pos;
        // Lazy-SMP: capped at 4 threads — a shared-TT search sees diminishing
        // (and eventually negative, from lock contention) returns well
        // before using every core on a big machine.
        let num_threads = rayon::current_num_threads().min(4);
        let best = self.search.iterative_deepening_mt(&pos, &limits, num_threads, |info: &DepthInfo| {
            emit(format_info(info));
        });
        match best {
            Some(mv) => emit(format!("bestmove {}", mv.to_uci())),
            None => emit("bestmove 0000".to_string()),
        }
    }
}

fn format_info(info: &DepthInfo) -> String {
    let score = match info.mate {
        Some(m) => format!("mate {m}"),
        None => format!("cp {}", info.score_cp),
    };
    let pv: Vec<String> = info.pv.iter().map(|m| m.to_uci()).collect();
    let nps = if info.time_ms > 0 { info.nodes * 1000 / info.time_ms } else { info.nodes };
    format!(
        "info depth {} score {} nodes {} nps {} time {} pv {}",
        info.depth, score, info.nodes, nps, info.time_ms, pv.join(" ")
    )
}
