#!/usr/bin/env bash
# Starts orbit-native with self-test arguments, waits, and prints the PSS of
# its process tree (PSS from /proc/<pid>/smaps_rollup plus SwapPss, summed over
# the app and its descendants), with language servers counted apart since they
# cost the same in any editor.
#   scripts/measure.sh <wait-seconds> [orbit-native args...]
set -euo pipefail
wait_s=$1; shift
bin=$(dirname "$0")/../../target/release/orbit-native
"$bin" "$@" --exit-after $((wait_s + 3)) >/dev/null 2>&1 &
root=$!
sleep "$wait_s"

tree() {
  echo "$1"
  for c in $(pgrep -P "$1" || true); do tree "$c"; done
}
app=0; others=0; servers=0
for p in $(tree "$root"); do
  [[ -r /proc/$p/smaps_rollup ]] || continue
  # Swapped-out pages leave PSS: count them back (SwapPss) so memory pressure doesn't flatter the numbers.
  pss=$(awk '/^Pss:/{p=$2}/^SwapPss:/{s=$2}END{print int((p+s)/1024)}' "/proc/$p/smaps_rollup" 2>/dev/null || echo 0)
  name=$(ps -o comm= -p "$p" || true)
  printf "  %-8s %-20s %6d MB\n" "$p" "$name" "$pss"
  if [[ $p == "$root" ]]; then app=$pss
  elif [[ $name =~ ^(clangd.*|rust-analyzer|typescript-lan.*|node)$ ]]; then servers=$((servers + pss))
  else others=$((others + pss)); fi
done
echo "app $app MB, app + shells $((app + others)) MB, language servers $servers MB"
wait "$root" || true
