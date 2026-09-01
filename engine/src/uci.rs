// UCI-lite protocol handler: `position fen ...` / `position startpos moves
// ...`, `go depth N` / `go movetime N`, `stop`, `uci`, `isready`,
// `ucinewgame`. Emits `info depth N score cp N nodes N time N pv ...` and
// `bestmove ...` — same line shapes the bundled Stockfish build emits, so
// App.tsx's existing regex-based info-line parser needs no changes to
// handle this engine as a second Worker.

use crate::board::{parse_sq, Move, Position, PROMO_B, PROMO_N, PROMO_Q, PROMO_R};
use crate::movegen::gen_legal;
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

    fn parse_move(pos: &Position, s: &str) -> Option<Move> {
        let from = parse_sq(&s[0..2])?;
        let to = parse_sq(&s[2..4])?;
        let promo = match s.as_bytes().get(4) {
            Some(b'q') => PROMO_Q,
            Some(b'r') => PROMO_R,
            Some(b'b') => PROMO_B,
            Some(b'n') => PROMO_N,
            _ => 0,
        };
        gen_legal(pos).into_iter().find(|m| m.from == from && m.to == to && m.promo == promo)
    }

    pub fn handle_line(&mut self, line: &str, emit: &mut dyn FnMut(String)) {
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
            if let Some(mv) = Self::parse_move(&self.pos, mv_str) {
                self.pos = crate::movegen::make_move(&self.pos, &mv);
            }
        }
    }

    fn handle_go(&mut self, it: &mut std::str::SplitWhitespace, emit: &mut dyn FnMut(String)) {
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
        let best = self.search.iterative_deepening(&pos, &limits, |info: &DepthInfo| {
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
