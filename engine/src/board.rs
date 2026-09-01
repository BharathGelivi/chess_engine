// Bitboard position representation, FEN parsing, Zobrist hashing.
//
// Square index: 0 = a1, 7 = h1, 8 = a2, ... 63 = h8 (little-endian rank-file).
// Copy-make (not make/unmake): Position is a small Copy struct, so move-making
// clones it rather than mutating + undoing. Simpler and far less bug-prone
// than incremental unmake for a solo project; costs some speed.
// ponytail: copy-make instead of incremental unmake — revisit with unmake-move
// if perft/search NPS profiling shows position-copy cost is the bottleneck.

pub const WHITE: usize = 0;
pub const BLACK: usize = 1;

pub const PAWN: usize = 0;
pub const KNIGHT: usize = 1;
pub const BISHOP: usize = 2;
pub const ROOK: usize = 3;
pub const QUEEN: usize = 4;
pub const KING: usize = 5;

pub const CASTLE_WK: u8 = 1;
pub const CASTLE_WQ: u8 = 2;
pub const CASTLE_BK: u8 = 4;
pub const CASTLE_BQ: u8 = 8;

pub const FILE_A: u64 = 0x0101010101010101;
pub const FILE_H: u64 = 0x8080808080808080;
pub const RANK_1: u64 = 0x00000000000000FF;
pub const RANK_2: u64 = 0x000000000000FF00;
pub const RANK_4: u64 = 0x00000000FF000000;
pub const RANK_5: u64 = 0x000000FF00000000;
pub const RANK_7: u64 = 0x00FF000000000000;
pub const RANK_8: u64 = 0xFF00000000000000;

#[inline]
pub fn sq(file: u8, rank: u8) -> u8 { rank * 8 + file }
#[inline]
pub fn file_of(s: u8) -> u8 { s % 8 }
#[inline]
pub fn rank_of(s: u8) -> u8 { s / 8 }

pub fn sq_name(s: u8) -> String {
    let f = (b'a' + file_of(s)) as char;
    let r = (b'1' + rank_of(s)) as char;
    format!("{f}{r}")
}

pub fn parse_sq(name: &str) -> Option<u8> {
    let b = name.as_bytes();
    if b.len() < 2 { return None; }
    let file = b[0].wrapping_sub(b'a');
    let rank = b[1].wrapping_sub(b'1');
    if file > 7 || rank > 7 { return None; }
    Some(sq(file, rank))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Move {
    pub from: u8,
    pub to: u8,
    pub promo: u8, // 0 none, 1 N, 2 B, 3 R, 4 Q
    pub flag: u8,  // 0 normal, 1 double pawn push, 2 en passant capture, 3 castle K-side, 4 castle Q-side
}

pub const PROMO_NONE: u8 = 0;
pub const PROMO_N: u8 = 1;
pub const PROMO_B: u8 = 2;
pub const PROMO_R: u8 = 3;
pub const PROMO_Q: u8 = 4;

pub const FLAG_NORMAL: u8 = 0;
pub const FLAG_DOUBLE: u8 = 1;
pub const FLAG_EP: u8 = 2;
pub const FLAG_CASTLE_K: u8 = 3;
pub const FLAG_CASTLE_Q: u8 = 4;

impl Move {
    pub fn to_uci(&self) -> String {
        let promo = match self.promo {
            PROMO_N => "n",
            PROMO_B => "b",
            PROMO_R => "r",
            PROMO_Q => "q",
            _ => "",
        };
        format!("{}{}{}", sq_name(self.from), sq_name(self.to), promo)
    }
}

#[derive(Clone, Copy)]
pub struct Position {
    pub pieces: [[u64; 6]; 2],
    pub occ: [u64; 2],
    pub all_occ: u64,
    pub side: usize,
    pub castling: u8,
    pub ep_square: Option<u8>,
    pub halfmove: u16,
    pub fullmove: u16,
    pub hash: u64,
}

pub struct Zobrist {
    pub piece_sq: [[[u64; 64]; 6]; 2],
    pub side: u64,
    pub castling: [u64; 16],
    pub ep_file: [u64; 8],
}

fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

impl Zobrist {
    pub fn new() -> Self {
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let mut piece_sq = [[[0u64; 64]; 6]; 2];
        for c in 0..2 {
            for p in 0..6 {
                for s in 0..64 {
                    piece_sq[c][p][s] = xorshift64(&mut state);
                }
            }
        }
        let side = xorshift64(&mut state);
        let mut castling = [0u64; 16];
        for c in castling.iter_mut() { *c = xorshift64(&mut state); }
        let mut ep_file = [0u64; 8];
        for e in ep_file.iter_mut() { *e = xorshift64(&mut state); }
        Zobrist { piece_sq, side, castling, ep_file }
    }
}

use std::sync::OnceLock;
static ZOBRIST: OnceLock<Zobrist> = OnceLock::new();
pub fn zobrist() -> &'static Zobrist {
    ZOBRIST.get_or_init(Zobrist::new)
}

impl Position {
    pub fn compute_hash(&self) -> u64 {
        let z = zobrist();
        let mut h = 0u64;
        for c in 0..2 {
            for p in 0..6 {
                let mut bb = self.pieces[c][p];
                while bb != 0 {
                    let s = bb.trailing_zeros() as usize;
                    h ^= z.piece_sq[c][p][s];
                    bb &= bb - 1;
                }
            }
        }
        if self.side == BLACK { h ^= z.side; }
        h ^= z.castling[(self.castling & 0xF) as usize];
        if let Some(epsq) = self.ep_square {
            h ^= z.ep_file[file_of(epsq) as usize];
        }
        h
    }

    pub fn startpos() -> Self {
        Self::from_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1").unwrap()
    }

    pub fn piece_at(&self, s: u8) -> Option<(usize, usize)> {
        let bit = 1u64 << s;
        for c in 0..2 {
            for p in 0..6 {
                if self.pieces[c][p] & bit != 0 { return Some((c, p)); }
            }
        }
        None
    }

    pub fn recompute_occ(&mut self) {
        self.occ[WHITE] = self.pieces[WHITE].iter().fold(0, |a, b| a | b);
        self.occ[BLACK] = self.pieces[BLACK].iter().fold(0, |a, b| a | b);
        self.all_occ = self.occ[WHITE] | self.occ[BLACK];
    }

    pub fn from_fen(fen: &str) -> Option<Self> {
        let parts: Vec<&str> = fen.split_whitespace().collect();
        if parts.len() < 4 { return None; }
        let mut pieces = [[0u64; 6]; 2];
        let ranks: Vec<&str> = parts[0].split('/').collect();
        if ranks.len() != 8 { return None; }
        for (i, rank_str) in ranks.iter().enumerate() {
            let rank = 7 - i as u8;
            let mut file = 0u8;
            for ch in rank_str.chars() {
                if let Some(d) = ch.to_digit(10) {
                    file += d as u8;
                } else {
                    let (color, ptype) = match ch {
                        'P' => (WHITE, PAWN), 'N' => (WHITE, KNIGHT), 'B' => (WHITE, BISHOP),
                        'R' => (WHITE, ROOK), 'Q' => (WHITE, QUEEN), 'K' => (WHITE, KING),
                        'p' => (BLACK, PAWN), 'n' => (BLACK, KNIGHT), 'b' => (BLACK, BISHOP),
                        'r' => (BLACK, ROOK), 'q' => (BLACK, QUEEN), 'k' => (BLACK, KING),
                        _ => return None,
                    };
                    let s = sq(file, rank);
                    pieces[color][ptype] |= 1u64 << s;
                    file += 1;
                }
            }
        }
        let side = if parts[1] == "w" { WHITE } else { BLACK };
        let mut castling = 0u8;
        if parts[2] != "-" {
            for ch in parts[2].chars() {
                match ch {
                    'K' => castling |= CASTLE_WK,
                    'Q' => castling |= CASTLE_WQ,
                    'k' => castling |= CASTLE_BK,
                    'q' => castling |= CASTLE_BQ,
                    _ => {}
                }
            }
        }
        let ep_square = if parts[3] == "-" { None } else { parse_sq(parts[3]) };
        let halfmove = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        let fullmove = parts.get(5).and_then(|s| s.parse().ok()).unwrap_or(1);
        let mut pos = Position {
            pieces, occ: [0; 2], all_occ: 0, side, castling, ep_square, halfmove, fullmove, hash: 0,
        };
        pos.recompute_occ();
        pos.hash = pos.compute_hash();
        Some(pos)
    }

    pub fn to_fen(&self) -> String {
        let mut out = String::new();
        for rank in (0..8u8).rev() {
            let mut empty = 0;
            for file in 0..8u8 {
                let s = sq(file, rank);
                match self.piece_at(s) {
                    None => empty += 1,
                    Some((c, p)) => {
                        if empty > 0 { out.push_str(&empty.to_string()); empty = 0; }
                        let ch = piece_char(c, p);
                        out.push(ch);
                    }
                }
            }
            if empty > 0 { out.push_str(&empty.to_string()); }
            if rank != 0 { out.push('/'); }
        }
        out.push(' ');
        out.push(if self.side == WHITE { 'w' } else { 'b' });
        out.push(' ');
        if self.castling == 0 {
            out.push('-');
        } else {
            if self.castling & CASTLE_WK != 0 { out.push('K'); }
            if self.castling & CASTLE_WQ != 0 { out.push('Q'); }
            if self.castling & CASTLE_BK != 0 { out.push('k'); }
            if self.castling & CASTLE_BQ != 0 { out.push('q'); }
        }
        out.push(' ');
        match self.ep_square {
            Some(s) => out.push_str(&sq_name(s)),
            None => out.push('-'),
        }
        out.push(' ');
        out.push_str(&self.halfmove.to_string());
        out.push(' ');
        out.push_str(&self.fullmove.to_string());
        out
    }
}

fn piece_char(color: usize, ptype: usize) -> char {
    let c = match ptype {
        PAWN => 'p', KNIGHT => 'n', BISHOP => 'b', ROOK => 'r', QUEEN => 'q', KING => 'k',
        _ => '?',
    };
    if color == WHITE { c.to_ascii_uppercase() } else { c }
}
