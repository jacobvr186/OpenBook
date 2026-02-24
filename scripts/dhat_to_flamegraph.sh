#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "Usage: $0 <dhat-heap.json> <output.svg> [metric]"
  echo "  metric: gb (default), mb, eb, tb"
  exit 1
fi

INPUT_JSON="$1"
OUTPUT_SVG="$2"
METRIC="${3:-gb}"

case "$METRIC" in
  gb|mb|eb|tb) ;;
  *)
    echo "Invalid metric '$METRIC'. Use one of: gb, mb, eb, tb"
    exit 1
    ;;
esac

if [[ ! -f "$INPUT_JSON" ]]; then
  echo "Input file not found: $INPUT_JSON"
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "Missing dependency: jq"
  exit 1
fi

if ! command -v inferno-flamegraph >/dev/null 2>&1; then
  echo "Missing dependency: inferno-flamegraph"
  echo "Install with: cargo install inferno"
  exit 1
fi

tmp_folded="$(mktemp -t dhat_folded.XXXXXX)"
trap 'rm -f "$tmp_folded"' EXIT

jq -r --arg metric "$METRIC" '
  . as $root
  | $root.pps[]
  | . as $pp
  | (
      if $metric == "gb" then ($pp.gb // 0)
      elif $metric == "mb" then ($pp.mb // 0)
      elif $metric == "eb" then ($pp.eb // 0)
      else ($pp.tb // 0)
      end
    ) as $bytes
  | select($bytes > 0)
  | (
      if (($pp.fs | length) > 0)
      then ([ $pp.fs[] | $root.ftbl[.] | gsub(";"; ",") ] | reverse | join(";"))
      else "[unknown]"
      end
    ) + " " + ($bytes | floor | tostring)
' "$INPUT_JSON" >"$tmp_folded"

inferno-flamegraph \
  --countname="bytes" \
  --title="DHAT heap flamegraph ($METRIC)" \
  "$tmp_folded" >"$OUTPUT_SVG"

echo "Wrote flame graph: $OUTPUT_SVG"
