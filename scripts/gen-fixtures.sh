#!/usr/bin/env bash
# Regenerate the synthetic audio fixtures in crates/audio/tests/fixtures.
# The outputs are committed so tests never need ffmpeg or network access.
# Requires ffmpeg with libmp3lame; everything else uses native encoders.
set -euo pipefail

out="$(cd "$(dirname "$0")/.." && pwd)/crates/audio/tests/fixtures"
mkdir -p "$out"
cd "$out"

ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }

# Two seconds of a 440 Hz tone, mono, 44.1 kHz.
tone=(-f lavfi -i "sine=frequency=440:sample_rate=44100:duration=2")
tags=(
  -metadata artist="Fixture Collective"
  -metadata title="Night Signal (Extended Mix)"
  -metadata album="Test Pressings EP"
  -metadata track="3"
  -metadata date="2024"
  -metadata genre="House"
)

ff "${tone[@]}" -c:a pcm_s16le tone.wav
ff "${tone[@]}" -c:a pcm_s16be tone.aiff
ff "${tone[@]}" -c:a flac "${tags[@]}" -metadata LABEL="Test Pressings" -metadata BPM="124" -metadata INITIALKEY="8A" tone.flac
ff "${tone[@]}" -c:a libmp3lame -b:a 128k "${tags[@]}" -metadata publisher="Test Pressings" -metadata TBPM="124" -metadata TKEY="8A" tone.mp3
ff "${tone[@]}" -c:a libmp3lame -b:a 320k tone-320.mp3
ff "${tone[@]}" -c:a aac -b:a 96k "${tags[@]}" tone.m4a
ff "${tone[@]}" -c:a alac tone-alac.m4a
ff "${tone[@]}" -c:a vorbis -strict -2 -ac 2 tone.ogg

# Stereo, different pitch: a different "recording" for identity tests.
ff -f lavfi -i "sine=frequency=880:sample_rate=48000:duration=2" -ac 2 -c:a flac \
  -metadata artist="Sine Wave Society" -metadata title="Four Forty" -metadata album="Oscillations" \
  other-stereo.flac

# Unicode tags.
ff "${tone[@]}" -c:a flac -metadata artist="Róisín Mürphy" -metadata title="Überlicht (Dub)" unicode.flac

# Not audio: an MP3 extension over random bytes, and a truncated FLAC.
head -c 4096 /dev/urandom > corrupt.mp3
head -c 200 tone.flac > truncated.flac

ls -l "$out"
