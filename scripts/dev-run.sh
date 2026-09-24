#!/bin/sh
# Cargo runner for macOS: signs the app binary with the local dev identity
# (see dev-signing-setup.sh) before running it. Other binaries, and
# machines without the identity, run unchanged.
bin="$1"
if [ "$(basename "$bin")" = "crate-digger" ] && security find-certificate -c "Crate Digger Dev" >/dev/null 2>&1; then
  codesign --force --sign "Crate Digger Dev" --identifier io.github.timini.cratedigger "$bin" >/dev/null 2>&1
  # codesign can report an error yet still sign, so check the result.
  codesign -dv "$bin" 2>&1 | grep -q "Identifier=io.github.timini.cratedigger" \
    || echo "dev-run: could not sign $bin; keychain prompts may repeat after rebuilds" >&2
fi
exec "$@"
