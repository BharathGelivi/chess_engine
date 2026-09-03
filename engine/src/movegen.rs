// Move generation: precomputed leaper attack tables + magic bitboards for
// sliding pieces (bishop/rook/queen), generated at process startup via
// trial-and-error search (Chess Programming Wiki "Looking for Magics")
// rather than hardcoded magic constants.
//
// Legality: pseudo-legal generation, then brute-force filter by making the
// move on a Position copy and checking the mover's king isn't attacked
// afterward. This also correctly handles the en-passant-discovered-check
// edge case for free (no separate pin detection needed).
// ponytail: brute-force make+check-test for legality instead of pin-aware
// generation — simpler/harder to get wrong; revisit if perft NPS profiling
// shows legality filtering dominates search time.

use crate::board::*;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use std::sync::OnceLock;

#[inline]
fn pop_lsb(bb: &mut u64) -> u8 {
    let s = bb.trailing_zeros() as u8;
    *bb &= *bb - 1;
    s
}

fn knight_attacks_from(s: u8) -> u64 {
    let f = file_of(s) as i32;
    let r = rank_of(s) as i32;
    let deltas = [(1, 2), (2, 1), (2, -1), (1, -2), (-1, -2), (-2, -1), (-2, 1), (-1, 2)];
    let mut bb = 0u64;
    for (df, dr) in deltas {
        let nf = f + df;
        let nr = r + dr;
        if (0..8).contains(&nf) && (0..8).contains(&nr) {
            bb |= 1u64 << sq(nf as u8, nr as u8);
        }
    }
    bb
}

fn king_attacks_from(s: u8) -> u64 {
    let f = file_of(s) as i32;
    let r = rank_of(s) as i32;
    let mut bb = 0u64;
    for df in -1..=1 {
        for dr in -1..=1 {
            if df == 0 && dr == 0 { continue; }
            let nf = f + df;
            let nr = r + dr;
            if (0..8).contains(&nf) && (0..8).contains(&nr) {
                bb |= 1u64 << sq(nf as u8, nr as u8);
            }
        }
    }
    bb
}

fn pawn_attacks_from(s: u8, color: usize) -> u64 {
    let f = file_of(s) as i32;
    let r = rank_of(s) as i32;
    let dr: i32 = if color == WHITE { 1 } else { -1 };
    let mut bb = 0u64;
    for df in [-1, 1] {
        let nf = f + df;
        let nr = r + dr;
        if (0..8).contains(&nf) && (0..8).contains(&nr) {
            bb |= 1u64 << sq(nf as u8, nr as u8);
        }
    }
    bb
}

// Ray directions for sliding pieces, as (df, dr).
const BISHOP_DIRS: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
const ROOK_DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

fn slide_attacks(s: u8, occ: u64, dirs: &[(i32, i32); 4]) -> u64 {
    let f0 = file_of(s) as i32;
    let r0 = rank_of(s) as i32;
    let mut bb = 0u64;
    for (df, dr) in dirs {
        let mut f = f0 + df;
        let mut r = r0 + dr;
        while (0..8).contains(&f) && (0..8).contains(&r) {
            let t = sq(f as u8, r as u8);
            bb |= 1u64 << t;
            if occ & (1u64 << t) != 0 { break; }
            f += df;
            r += dr;
        }
    }
    bb
}

fn relevant_mask(s: u8, dirs: &[(i32, i32); 4]) -> u64 {
    let f0 = file_of(s) as i32;
    let r0 = rank_of(s) as i32;
    let mut bb = 0u64;
    for (df, dr) in dirs {
        let mut f = f0 + df;
        let mut r = r0 + dr;
        // Walk the ray, stopping one square short of the board edge in the
        // *direction of travel* — i.e. once the next step would leave the
        // board. Gating on "both axes in 1..7" (an earlier version of this
        // function) is wrong for rook rays: one axis is constant along a
        // straight ray, and if that constant axis starts on rank/file 0 or
        // 7 the check fails on step one, producing an empty mask for every
        // rook corner square (bits=0 -> shift=64 -> UB on `u64 >> 64`).
        while (0..8).contains(&f) && (0..8).contains(&r) {
            let nf = f + df;
            let nr = r + dr;
            if !(0..8).contains(&nf) || !(0..8).contains(&nr) { break; }
            bb |= 1u64 << sq(f as u8, r as u8);
            f = nf;
            r = nr;
        }
    }
    bb
}

fn subsets(mask: u64) -> Vec<u64> {
    // enumerate every subset of `mask`'s set bits (standard Carry-Rippler trick)
    let mut out = Vec::with_capacity(1 << mask.count_ones());
    let mut subset: u64 = 0;
    loop {
        out.push(subset);
        if subset == mask { break; }
        subset = subset.wrapping_sub(mask) & mask;
    }
    out
}

pub struct Magic {
    pub mask: u64,
    pub magic: u64,
    pub shift: u32,
    pub offset: usize,
}

pub struct Tables {
    pub knight: [u64; 64],
    pub king: [u64; 64],
    pub pawn: [[u64; 64]; 2],
    pub bishop_magics: [Magic; 64],
    pub rook_magics: [Magic; 64],
    pub bishop_table: Vec<u64>,
    pub rook_table: Vec<u64>,
}

fn find_magic(s: u8, dirs: &[(i32, i32); 4], rng: &mut SmallRng) -> (u64, u64, Vec<u64>) {
    let mask = relevant_mask(s, dirs);
    let bits = mask.count_ones();
    let shift = 64 - bits;
    let occ_subsets = subsets(mask);
    let attacks: Vec<u64> = occ_subsets.iter().map(|&o| slide_attacks(s, o, dirs)).collect();
    let size = 1usize << bits;
    loop {
        // sparse random candidate converges faster (standard trick)
        let magic = rng.gen::<u64>() & rng.gen::<u64>() & rng.gen::<u64>();
        if ((mask.wrapping_mul(magic)) >> 56).count_ones() < 6 { continue; }
        let mut table = vec![u64::MAX; size];
        let mut ok = true;
        for (i, &occ) in occ_subsets.iter().enumerate() {
            let idx = ((occ.wrapping_mul(magic)) >> shift) as usize;
            if table[idx] == u64::MAX {
                table[idx] = attacks[i];
            } else if table[idx] != attacks[i] {
                ok = false;
                break;
            }
        }
        if ok {
            return (magic, mask, table);
        }
    }
}

fn build_magics(dirs: &[(i32, i32); 4], seed: u64) -> ([Magic; 64], Vec<u64>) {
    let mut rng = SmallRng::seed_from_u64(seed);
    let mut flat = Vec::new();
    let magics: Vec<Magic> = (0..64u8)
        .map(|s| {
            let (magic, mask, table) = find_magic(s, dirs, &mut rng);
            let bits = mask.count_ones();
            let shift = 64 - bits;
            let offset = flat.len();
            flat.extend_from_slice(&table);
            Magic { mask, magic, shift, offset }
        })
        .collect();
    (magics.try_into().unwrap_or_else(|_| unreachable!()), flat)
}

fn init_tables() -> Tables {
    let mut knight = [0u64; 64];
    let mut king = [0u64; 64];
    let mut pawn = [[0u64; 64]; 2];
    for s in 0..64u8 {
        knight[s as usize] = knight_attacks_from(s);
        king[s as usize] = king_attacks_from(s);
        pawn[WHITE][s as usize] = pawn_attacks_from(s, WHITE);
        pawn[BLACK][s as usize] = pawn_attacks_from(s, BLACK);
    }
    let (bishop_magics, bishop_table) = build_magics(&BISHOP_DIRS, 0xB157);
    let (rook_magics, rook_table) = build_magics(&ROOK_DIRS, 0xB00C_D00D);
    Tables { knight, king, pawn, bishop_magics, rook_magics, bishop_table, rook_table }
}

static TABLES: OnceLock<Tables> = OnceLock::new();
pub fn tables() -> &'static Tables {
    TABLES.get_or_init(init_tables)
}

#[inline]
pub fn bishop_attacks(s: u8, occ: u64) -> u64 {
    let t = tables();
    let m = &t.bishop_magics[s as usize];
    let idx = (((occ & m.mask).wrapping_mul(m.magic)) >> m.shift) as usize;
    t.bishop_table[m.offset + idx]
}

#[inline]
pub fn rook_attacks(s: u8, occ: u64) -> u64 {
    let t = tables();
    let m = &t.rook_magics[s as usize];
    let idx = (((occ & m.mask).wrapping_mul(m.magic)) >> m.shift) as usize;
    t.rook_table[m.offset + idx]
}

#[inline]
pub fn queen_attacks(s: u8, occ: u64) -> u64 {
    bishop_attacks(s, occ) | rook_attacks(s, occ)
}

pub fn is_square_attacked(pos: &Position, s: u8, by: usize) -> bool {
    let t = tables();
    if t.pawn[1 - by][s as usize] & pos.pieces[by][PAWN] != 0 { return true; }
    if t.knight[s as usize] & pos.pieces[by][KNIGHT] != 0 { return true; }
    if t.king[s as usize] & pos.pieces[by][KING] != 0 { return true; }
    let occ = pos.all_occ;
    let bq = pos.pieces[by][BISHOP] | pos.pieces[by][QUEEN];
    if bishop_attacks(s, occ) & bq != 0 { return true; }
    let rq = pos.pieces[by][ROOK] | pos.pieces[by][QUEEN];
    if rook_attacks(s, occ) & rq != 0 { return true; }
    false
}

pub fn king_square(pos: &Position, color: usize) -> u8 {
    pos.pieces[color][KING].trailing_zeros() as u8
}

pub fn in_check(pos: &Position, color: usize) -> bool {
    is_square_attacked(pos, king_square(pos, color), 1 - color)
}

fn add_promos(moves: &mut Vec<Move>, from: u8, to: u8, flag: u8) {
    for promo in [PROMO_Q, PROMO_R, PROMO_B, PROMO_N] {
        moves.push(Move { from, to, promo, flag });
    }
}

pub fn gen_pseudo_legal(pos: &Position) -> Vec<Move> {
    let t = tables();
    let us = pos.side;
    let them = 1 - us;
    let own = pos.occ[us];
    let occ = pos.all_occ;
    let mut moves = Vec::with_capacity(48);

    // Pawns
    {
        let mut pawns = pos.pieces[us][PAWN];
        let (push_dir, start_rank, promo_rank): (i32, u64, u64) = if us == WHITE {
            (8, RANK_2, RANK_8)
        } else {
            (-8, RANK_7, RANK_1)
        };
        while pawns != 0 {
            let from = pop_lsb(&mut pawns);
            let one_sq = from as i32 + push_dir;
            if (0..64).contains(&one_sq) {
                let one = one_sq as u8;
                if occ & (1u64 << one) == 0 {
                    if (1u64 << one) & promo_rank != 0 {
                        add_promos(&mut moves, from, one, FLAG_NORMAL);
                    } else {
                        moves.push(Move { from, to: one, promo: PROMO_NONE, flag: FLAG_NORMAL });
                        if (1u64 << from) & start_rank != 0 {
                            let two = (one as i32 + push_dir) as u8;
                            if occ & (1u64 << two) == 0 {
                                moves.push(Move { from, to: two, promo: PROMO_NONE, flag: FLAG_DOUBLE });
                            }
                        }
                    }
                }
            }
            let mut attacks = t.pawn[us][from as usize] & pos.occ[them];
            while attacks != 0 {
                let to = pop_lsb(&mut attacks);
                if (1u64 << to) & promo_rank != 0 {
                    add_promos(&mut moves, from, to, FLAG_NORMAL);
                } else {
                    moves.push(Move { from, to, promo: PROMO_NONE, flag: FLAG_NORMAL });
                }
            }
            if let Some(epsq) = pos.ep_square {
                if t.pawn[us][from as usize] & (1u64 << epsq) != 0 {
                    moves.push(Move { from, to: epsq, promo: PROMO_NONE, flag: FLAG_EP });
                }
            }
        }
    }

    // Knights
    {
        let mut knights = pos.pieces[us][KNIGHT];
        while knights != 0 {
            let from = pop_lsb(&mut knights);
            let mut atk = t.knight[from as usize] & !own;
            while atk != 0 {
                let to = pop_lsb(&mut atk);
                moves.push(Move { from, to, promo: PROMO_NONE, flag: FLAG_NORMAL });
            }
        }
    }

    // Bishops / Rooks / Queens
    let slider_kinds: [(usize, fn(u8, u64) -> u64); 3] = [
        (BISHOP, bishop_attacks as fn(u8, u64) -> u64),
        (ROOK, rook_attacks as fn(u8, u64) -> u64),
        (QUEEN, queen_attacks as fn(u8, u64) -> u64),
    ];
    for (piece, attack_fn) in slider_kinds {
        let mut pieces = pos.pieces[us][piece];
        while pieces != 0 {
            let from = pop_lsb(&mut pieces);
            let mut atk = attack_fn(from, occ) & !own;
            while atk != 0 {
                let to = pop_lsb(&mut atk);
                moves.push(Move { from, to, promo: PROMO_NONE, flag: FLAG_NORMAL });
            }
        }
    }

    // King
    {
        let from = king_square(pos, us);
        let mut atk = t.king[from as usize] & !own;
        while atk != 0 {
            let to = pop_lsb(&mut atk);
            moves.push(Move { from, to, promo: PROMO_NONE, flag: FLAG_NORMAL });
        }
        // Castling
        let (k_flag, q_flag, rank) = if us == WHITE { (CASTLE_WK, CASTLE_WQ, 0u8) } else { (CASTLE_BK, CASTLE_BQ, 7u8) };
        let e_sq = sq(4, rank);
        if from == e_sq && !in_check(pos, us) {
            if pos.castling & k_flag != 0 {
                let f_sq = sq(5, rank);
                let g_sq = sq(6, rank);
                let h_sq = sq(7, rank);
                if pos.piece_at(h_sq) == Some((us, ROOK))
                    && occ & (1u64 << f_sq) == 0 && occ & (1u64 << g_sq) == 0
                    && !is_square_attacked(pos, f_sq, them) && !is_square_attacked(pos, g_sq, them)
                {
                    moves.push(Move { from, to: g_sq, promo: PROMO_NONE, flag: FLAG_CASTLE_K });
                }
            }
            if pos.castling & q_flag != 0 {
                let d_sq = sq(3, rank);
                let c_sq = sq(2, rank);
                let b_sq = sq(1, rank);
                let a_sq = sq(0, rank);
                if pos.piece_at(a_sq) == Some((us, ROOK))
                    && occ & (1u64 << d_sq) == 0 && occ & (1u64 << c_sq) == 0 && occ & (1u64 << b_sq) == 0
                    && !is_square_attacked(pos, d_sq, them) && !is_square_attacked(pos, c_sq, them)
                {
                    moves.push(Move { from, to: c_sq, promo: PROMO_NONE, flag: FLAG_CASTLE_Q });
                }
            }
        }
    }

    moves
}

pub fn make_move(pos: &Position, mv: &Move) -> Position {
    let mut np = *pos;
    let us = pos.side;
    let them = 1 - us;
    let (_, moved_piece) = pos.piece_at(mv.from).expect("make_move: no piece on from-square");
    let from_bit = 1u64 << mv.from;
    let to_bit = 1u64 << mv.to;

    np.halfmove += 1;
    if moved_piece == PAWN { np.halfmove = 0; }

    // Remove any captured piece (including en passant, which captures on a
    // different square than `to`)
    if mv.flag == FLAG_EP {
        let cap_sq = if us == WHITE { mv.to - 8 } else { mv.to + 8 };
        np.pieces[them][PAWN] &= !(1u64 << cap_sq);
        np.halfmove = 0;
    } else if pos.occ[them] & to_bit != 0 {
        for p in 0..6 {
            if np.pieces[them][p] & to_bit != 0 {
                np.pieces[them][p] &= !to_bit;
                break;
            }
        }
        np.halfmove = 0;
    }

    np.pieces[us][moved_piece] &= !from_bit;
    if mv.promo != PROMO_NONE {
        let promo_piece = match mv.promo { PROMO_N => KNIGHT, PROMO_B => BISHOP, PROMO_R => ROOK, _ => QUEEN };
        np.pieces[us][promo_piece] |= to_bit;
    } else {
        np.pieces[us][moved_piece] |= to_bit;
    }

    if mv.flag == FLAG_CASTLE_K || mv.flag == FLAG_CASTLE_Q {
        let rank = rank_of(mv.from);
        let (rook_from, rook_to) = if mv.flag == FLAG_CASTLE_K {
            (sq(7, rank), sq(5, rank))
        } else {
            (sq(0, rank), sq(3, rank))
        };
        np.pieces[us][ROOK] &= !(1u64 << rook_from);
        np.pieces[us][ROOK] |= 1u64 << rook_to;
    }

    np.ep_square = if mv.flag == FLAG_DOUBLE {
        Some(if us == WHITE { mv.from + 8 } else { mv.from - 8 })
    } else {
        None
    };

    // Castling rights: lost when king moves, or when a rook moves/is captured
    // from its home square.
    if moved_piece == KING {
        np.castling &= if us == WHITE { !(CASTLE_WK | CASTLE_WQ) } else { !(CASTLE_BK | CASTLE_BQ) };
    }
    for &(s, mask) in &[(0u8, CASTLE_WQ), (7u8, CASTLE_WK), (56u8, CASTLE_BQ), (63u8, CASTLE_BK)] {
        if mv.from == s || mv.to == s { np.castling &= !mask; }
    }

    np.side = them;
    if us == BLACK { np.fullmove += 1; }
    np.recompute_occ();
    np.hash = np.compute_hash();
    np
}

pub fn gen_legal(pos: &Position) -> Vec<Move> {
    let us = pos.side;
    gen_pseudo_legal(pos)
        .into_iter()
        .filter(|mv| {
            let np = make_move(pos, mv);
            !in_check(&np, us)
        })
        .collect()
}

/// Parse a UCI move string (`"e2e4"`, `"e7e8q"`) against the legal moves in
/// `pos`, matching on from/to/promotion (castling's from/to is the king's
/// own move, so no separate castle-notation case is needed).
pub fn parse_uci_move(pos: &Position, s: &str) -> Option<Move> {
    if s.len() < 4 { return None; }
    let from = parse_sq(&s[0..2])?;
    let to = parse_sq(&s[2..4])?;
    let promo = match s.as_bytes().get(4) {
        Some(b'q') => PROMO_Q,
        Some(b'r') => PROMO_R,
        Some(b'b') => PROMO_B,
        Some(b'n') => PROMO_N,
        _ => PROMO_NONE,
    };
    gen_legal(pos).into_iter().find(|m| m.from == from && m.to == to && m.promo == promo)
}
