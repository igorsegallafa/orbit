#!/usr/bin/env bash
# Memory of a running Orbit (Linux): the app, its WebKit processes and what
# it started (language servers, terminals, agents), one line each, plus the
# total. PSS splits shared libraries fairly between processes, so the total
# is what Orbit really costs; RSS would count WebKit's libraries many times.
#   scripts/memory.sh            the first Orbit found
#   scripts/memory.sh <pid>      a given Orbit process
set -euo pipefail

root=${1:-$(pgrep -x Orbit | head -1 || true)}
[[ -n "$root" ]] || { echo "Orbit isn't running"; exit 1; }

tree() {
  echo "$1"
  for c in $(pgrep -P "$1" || true); do tree "$c"; done
}

total=0
printf "%-8s %-28s %9s %9s\n" PID PROCESS "PSS MB" "ANON MB"
for p in $(tree "$root"); do
  [[ -r /proc/$p/smaps_rollup ]] || continue
  # Short-lived children (git, a shell command) can be gone by now.
  read -r pss anon < <(awk '/^Pss:/{p=$2}/^Anonymous:/{a=$2}END{print int(p/1024), int(a/1024)}' "/proc/$p/smaps_rollup" 2>/dev/null || echo "0 0")
  (( pss > 0 )) || continue
  name=$(ps -o args= -p "$p" | awk '{n=$1; sub(".*/","",n); print n}')
  printf "%-8s %-28s %9d %9d\n" "$p" "${name:0:28}" "$pss" "$anon"
  total=$((total + pss))
done
printf "%-37s %9d\n" "Total" "$total"
