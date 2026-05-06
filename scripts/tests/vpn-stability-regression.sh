#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

FAKE_BIN="$TMP_DIR/bin"
OUT_DIR="$TMP_DIR/out"
mkdir -p "$FAKE_BIN" "$OUT_DIR"

cat >"$FAKE_BIN/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

url="${@: -1}"

if [[ "$url" == "http://keepalive.test/" ]]; then
  printf '204'
  exit 0
fi

sleep_secs="1.2"
case "${FAKE_CURL_MODE:-}" in
  race)
    delay_slot=$(( (BASHPID % 5) + 1 ))
    sleep_secs="1.${delay_slot}"
    ;;
esac

sleep "$sleep_secs"
printf '302\t1.500000\t0.001000\t'
exit 0
EOF
chmod +x "$FAKE_BIN/curl"

cat >"$FAKE_BIN/timeout" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -lt 2 ]]; then
  echo "fake timeout: missing command" >&2
  exit 1
fi

shift
"$@"
EOF
chmod +x "$FAKE_BIN/timeout"

export PATH="$FAKE_BIN:$PATH"

run_case() {
  local mode="$1"
  local case_dir="$OUT_DIR/$mode"
  mkdir -p "$case_dir"
  (
    cd "$ROOT_DIR"
    FAKE_CURL_MODE="$mode" \
    KEEPALIVE_URL="http://keepalive.test/" \
    KEEPALIVE_INT=60 \
    DURATION=1 \
    CONCURRENCY=5 \
    RAMP_UP=0 \
    CONNECT_TIMEOUT=5 \
    MAX_TIME=5 \
    OUT_DIR="$case_dir" \
    scripts/vpn-stability.sh https://target.test/
  )
}

assert_contains() {
  local file="$1"
  local pattern="$2"
  if ! grep -Fq "$pattern" "$file"; then
    echo "expected '$pattern' in $file" >&2
    cat "$file" >&2
    exit 1
  fi
}

extract_count() {
  local file="$1"
  local key="$2"
  awk -v wanted="$key" '$1 == wanted { print $2 }' "$file"
}

run_case normal
NORMAL_SUMMARY="$OUT_DIR/normal/summary.txt"
assert_contains "$NORMAL_SUMMARY" "p50=1.50s"
assert_contains "$NORMAL_SUMMARY" "p90=1.50s"
assert_contains "$NORMAL_SUMMARY" "p95=1.50s"
assert_contains "$NORMAL_SUMMARY" "p99=1.50s"

run_case race
sleep 2
RACE_RESULTS="$OUT_DIR/race/results.tsv"
RACE_SUMMARY="$OUT_DIR/race/summary.txt"
results_ok=$(awk -F'\t' 'NR > 1 && $5 == 302 { n++ } END { print n + 0 }' "$RACE_RESULTS")
summary_ok=$(extract_count "$RACE_SUMMARY" "302")

if [[ "$results_ok" != "$summary_ok" ]]; then
  echo "summary mismatch: results_ok=$results_ok summary_ok=${summary_ok:-missing}" >&2
  echo "--- summary ---" >&2
  cat "$RACE_SUMMARY" >&2
  echo "--- results ---" >&2
  cat "$RACE_RESULTS" >&2
  exit 1
fi

echo "vpn-stability regression test passed"
