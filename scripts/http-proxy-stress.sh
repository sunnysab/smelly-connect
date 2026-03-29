#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/http-proxy-stress.sh [URL ...]

Environment variables:
  PROXY_URL       HTTP proxy address. Default: http://127.0.0.1:8080
  TOTAL           Total requests to send. Default: 40
  CONCURRENCY     Concurrent curl workers. Default: 4
  CONNECT_TIMEOUT curl --connect-timeout seconds. Default: 5
  MAX_TIME        curl --max-time seconds. Default: 15
  OUT_DIR         Output directory. Default: ./tmp/http-proxy-stress
  CURL_INSECURE   Set to 1 to add -k. Default: 1
  CURL_HEAD       Set to 1 to use -I. Default: 1

Examples:
  scripts/http-proxy-stress.sh https://jwxt.sit.edu.cn/ https://xg.sit.edu.cn/
  TOTAL=100 CONCURRENCY=10 scripts/http-proxy-stress.sh https://authserver.sit.edu.cn/
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

PROXY_URL="${PROXY_URL:-http://127.0.0.1:8080}"
TOTAL="${TOTAL:-40}"
CONCURRENCY="${CONCURRENCY:-4}"
CONNECT_TIMEOUT="${CONNECT_TIMEOUT:-5}"
MAX_TIME="${MAX_TIME:-15}"
OUT_DIR="${OUT_DIR:-./tmp/http-proxy-stress}"
CURL_INSECURE="${CURL_INSECURE:-1}"
CURL_HEAD="${CURL_HEAD:-1}"

mkdir -p "$OUT_DIR"
RESULTS_FILE="$OUT_DIR/results.tsv"
SUMMARY_FILE="$OUT_DIR/summary.txt"

if [[ "$#" -eq 0 ]]; then
  URLS=("https://jwxt.sit.edu.cn/")
else
  URLS=("$@")
fi

printf "request_id\turl\tcurl_exit\thttp_code\ttime_total\ttime_connect\ttime_appconnect\tremote_ip\terror\n" > "$RESULTS_FILE"

run_one() {
  local request_id="$1"
  local url="$2"
  local tmp_out
  local tmp_err
  local curl_exit=0
  local http_code="000"
  local time_total="0"
  local time_connect="0"
  local time_appconnect="0"
  local remote_ip="-"
  local error="-"
  local -a curl_args

  tmp_out="$(mktemp)"
  tmp_err="$(mktemp)"
  curl_args=(
    --proxy "$PROXY_URL"
    --silent
    --show-error
    --output /dev/null
    --write-out '%{http_code}\t%{time_total}\t%{time_connect}\t%{time_appconnect}\t%{remote_ip}\t%{errormsg}'
    --connect-timeout "$CONNECT_TIMEOUT"
    --max-time "$MAX_TIME"
  )

  if [[ "$CURL_INSECURE" == "1" ]]; then
    curl_args+=(-k)
  fi
  if [[ "$CURL_HEAD" == "1" ]]; then
    curl_args+=(-I)
  fi

  if curl "${curl_args[@]}" "$url" >"$tmp_out" 2>"$tmp_err"; then
    curl_exit=0
  else
    curl_exit=$?
  fi

  IFS=$'\t' read -r http_code time_total time_connect time_appconnect remote_ip error <"$tmp_out" || true
  if [[ -s "$tmp_err" ]]; then
    error="$(tr '\n' ' ' <"$tmp_err" | sed 's/[[:space:]]\+/ /g; s/^ //; s/ $//')"
  fi
  rm -f "$tmp_out"
  rm -f "$tmp_err"

  printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n" \
    "$request_id" "$url" "$curl_exit" "$http_code" "$time_total" "$time_connect" \
    "$time_appconnect" "${remote_ip:--}" "${error:--}" >> "$RESULTS_FILE"
}

export PROXY_URL CONNECT_TIMEOUT MAX_TIME CURL_INSECURE CURL_HEAD RESULTS_FILE
export -f run_one

in_flight=0
for request_id in $(seq 1 "$TOTAL"); do
  url="${URLS[$(((request_id - 1) % ${#URLS[@]}))]}"
  bash -lc 'run_one "$1" "$2"' _ "$request_id" "$url" &
  in_flight=$((in_flight + 1))
  if (( in_flight >= CONCURRENCY )); then
    wait -n
    in_flight=$((in_flight - 1))
  fi
done
wait

{
  echo "proxy_url=$PROXY_URL"
  echo "total=$TOTAL"
  echo "concurrency=$CONCURRENCY"
  echo "connect_timeout=$CONNECT_TIMEOUT"
  echo "max_time=$MAX_TIME"
  echo "targets=${URLS[*]}"
  echo
  echo "[http_code]"
  awk -F '\t' 'NR > 1 { count[$4]++ } END { for (code in count) printf "%s\t%s\n", code, count[code] }' \
    "$RESULTS_FILE" | sort
  echo
  echo "[curl_exit]"
  awk -F '\t' 'NR > 1 { count[$3]++ } END { for (code in count) printf "%s\t%s\n", code, count[code] }' \
    "$RESULTS_FILE" | sort -n
  echo
  echo "[slowest]"
  awk -F '\t' 'NR > 1 { print $5 "\t" $0 }' "$RESULTS_FILE" | sort -nr | head -n 10 | cut -f2-
} | tee "$SUMMARY_FILE"

echo
echo "results_file=$RESULTS_FILE"
echo "summary_file=$SUMMARY_FILE"
