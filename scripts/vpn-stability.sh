#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/vpn-stability.sh [URL ...]

Test smelly-connect VPN stability under sustained load.

Environment variables:
  PROXY_URL       HTTP proxy address. Default: http://127.0.0.1:8080
  DURATION        Test duration in seconds. Default: 300 (5 min)
  CONCURRENCY     Concurrent curl workers. Default: 10
  RAMP_UP         Seconds to ramp up to full concurrency. Default: 30
  CONNECT_TIMEOUT curl --connect-timeout seconds. Default: 5
  MAX_TIME        curl --max-time seconds. Default: 15
  OUT_DIR         Output directory. Default: ./tmp/vpn-stability
  CURL_INSECURE   Set to 1 to add -k. Default: 1
  HEAD_REQUEST    Set to 1 to use -I (HEAD). Default: 0
  KEEPALIVE_URL   URL for keepalive pings. Default: http://www.baidu.com
  KEEPALIVE_INT   Keepalive interval in seconds. Default: 30
  MANAGEMENT_URL  Management API URL. Default: http://127.0.0.1:9090

Examples:
  scripts/vpn-stability.sh https://jwxt.sit.edu.cn/
  DURATION=600 CONCURRENCY=20 scripts/vpn-stress.sh https://jwxt.sit.edu.cn/
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

PROXY_URL="${PROXY_URL:-http://127.0.0.1:8080}"
DURATION="${DURATION:-300}"
CONCURRENCY="${CONCURRENCY:-10}"
RAMP_UP="${RAMP_UP:-30}"
CONNECT_TIMEOUT="${CONNECT_TIMEOUT:-5}"
MAX_TIME="${MAX_TIME:-15}"
OUT_DIR="${OUT_DIR:-./tmp/vpn-stability}"
CURL_INSECURE="${CURL_INSECURE:-1}"
HEAD_REQUEST="${HEAD_REQUEST:-0}"
KEEPALIVE_URL="${KEEPALIVE_URL:-http://www.baidu.com}"
KEEPALIVE_INT="${KEEPALIVE_INT:-30}"
MANAGEMENT_URL="${MANAGEMENT_URL:-http://127.0.0.1:9090}"

mkdir -p "$OUT_DIR"
RESULTS_FILE="$OUT_DIR/results.tsv"
SUMMARY_FILE="$OUT_DIR/summary.txt"
HEALTH_FILE="$OUT_DIR/health.tsv"
LOG_FILE="$OUT_DIR/test.log"

if [[ "$#" -eq 0 ]]; then
  URLS=("https://jwxt.sit.edu.cn/")
else
  URLS=("$@")
fi

log() { printf "[%s] %s\n" "$(date '+%H:%M:%S')" "$*" | tee -a "$LOG_FILE"; }

# ── results header ──
printf "ts\treq_id\turl\tcurl_exit\thttp_code\ttime_total\ttime_connect\terror\n" > "$RESULTS_FILE"
printf "ts\tstatus\tlatency_ms\n" > "$HEALTH_FILE"

# ── single request ──
run_one() {
  local request_id="$1"
  local url="$2"
  local ts
  ts="$(date +%s%3N)"
  local curl_exit=0 http_code="000" time_total="0" time_connect="0" error="-"
  local -a curl_args
  local tmp_out tmp_err

  tmp_out="$(mktemp)"
  tmp_err="$(mktemp)"
  curl_args=(
    --proxy "$PROXY_URL"
    --silent --show-error
    --output /dev/null
    --write-out '%{http_code}\t%{time_total}\t%{time_connect}\t%{errormsg}'
    --connect-timeout "$CONNECT_TIMEOUT"
    --max-time "$MAX_TIME"
  )
  [[ "$CURL_INSECURE" == "1" ]] && curl_args+=(-k)
  [[ "$HEAD_REQUEST" == "1" ]] && curl_args+=(-I)

  if curl "${curl_args[@]}" "$url" >"$tmp_out" 2>"$tmp_err"; then
    curl_exit=0
  else
    curl_exit=$?
  fi

  IFS=$'\t' read -r http_code time_total time_connect error <"$tmp_out" || true
  [[ -s "$tmp_err" ]] && error="$(tr '\n' ' ' <"$tmp_err" | sed 's/[[:space:]]\+/ /g; s/^ //; s/ $//')"
  rm -f "$tmp_out" "$tmp_err"

  printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n" \
    "$ts" "$request_id" "$url" "$curl_exit" "$http_code" "$time_total" "$time_connect" "${error:--}" \
    >> "$RESULTS_FILE"
}

export PROXY_URL CONNECT_TIMEOUT MAX_TIME CURL_INSECURE HEAD_REQUEST RESULTS_FILE
export -f run_one

# ── keepalive ping ──
keepalive_loop() {
  local count=0 fail=0
  while true; do
    count=$((count + 1))
    local ts start_ms end_ms lat status
    ts="$(date +%s%3N)"
    start_ms="$(date +%s%3N)"
    if curl --proxy "$PROXY_URL" -s -o /dev/null -w '%{http_code}' \
         --connect-timeout 3 --max-time 5 "$KEEPALIVE_URL" >/dev/null 2>&1; then
      status="ok"
    else
      status="fail"
      fail=$((fail + 1))
    fi
    end_ms="$(date +%s%3N)"
    lat=$(( end_ms - start_ms ))
    printf "%s\t%s\t%s\n" "$ts" "$status" "$lat" >> "$HEALTH_FILE"
    sleep "$KEEPALIVE_INT"
  done
}

# ── ramp-up helper: returns current concurrency for this second ──
current_concurrency() {
  local elapsed="$1"
  if (( elapsed >= RAMP_UP )); then
    echo "$CONCURRENCY"
  else
    # linear ramp
    echo $(( 1 + (CONCURRENCY - 1) * elapsed / RAMP_UP ))
  fi
}

# ── main load loop ──
log "=== VPN Stability Test ==="
log "proxy=$PROXY_URL  duration=${DURATION}s  concurrency=$CONCURRENCY  ramp_up=${RAMP_UP}s"
log "targets: ${URLS[*]}"
log "results=$RESULTS_FILE"
log ""

# start keepalive in background
keepalive_loop &
KEEPALIVE_PID=$!

START_TS="$(date +%s)"
req_id=0
in_flight=0
last_report=$START_TS

while true; do
  now="$(date +%s)"
  elapsed=$(( now - START_TS ))
  if (( elapsed >= DURATION )); then
    break
  fi

  # periodic progress report every 30s
  if (( now - last_report >= 30 )); then
    total_reqs=$((req_id))
    ok_reqs=$(awk -F'\t' 'NR>1 && $5>=200 && $5<400 {n++} END{print n+0}' "$RESULTS_FILE")
    fail_reqs=$(awk -F'\t' 'NR>1 && ($5<200 || $5>=400) && $5!="000" {n++} END{print n+0}' "$RESULTS_FILE")
    timeout_reqs=$(awk -F'\t' 'NR>1 && $4!=0 {n++} END{print n+0}' "$RESULTS_FILE")
    log "  [${elapsed}s/${DURATION}s] reqs=$total_reqs ok=$ok_reqs fail=$fail_reqs timeout=$timeout_reqs in_flight=$in_flight"
    last_report=$now
  fi

  # determine target concurrency for this moment
  want=$(current_concurrency "$elapsed")

  # spawn requests up to desired concurrency
  while (( in_flight < want )); do
    req_id=$((req_id + 1))
    url="${URLS[$(( (req_id - 1) % ${#URLS[@]} ))]}"
    run_one "$req_id" "$url" &
    in_flight=$((in_flight + 1))
  done

  # reap one finished worker
  wait -n 2>/dev/null || true
  in_flight=$((in_flight - 1))
done

log "Duration reached, waiting for in-flight requests..."
wait 2>/dev/null || true
kill "$KEEPALIVE_PID" 2>/dev/null || true
wait "$KEEPALIVE_PID" 2>/dev/null || true

# ── summary ──
log ""
log "=== Results ==="

{
  echo "proxy_url=$PROXY_URL"
  echo "duration=${DURATION}s"
  echo "concurrency=$CONCURRENCY"
  echo "ramp_up=${RAMP_UP}s"
  echo "targets=${URLS[*]}"
  echo "total_requests=$req_id"
  echo ""

  echo "[http_code_distribution]"
  awk -F'\t' 'NR>1 { count[$5]++ } END { for (c in count) printf "  %s\t%s\n", c, count[c] }' \
    "$RESULTS_FILE" | sort
  echo ""

  echo "[curl_exit_distribution]"
  awk -F'\t' 'NR>1 { count[$4]++ } END { for (c in count) printf "  %s\t%s\n", c, count[c] }' \
    "$RESULTS_FILE" | sort -n
  echo ""

  echo "[latency_percentiles]"
  awk -F'\t' 'NR>1 && $7+0 > 0 { print $7+0 }' "$RESULTS_FILE" | sort -n | {
    readarray -t vals
    n=${#vals[@]}
    if (( n > 0 )); then
      p50=${vals[$(( n * 50 / 100 ))]}
      p90=${vals[$(( n * 90 / 100 ))]}
      p95=${vals[$(( n * 95 / 100 ))]}
      p99=${vals[$(( n * 99 / 100 ))]}
      printf "  p50=%.3fs  p90=%.3fs  p95=%.3fs  p99=%.3fs  (n=%d)\n" "$p50" "$p90" "$p95" "$p99" "$n"
    else
      echo "  no data"
    fi
  }
  echo ""

  echo "[error_summary]"
  awk -F'\t' 'NR>1 && $4!=0 { count[$8]++ } END { for (e in count) printf "  %d\t%s\n", count[e], e }' \
    "$RESULTS_FILE" | sort -rn | head -10
  echo ""

  echo "[slowest_10]"
  awk -F'\t' 'NR>1 { print $7 "\t" $0 }' "$RESULTS_FILE" | sort -rn | head -10 | cut -f2-
  echo ""

  echo "[keepalive_health]"
  total_health=$(awk 'END{print NR-1}' "$HEALTH_FILE")
  ok_health=$(awk -F'\t' 'NR>1 && $2=="ok" {n++} END{print n+0}' "$HEALTH_FILE")
  fail_health=$(awk -F'\t' 'NR>1 && $2=="fail" {n++} END{print n+0}' "$HEALTH_FILE")
  echo "  total=$total_health  ok=$ok_health  fail=$fail_health"
  if (( total_health > 0 )); then
    rate=$(awk "BEGIN{printf \"%.1f\", $ok_health / $total_health * 100}")
    echo "  success_rate=${rate}%"
  fi

} | tee "$SUMMARY_FILE"

log ""
log "results_file=$RESULTS_FILE"
log "summary_file=$SUMMARY_FILE"
log "health_file=$HEALTH_FILE"
log "Done."
