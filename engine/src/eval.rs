// Handcrafted evaluation: material + piece-square tables + mobility + basic
// king safety, per ENGINE_ARCHITECTURE.md §4. Score is centipawns from the
// side-to-move's perspective (negamax convention).

use crate::board::*;
use crate::movegen::{bishop_attacks, gen_pseudo_legal, king_square, queen_attacks, rook_attacks, tables};

pub const MATE_SCORE: i32 = 30000;

const PIECE_VALUE: [i32; 6] = [100, 320, 330, 500, 900, 0];

// Standard "simplified eval" PSTs (Tomasz Michniewski), indexed a1..h8
// (rank 1 first). White's perspective; mirror the rank for black.
#[rustfmt::skip]
const PAWN_PST: [i32; 64] = [
      0,  0,  0,  0,  0,  0,  0,  0,
     50, 50, 50, 50, 50, 50, 50, 50,
     10, 10, 20, 30, 30, 20, 10, 10,
      5,  5, 10, 25, 25, 10,  5,  5,
      0,  0,  0, 20, 20,  0,  0,  0,
      5, -5,-10,  0,  0,-10, -5,  5,
      5, 10, 10,-20,-20, 10, 10,  5,
      0,  0,  0,  0,  0,  0,  0,  0,
];
#[rustfmt::skip]
const KNIGHT_PST: [i32; 64] = [
    -50,-40,-30,-30,-30,-30,-40,-50,
    -40,-20,  0,  0,  0,  0,-20,-40,
    -30,  0, 10, 15, 15, 10,  0,-30,
    -30,  5, 15, 20, 20, 15,  5,-30,
    -30,  0, 15, 20, 20, 15,  0,-30,
    -30,  5, 10, 15, 15, 10,  5,-30,
    -40,-20,  0,  5,  5,  0,-20,-40,
    -50,-40,-30,-30,-30,-30,-40,-50,
];
#[rustfmt::skip]
const BISHOP_PST: [i32; 64] = [
    -20,-10,-10,-10,-10,-10,-10,-20,
    -10,  0,  0,  0,  0,  0,  0,-10,
    -10,  0,  5, 10, 10,  5,  0,-10,
    -10,  5,  5, 10, 10,  5,  5,-10,
    -10,  0, 10, 10, 10, 10,  0,-10,
    -10, 10, 10, 10, 10, 10, 10,-10,
    -10,  5,  0,  0,  0,  0,  5,-10,
    -20,-10,-10,-10,-10,-10,-10,-20,
];
#[rustfmt::skip]
const ROOK_PST: [i32; 64] = [
      0,  0,  0,  0,  0,  0,  0,  0,
      5, 10, 10, 10, 10, 10, 10,  5,
     -5,  0,  0,  0,  0,  0,  0, -5,
     -5,  0,  0,  0,  0,  0,  0, -5,
     -5,  0,  0,  0,  0,  0,  0, -5,
     -5,  0,  0,  0,  0,  0,  0, -5,
     -5,  0,  0,  0,  0,  0,  0, -5,
      0,  0,  0,  5,  5,  0,  0,  0,
];
#[rustfmt::skip]
const QUEEN_PST: [i32; 64] = [
    -20,-10,-10, -5, -5,-10,-10,-20,
    -10,  0,  0,  0,  0,  0,  0,-10,
    -10,  0,  5,  5,  5,  5,  0,-10,
     -5,  0,  5,  5,  5,  5,  0, -5,
      0,  0,  5,  5,  5,  5,  0, -5,
    -10,  5,  5,  5,  5,  5,  0,-10,
    -10,  0,  5,  0,  0,  0,  0,-10,
    -20,-10,-10, -5, -5,-10,-10,-20,
];
#[rustfmt::skip]
const KING_MG_PST: [i32; 64] = [
    -30,-40,-40,-50,-50,-40,-40,-30,
    -30,-40,-40,-50,-50,-40,-40,-30,
    -30,-40,-40,-50,-50,-40,-40,-30,
    -30,-40,-40,-50,-50,-40,-40,-30,
    -20,-30,-30,-40,-40,-30,-30,-20,
    -10,-20,-20,-20,-20,-20,-20,-10,
     20, 20,  0,  0,  0,  0, 20, 20,
     20, 30, 10,  0,  0, 10, 30, 20,
];
#[rustfmt::skip]
const KING_EG_PST: [i32; 64] = [
    -50,-40,-30,-20,-20,-30,-40,-50,
    -30,-20,-10,  0,  0,-10,-20,-30,
    -30,-10, 20, 30, 30, 20,-10,-30,
    -30,-10, 30, 40, 40, 30,-10,-30,
    -30,-10, 30, 40, 40, 30,-10,-30,
    -30,-10, 20, 30, 30, 20,-10,-30,
    -30,-30,  0,  0,  0,  0,-30,-30,
    -50,-30,-30,-30,-30,-30,-30,-50,
];

fn pst_value(ptype: usize, color: usize, s: u8, phase: f32) -> i32 {
    // Tables above are written rank8-first (standard "visual board" layout
    // from the published source), but squares are a1=0-indexed here, so a
    // White piece needs its rank flipped to land on the matching table row;
    // a Black piece's natural (already-flipped) perspective lines up as-is.
    let idx = if color == WHITE { (s ^ 56) as usize } else { s as usize };
    match ptype {
        PAWN => PAWN_PST[idx],
        KNIGHT => KNIGHT_PST[idx],
        BISHOP => BISHOP_PST[idx],
        ROOK => ROOK_PST[idx],
        QUEEN => QUEEN_PST[idx],
        KING => {
            let mg = KING_MG_PST[idx] as f32;
            let eg = KING_EG_PST[idx] as f32;
            (mg * phase + eg * (1.0 - phase)) as i32
        }
        _ => 0,
    }
}

const MOBILITY_WEIGHT: [i32; 6] = [0, 4, 4, 2, 1, 0];

fn game_phase(pos: &Position) -> f32 {
    // 0 = endgame, 1 = full midgame material, based on remaining non-pawn
    // material relative to the starting amount.
    let max_phase = 4 * (320 + 330 + 500) + 2 * 900; // 2N+2B+2R+1Q per side... approximated with both sides below
    let mut total = 0i32;
    for c in 0..2 {
        total += (pos.pieces[c][KNIGHT].count_ones() as i32) * 320;
        total += (pos.pieces[c][BISHOP].count_ones() as i32) * 330;
        total += (pos.pieces[c][ROOK].count_ones() as i32) * 500;
        total += (pos.pieces[c][QUEEN].count_ones() as i32) * 900;
    }
    (total as f32 / max_phase as f32).clamp(0.0, 1.0)
}

fn king_safety(pos: &Position, color: usize) -> i32 {
    let ks = king_square(pos, color);
    let kf = file_of(ks) as i32;
    let mut score = 0;
    let (shield_rank, dir): (i32, i32) = if color == WHITE { (rank_of(ks) as i32 + 1, 1) } else { (rank_of(ks) as i32 - 1, -1) };
    for df in -1..=1 {
        let f = kf + df;
        if !(0..8).contains(&f) { continue; }
        let own_pawns_on_file = {
            let file_mask = FILE_A << f;
            pos.pieces[color][PAWN] & file_mask
        };
        if own_pawns_on_file == 0 {
            score -= 15; // open/half-open file next to king
        }
        if (0..8).contains(&shield_rank) {
            let shield_sq = sq(f as u8, shield_rank as u8);
            if pos.pieces[color][PAWN] & (1u64 << shield_sq) != 0 {
                score += 10;
            }
        }
    }
    let _ = dir;
    score
}

pub fn mobility(pos: &Position, color: usize) -> i32 {
    let t = tables();
    let occ = pos.all_occ;
    let own = pos.occ[color];
    let mut score = 0;
    let mut knights = pos.pieces[color][KNIGHT];
    while knights != 0 {
        let s = knights.trailing_zeros() as u8;
        knights &= knights - 1;
        score += (t.knight[s as usize] & !own).count_ones() as i32 * MOBILITY_WEIGHT[KNIGHT];
    }
    let mut bishops = pos.pieces[color][BISHOP];
    while bishops != 0 {
        let s = bishops.trailing_zeros() as u8;
        bishops &= bishops - 1;
        score += (bishop_attacks(s, occ) & !own).count_ones() as i32 * MOBILITY_WEIGHT[BISHOP];
    }
    let mut rooks = pos.pieces[color][ROOK];
    while rooks != 0 {
        let s = rooks.trailing_zeros() as u8;
        rooks &= rooks - 1;
        score += (rook_attacks(s, occ) & !own).count_ones() as i32 * MOBILITY_WEIGHT[ROOK];
    }
    let mut queens = pos.pieces[color][QUEEN];
    while queens != 0 {
        let s = queens.trailing_zeros() as u8;
        queens &= queens - 1;
        score += (queen_attacks(s, occ) & !own).count_ones() as i32 * MOBILITY_WEIGHT[QUEEN];
    }
    score
}

pub fn evaluate(pos: &Position) -> i32 {
    let phase = game_phase(pos);
    let mut score = [0i32; 2];
    for color in 0..2 {
        for ptype in 0..6 {
            let mut bb = pos.pieces[color][ptype];
            while bb != 0 {
                let s = bb.trailing_zeros() as u8;
                bb &= bb - 1;
                score[color] += PIECE_VALUE[ptype] + pst_value(ptype, color, s, phase);
            }
        }
        score[color] += mobility(pos, color);
        score[color] += king_safety(pos, color);
    }
    let white_score = score[WHITE] - score[BLACK];
    if pos.side == WHITE { white_score } else { -white_score }
}

/// Cheap fallback used only to avoid pulling in full legality checks where a
/// pseudo-legal move count is an adequate proxy (used by mobility above via
/// attack bitboards directly, not this function — kept for eval.rs debug use).
#[allow(dead_code)]
pub fn pseudo_move_count(pos: &Position) -> usize {
    gen_pseudo_legal(pos).len()
}
