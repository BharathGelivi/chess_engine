// Opening book: a small set of well-known opening lines, keyed by position
// (Zobrist hash) rather than move sequence, so any position transposed into
// still hits the book. Built once from human-readable UCI move lists rather
// than a binary Polyglot book — simpler to read/extend, and plenty for
// keeping the engine "in book" through common openings instead of spending
// search time re-deriving established theory.

use crate::board::{Move, Position};
use crate::movegen::{make_move, parse_uci_move};
use std::collections::HashMap;
use std::sync::OnceLock;

#[rustfmt::skip]
const LINES: &[&[&str]] = &[
    // Italian Game
    &["e2e4", "e7e5", "g1f3", "b8c6", "f1c4", "f8c5", "c2c3", "g8f6", "d2d3"],
    // Ruy Lopez, Morphy Defense
    &["e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6", "b5a4", "g8f6", "e1g1", "f8e7"],
    // Scotch Game
    &["e2e4", "e7e5", "g1f3", "b8c6", "d2d4", "e5d4", "f3d4", "g8f6"],
    // Open Sicilian, Najdorf setup
    &["e2e4", "c7c5", "g1f3", "d7d6", "d2d4", "c5d4", "f3d4", "g8f6", "b1c3", "a7a6"],
    // Open Sicilian, Dragon setup
    &["e2e4", "c7c5", "g1f3", "d7d6", "d2d4", "c5d4", "f3d4", "g8f6", "b1c3", "g7g6"],
    // Caro-Kann
    &["e2e4", "c7c6", "d2d4", "d7d5", "b1c3", "d5e4", "c3e4", "c8f5"],
    // French Defense
    &["e2e4", "e7e6", "d2d4", "d7d5", "b1c3", "g8f6", "c1g5", "f8e7"],
    // Queen's Gambit Declined
    &["d2d4", "d7d5", "c2c4", "e7e6", "b1c3", "g8f6", "c1g5", "f8e7"],
    // Slav Defense
    &["d2d4", "d7d5", "c2c4", "c7c6", "g1f3", "g8f6", "b1c3", "d5c4"],
    // King's Indian Defense
    &["d2d4", "g8f6", "c2c4", "g7g6", "b1c3", "f8g7", "e2e4", "d7d6"],
    // Nimzo-Indian
    &["d2d4", "g8f6", "c2c4", "e7e6", "b1c3", "f8b4"],
    // English Opening
    &["c2c4", "e7e5", "b1c3", "g8f6", "g1f3", "b8c6"],
];

fn book_map() -> &'static HashMap<u64, Vec<Move>> {
    static MAP: OnceLock<HashMap<u64, Vec<Move>>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map: HashMap<u64, Vec<Move>> = HashMap::new();
        for line in LINES {
            let mut pos = Position::startpos();
            for mv_str in *line {
                let Some(mv) = parse_uci_move(&pos, mv_str) else { break };
                let candidates = map.entry(pos.hash).or_default();
                if !candidates.contains(&mv) { candidates.push(mv); }
                pos = make_move(&pos, &mv);
            }
        }
        map
    })
}

/// A book move for `pos`, if any of the known lines pass through it. Picks
/// among tied candidates with a time-seeded index — not a real RNG, just
/// enough to vary which known line gets played across games.
pub fn book_move(pos: &Position) -> Option<Move> {
    let candidates = book_map().get(&pos.hash)?;
    let idx = (crate::search::now_ms() as u64 as usize) % candidates.len();
    candidates.get(idx).copied()
}
