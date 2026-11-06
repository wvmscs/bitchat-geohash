#!/bin/sh
# SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
# SPDX-License-Identifier: GPL-3.0-or-later

# Builds the map's site for the browser (no server): the library in
# WebAssembly, its JavaScript bindings, the page's files (the same as the
# server's, plus shim.js and worker.js). Needs the wasm32-unknown-unknown
# target and the `wasm-bindgen` command of the version in Cargo.toml.
#
#   web/build.sh [out-dir]       # default target/web
#
# Serve the directory at the root of a site (the page asks for /assets/...).

set -eu
cd "$(dirname "$0")/.."
out=${1:-target/web}

cargo rustc --lib --release --locked --target wasm32-unknown-unknown \
    --no-default-features --crate-type cdylib

rm -rf "$out"
mkdir -p "$out/assets/pkg"
wasm-bindgen --target web --no-typescript --out-dir "$out/assets/pkg" \
    target/wasm32-unknown-unknown/release/bitchat_geohash.wasm
cp -R src/server/assets/. "$out/assets/"
rm "$out/assets/index.html"
cp web/shim.js web/worker.js "$out/assets/"
# The server's page, with shim.js before app.js.
awk '/src="\/assets\/app.js"/ { print "<script src=\"/assets/shim.js\"></script>" } { print }' \
    src/server/assets/index.html > "$out/index.html"
grep -q 'assets/shim.js' "$out/index.html"
echo "map site in $out"
