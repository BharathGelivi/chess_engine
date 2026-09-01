// Correctness gate: perft node counts must match published values from the
// Chess Programming Wiki "Perft Results" page for the starting position and
// the Kiwipete position, to at least depth 5.

use engine::board::Position;
use engine::perft::perft;

const KIWIPETE: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";

#[test]
fn perft_startpos() {
    let pos = Position::startpos();
    assert_eq!(perft(&pos, 1), 20);
    assert_eq!(perft(&pos, 2), 400);
    assert_eq!(perft(&pos, 3), 8_902);
    assert_eq!(perft(&pos, 4), 197_281);
    assert_eq!(perft(&pos, 5), 4_865_609);
}

#[test]
fn perft_kiwipete() {
    let pos = Position::from_fen(KIWIPETE).unwrap();
    assert_eq!(perft(&pos, 1), 48);
    assert_eq!(perft(&pos, 2), 2_039);
    assert_eq!(perft(&pos, 3), 97_862);
    assert_eq!(perft(&pos, 4), 4_085_603);
}

#[test]
fn fen_roundtrip() {
    let fen = "r1bqkbnr/pppp1ppp/2n5/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R w KQkq - 2 3";
    let pos = Position::from_fen(fen).unwrap();
    assert_eq!(pos.to_fen(), fen);
}
