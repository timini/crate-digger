#!/usr/bin/env bash
# Encode a synthetic song with lossy codecs for the identity calibration
# test (crates/core/tests/identity_calibration.rs). Everything else the
# test needs is generated in memory. Requires ffmpeg with libmp3lame.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/crates/audio/tests/fixtures/identity"
mkdir -p "$out"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cargo run -q --release -p cd-audio --example synth_wav -- "$tmp/song1.wav" 1 30

ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }
ff -i "$tmp/song1.wav" -c:a libmp3lame -b:a 128k "$out/song1-128.mp3"
ff -i "$tmp/song1.wav" -c:a libmp3lame -b:a 64k -ac 1 "$out/song1-64-mono.mp3"
ff -i "$tmp/song1.wav" -c:a aac -b:a 96k "$out/song1-96.m4a"
ls -l "$out"
