#!/usr/bin/env bash
# Launch the release binary with a throwaway data directory and check it
# starts, opens its database and exits cleanly.
set -euo pipefail

target="${1:?usage: smoke-launch.sh <rust-target-triple> [profile]}"
profile="${2:-release}"
bin_dir="target/${target}/${profile}"
[[ -d "$bin_dir" ]] || bin_dir="target/${profile}"
case "$target" in
  *windows*) bin="${bin_dir}/crate-digger.exe" ;;
  *) bin="${bin_dir}/crate-digger" ;;
esac

data_dir="$(mktemp -d)"
export CRATE_DIGGER_DATA_DIR="$data_dir"
export CRATE_DIGGER_SMOKE=1

run=("$bin")
if command -v timeout >/dev/null; then
  run=(timeout 120 "${run[@]}")
elif command -v gtimeout >/dev/null; then
  run=(gtimeout 120 "${run[@]}")
fi
if [[ "$target" == *linux* ]]; then
  run=(xvfb-run -a "${run[@]}")
fi

output="$("${run[@]}" 2>&1)" || { echo "$output"; echo "app exited with an error"; exit 1; }
echo "$output"
grep -q "crate-digger smoke: started" <<<"$output"
test -f "$data_dir/crate-digger.sqlite"
echo "smoke test passed for $target"
