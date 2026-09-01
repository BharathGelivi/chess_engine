// Native binary: fast local iteration for perft/search benchmarking and a
// stdin/stdout UCI-lite loop, without going through WASM.
//
// Usage:
//   cargo run --release              -> runs perft correctness check + search benchmark
//   cargo run --release -- uci       -> UCI-lite loop over stdin/stdout
//   cargo run --release -- perft N [fen]

use engine::board::Position;
use engine::perft::perft;
use engine::search::{Search, SearchLimits, SearchOptions};
use engine::uci::Engine;
use std::io::{self, BufRead};
use std::time::Instant;

const KIWIPETE: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";

fn run_uci_loop() {
    let mut engine = Engine::new();
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line { Ok(l) => l, Err(_) => break };
        if line.trim() == "quit" { break; }
        engine.handle_line(&line, &mut |s| println!("{s}"));
    }
}

fn run_perft_check() {
    println!("=== Perft correctness (vs. Chess Programming Wiki published values) ===");
    let start = Position::startpos();
    // https://www.chessprogramming.org/Perft_Results  Position 1 (startpos)
    let expected_start: [u64; 6] = [1, 20, 400, 8_902, 197_281, 4_865_609];
    for (depth, &exp) in expected_start.iter().enumerate() {
        let t0 = Instant::now();
        let n = perft(&start, depth as u32);
        let dt = t0.elapsed();
        let nps = if dt.as_secs_f64() > 0.0 { (n as f64 / dt.as_secs_f64()) as u64 } else { 0 };
        println!("startpos depth {depth}: got {n}, expected {exp}, match={} ({:?}, {nps} nps)", n == exp, dt);
    }

    let kiwi = Position::from_fen(KIWIPETE).expect("kiwipete fen");
    // Chess Programming Wiki Position 2 ("Kiwipete")
    let expected_kiwi: [u64; 5] = [1, 48, 2_039, 97_862, 4_085_603];
    for (depth, &exp) in expected_kiwi.iter().enumerate() {
        let t0 = Instant::now();
        let n = perft(&kiwi, depth as u32);
        let dt = t0.elapsed();
        let nps = if dt.as_secs_f64() > 0.0 { (n as f64 / dt.as_secs_f64()) as u64 } else { 0 };
        println!("kiwipete depth {depth}: got {n}, expected {exp}, match={} ({:?}, {nps} nps)", n == exp, dt);
    }
}

fn bench_search(label: &str, opts: SearchOptions, depth: u32) {
    let pos = Position::startpos();
    let mut search = Search::new(opts);
    let limits = SearchLimits { depth: Some(depth), movetime_ms: None };
    let t0 = Instant::now();
    let mut last_nodes = 1u64;
    let mut last_depth = 0u32;
    search.iterative_deepening(&pos, &limits, |info| {
        last_nodes = info.nodes;
        last_depth = info.depth;
    });
    let dt = t0.elapsed();
    let nps = if dt.as_secs_f64() > 0.0 { (last_nodes as f64 / dt.as_secs_f64()) as u64 } else { 0 };
    let ebf = if last_depth > 0 { (last_nodes as f64).powf(1.0 / last_depth as f64) } else { 0.0 };
    println!(
        "{label}: depth {last_depth}, nodes {last_nodes}, time {dt:?}, nps {nps}, effective branching factor {ebf:.3}"
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "uci" {
        run_uci_loop();
        return;
    }
    if args.len() > 1 && args[1] == "perft" {
        let depth: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
        let pos = match args.get(3) {
            Some(fen) => Position::from_fen(fen).expect("bad fen"),
            None => Position::startpos(),
        };
        let t0 = Instant::now();
        let n = perft(&pos, depth);
        println!("perft({depth}) = {n}  ({:?})", t0.elapsed());
        return;
    }

    run_perft_check();

    println!("\n=== Search benchmark: minimal (alpha-beta + ID only) vs full (Phase 3+4) ===");
    bench_search("minimal", SearchOptions::minimal(), 6);
    bench_search("phase3_only (TT+ordering+quiescence+PVS)", SearchOptions::phase3_only(), 6);
    bench_search("full (+ null-move + LMR + aspiration)", SearchOptions::default(), 6);
}
