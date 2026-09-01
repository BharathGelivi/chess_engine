// perft(depth): count leaf nodes of the legal move tree at fixed depth.
// The standard move-generation correctness gate — compared against published
// values from the Chess Programming Wiki "Perft Results" page.

use crate::board::Position;
use crate::movegen::{gen_legal, make_move};

pub fn perft(pos: &Position, depth: u32) -> u64 {
    if depth == 0 { return 1; }
    let moves = gen_legal(pos);
    if depth == 1 { return moves.len() as u64; }
    let mut nodes = 0u64;
    for mv in moves {
        let np = make_move(pos, &mv);
        nodes += perft(&np, depth - 1);
    }
    nodes
}

/// Per-move breakdown at the root, for `go perft N` / debugging divergence
/// against a reference perft.
pub fn perft_divide(pos: &Position, depth: u32) -> Vec<(String, u64)> {
    gen_legal(pos)
        .into_iter()
        .map(|mv| {
            let np = make_move(pos, &mv);
            let n = if depth <= 1 { 1 } else { perft(&np, depth - 1) };
            (mv.to_uci(), n)
        })
        .collect()
}
