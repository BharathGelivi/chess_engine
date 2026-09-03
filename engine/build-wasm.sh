#!/usr/bin/env bash
# Rebuilds the WASM engine and fixes up wasm-pack's output in place. Run
# from anywhere; cds into engine/ itself. Three gotchas this papers over,
# each bitten this project before (see CLAUDE.md):
#   1. Threading needs nightly + build-std (stable's std has no atomics) —
#      target-feature/link-arg flags live in engine/.cargo/config.toml,
#      scoped to wasm32-unknown-unknown only.
#   2. wasm-pack regenerates a `pkg/.gitignore` containing just `*` every
#      build, which would silently exclude the whole engine from git (this
#      is exactly what 404'd the engine in prod once already).
#   3. wasm-bindgen-rayon's generated workerHelpers.js does a bundler-style
#      directory import (`import('../../..')`) to reach engine.js, which
#      only resolves under a bundler's package resolution — this app loads
#      pkg/ as raw static files (no bundler), so it 404s in a real browser.
#      Patch it to the explicit file path.
#   4. NOT papered over by this script, but the reason the output dir below
#      is `engine-v2/` and not `engine/`: this directory is served with a
#      1-year `immutable` Cache-Control (vercel.json), so any browser that
#      already fetched it under the old path/headers is stuck until the URL
#      itself changes — a plain page refresh does not reliably bust a
#      cached Worker's own sub-resource fetches. If engine/'s response
#      headers change again in the future (not just its content), bump this
#      to engine-v3 and update every reference (App.tsx's makeEngineWorker,
#      this script's --out-dir, vercel.json's matching header block).
set -euo pipefail
cd "$(dirname "$0")"

RUSTUP_TOOLCHAIN=nightly wasm-pack build . --target web --out-dir ../public/engine-v2/pkg -- -Z build-std=panic_abort,std

rm -f ../public/engine-v2/pkg/.gitignore

for f in ../public/engine-v2/pkg/snippets/wasm-bindgen-rayon-*/src/workerHelpers.js; do
  sed -i "s#await import('\.\./\.\./\.\.')#await import('../../../engine.js')#" "$f"
done

echo "WASM engine rebuilt at public/engine-v2/pkg — remember to test the 'Overboard Engine' picker in a browser before committing."
