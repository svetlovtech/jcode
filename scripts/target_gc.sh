#!/usr/bin/env bash
# Fork: reclaim disk space from cargo target/ orphans.
#
# Cargo never deletes artifacts whose fingerprint is gone (old feature sets,
# renamed crates, edited test binaries). On this workspace those orphans grow
# to ~10 GB per heavy week: stale `lib*.rlib`/`lib*.rmeta` stay in
# target/<profile>/deps even though cargo cannot reuse them.
#
# This script removes exactly the orphans: artifacts whose fingerprint dir is
# missing under target/<profile>/.fingerprint. Cargo rebuilds them on demand.
# Hot artifacts (fingerprints intact) are never touched, so incremental
# builds stay fast after a cleanup.
#
# Usage:
#   scripts/target_gc.sh [--dry-run]     # default: clean debug + selfdev
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

dry_run=0
profiles=(debug selfdev)
args=()
for arg in "$@"; do
  case "$arg" in
    --dry-run) dry_run=1 ;;
    *) args+=("$arg") ;;
  esac
done
if [ ${#args[@]} -gt 0 ]; then profiles=("${args[@]}"); fi

total_freed=0

for profile in "${profiles[@]}"; do
  fp_root="target/$profile/.fingerprint"
  deps="target/$profile/deps"
  [ -d "$deps" ] || continue

  removed=0
  bytes=0

  while IFS= read -r -d '' f; do
    name=$(basename "$f")
    base="${name#lib}"; base="${base%.rlib}"; base="${base%.rmeta}"
    stem="${base%-*}"; hash="${base##*-}"
    if [ ! -d "$fp_root/$stem-$hash" ]; then
      size=$(stat -c %s "$f")
      if [ "$dry_run" -eq 1 ]; then
        echo "[dry-run] would remove $f ($((size / 1048576)) MB)"
      else
        rm -f "$f"
      fi
      removed=$((removed + 1)); bytes=$((bytes + size))
    fi
  done < <(find "$deps" -maxdepth 1 -type f \( -name 'lib*.rlib' -o -name 'lib*.rmeta' \) -print0)

  # Orphaned dep-info files.
  while IFS= read -r -d '' f; do
    name=$(basename "$f" .d)
    base="${name#lib}"
    stem="${base%-*}"; hash="${base##*-}"
    if [ ! -d "$fp_root/$stem-$hash" ]; then
      if [ "$dry_run" -eq 1 ]; then
        echo "[dry-run] would remove $f"
      else
        rm -f "$f"
      fi
      removed=$((removed + 1))
    fi
  done < <(find "$deps" -maxdepth 1 -type f -name '*.d' -print0)

  echo "$profile: $removed orphans, $((bytes / 1048576)) MB"
  total_freed=$((total_freed + bytes))
done

echo "total: $((total_freed / 1048576)) MB $([ "$dry_run" -eq 1 ] && echo '(dry-run)')"
