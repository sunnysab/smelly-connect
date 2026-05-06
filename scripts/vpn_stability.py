#!/usr/bin/env python3
from __future__ import annotations

import argparse
import asyncio
import math
import os
import sys
import time
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

import httpx

DEFAULT_URLS = ("https://xg.sit.edu.cn/",)
DEFAULT_PROXY_URL = "http://127.0.0.1:8080"
DEFAULT_KEEPALIVE_URL = "https://jwxt.sit.edu.cn/"
DEFAULT_OUT_ROOT = Path("./tmp/vpn-stability")
DEFAULT_REPORT_INTERVAL = 30.0


@dataclass(frozen=True)
class Settings:
    urls: tuple[str, ...]
    proxy_url: str | None
    duration: float
    concurrency: int
    ramp_up: float
    connect_timeout: float
    max_time: float
    out_dir: Path
    insecure: bool
    head_request: bool
    fresh_tcp_per_request: bool
    keepalive_url: str
    keepalive_interval: float
    report_interval: float


@dataclass(frozen=True)
class RequestResult:
    ts_ms: int
    request_id: int
    url: str
    exit_code: int
    http_code: int
    time_total: float
    time_connect: float
    error: str

    def to_tsv(self) -> str:
        return (
            f"{self.ts_ms}\t{self.request_id}\t{self.url}\t{self.exit_code}\t"
            f"{self.http_code:03d}\t{self.time_total:.6f}\t{self.time_connect:.6f}\t{self.error}\n"
        )


@dataclass(frozen=True)
class HealthResult:
    ts_ms: int
    status: str
    latency_ms: int

    def to_tsv(self) -> str:
        return f"{self.ts_ms}\t{self.status}\t{self.latency_ms}\n"


@dataclass(frozen=True)
class RunResult:
    out_dir: Path
    results_path: Path
    summary_path: Path
    health_path: Path
    log_path: Path
    total_requests: int
    ok_requests: int
    fail_requests: int
    timeout_requests: int
    network_error_requests: int


class Stats:
    def __init__(self) -> None:
        self.total_requests = 0
        self.ok_requests = 0
        self.fail_requests = 0
        self.timeout_requests = 0
        self.network_error_requests = 0
        self.http_codes: Counter[int] = Counter()
        self.exit_codes: Counter[int] = Counter()
        self.errors: Counter[str] = Counter()
        self.latencies: list[float] = []
        self.keepalive_total = 0
        self.keepalive_ok = 0
        self.keepalive_fail = 0

    def add_request(self, result: RequestResult) -> None:
        self.total_requests += 1
        self.http_codes[result.http_code] += 1
        self.exit_codes[result.exit_code] += 1
        if result.error != "-":
            self.errors[result.error] += 1
        if result.time_total > 0:
            self.latencies.append(result.time_total)
        if 200 <= result.http_code < 400 and result.exit_code == 0:
            self.ok_requests += 1
        else:
            self.fail_requests += 1
        error_lower = result.error.lower()
        if result.exit_code != 0 and error_lower.startswith("timed out:"):
            self.timeout_requests += 1
        if result.exit_code != 0 and error_lower.startswith("network error:"):
            self.network_error_requests += 1

    def add_health(self, result: HealthResult) -> None:
        self.keepalive_total += 1
        if result.status == "ok":
            self.keepalive_ok += 1
        else:
            self.keepalive_fail += 1


def env_bool(value: str | None, *, default: bool) -> bool:
    if value is None:
        return default
    return value.strip().lower() not in {"0", "false", "no", "off", ""}


def unique_out_dir(now: float | None = None) -> Path:
    ts = time.localtime(now)
    stamp = time.strftime("%Y%m%d-%H%M%S", ts)
    millis = int(((now or time.time()) % 1) * 1000)
    return DEFAULT_OUT_ROOT / f"{stamp}-{millis:03d}-{os.getpid()}"


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="Test smelly-connect VPN stability under sustained load.",
    )
    p.add_argument("urls", nargs="*", help="Target URLs")
    p.add_argument("--proxy-url")
    p.add_argument("--duration", type=float)
    p.add_argument("--concurrency", type=int)
    p.add_argument("--ramp-up", type=float)
    p.add_argument("--connect-timeout", type=float)
    p.add_argument("--max-time", type=float)
    p.add_argument("--out-dir")
    p.add_argument("--insecure", action="store_true")
    p.add_argument("--no-insecure", action="store_true")
    p.add_argument("--head-request", action="store_true")
    p.add_argument("--fresh-tcp-per-request", action="store_true")
    p.add_argument("--keepalive-url")
    p.add_argument("--keepalive-int", type=float)
    p.add_argument("--report-interval", type=float)
    return p


def load_settings(
    argv: list[str] | tuple[str, ...],
    env: Mapping[str, str] | None = None,
    *,
    now: float | None = None,
) -> Settings:
    values = dict(env or os.environ)
    args = parser().parse_args(list(argv))

    insecure_default = env_bool(values.get("CURL_INSECURE"), default=True)
    insecure = insecure_default
    if args.insecure:
        insecure = True
    if args.no_insecure:
        insecure = False

    proxy_url = args.proxy_url if args.proxy_url is not None else values.get("PROXY_URL")
    if proxy_url is None:
        proxy_url = DEFAULT_PROXY_URL
    proxy_url = proxy_url.strip() or None

    urls = tuple(args.urls or DEFAULT_URLS)
    out_dir = Path(args.out_dir or values.get("OUT_DIR") or unique_out_dir(now))

    return Settings(
        urls=urls,
        proxy_url=proxy_url,
        duration=float(args.duration if args.duration is not None else values.get("DURATION", 300)),
        concurrency=int(
            args.concurrency if args.concurrency is not None else values.get("CONCURRENCY", 10)
        ),
        ramp_up=float(args.ramp_up if args.ramp_up is not None else values.get("RAMP_UP", 30)),
        connect_timeout=float(
            args.connect_timeout
            if args.connect_timeout is not None
            else values.get("CONNECT_TIMEOUT", 30)
        ),
        max_time=float(args.max_time if args.max_time is not None else values.get("MAX_TIME", 30)),
        out_dir=out_dir,
        insecure=insecure,
        head_request=env_bool(
            "1" if args.head_request else values.get("HEAD_REQUEST"),
            default=False,
        ),
        fresh_tcp_per_request=env_bool(
            "1" if args.fresh_tcp_per_request else values.get("FRESH_TCP_PER_REQUEST"),
            default=False,
        ),
        keepalive_url=args.keepalive_url or values.get("KEEPALIVE_URL", DEFAULT_KEEPALIVE_URL),
        keepalive_interval=float(
            args.keepalive_int if args.keepalive_int is not None else values.get("KEEPALIVE_INT", 30)
        ),
        report_interval=float(
            args.report_interval
            if args.report_interval is not None
            else values.get("REPORT_INTERVAL", DEFAULT_REPORT_INTERVAL)
        ),
    )


def current_concurrency(elapsed: float, settings: Settings) -> int:
    if settings.ramp_up <= 0 or elapsed >= settings.ramp_up:
        return settings.concurrency
    ratio = elapsed / settings.ramp_up
    return max(1, 1 + math.floor((settings.concurrency - 1) * ratio))


def now_ms() -> int:
    return int(time.time() * 1000)


def percentile(values: list[float], p: int) -> float:
    if not values:
        return 0.0
    index = min(len(values) - 1, max(0, math.ceil(len(values) * p / 100) - 1))
    return values[index]


def classify_error(exc: Exception) -> tuple[int, str]:
    message = str(exc).strip() or exc.__class__.__name__
    lowered = message.lower()
    if isinstance(exc, httpx.TimeoutException):
        return 28, f"timed out: {message}"
    if isinstance(exc, (httpx.ProxyError, httpx.ConnectError, httpx.NetworkError)):
        return 7, f"network error: {message}"
    if "timed out" in lowered or "timeout" in lowered:
        return 28, f"timed out: {message}"
    return 1, f"http error: {message}"


async def execute_request(
    client: httpx.AsyncClient,
    settings: Settings,
    request_id: int,
    url: str,
) -> RequestResult:
    started = time.perf_counter()
    ts_ms = now_ms()
    method = "HEAD" if settings.head_request else "GET"
    try:
        response = await client.request(method, url)
    except Exception as exc:
        exit_code, error = classify_error(exc)
        return RequestResult(
            ts_ms=ts_ms,
            request_id=request_id,
            url=url,
            exit_code=exit_code,
            http_code=0,
            time_total=time.perf_counter() - started,
            time_connect=0.0,
            error=error,
        )

    return RequestResult(
        ts_ms=ts_ms,
        request_id=request_id,
        url=url,
        exit_code=0,
        http_code=response.status_code,
        time_total=time.perf_counter() - started,
        time_connect=0.0,
        error="-",
    )


async def execute_request_with_fresh_client(
    settings: Settings,
    request_id: int,
    url: str,
    timeout: httpx.Timeout,
    limits: httpx.Limits,
) -> RequestResult:
    async with httpx.AsyncClient(
        proxy=settings.proxy_url,
        verify=not settings.insecure,
        timeout=timeout,
        limits=limits,
        trust_env=False,
        follow_redirects=False,
        headers={"Connection": "close"},
    ) as client:
        return await execute_request(client, settings, request_id, url)


async def keepalive_loop(
    client: httpx.AsyncClient,
    settings: Settings,
    stop_event: asyncio.Event,
    health_path: Path,
    stats: Stats,
) -> None:
    while not stop_event.is_set():
        started = time.perf_counter()
        status = "ok"
        try:
            await client.get(settings.keepalive_url)
        except Exception:
            status = "fail"
        latency_ms = int((time.perf_counter() - started) * 1000)
        health = HealthResult(ts_ms=now_ms(), status=status, latency_ms=latency_ms)
        stats.add_health(health)
        with health_path.open("a", encoding="utf-8") as health_file:
            health_file.write(health.to_tsv())
        try:
            await asyncio.wait_for(stop_event.wait(), timeout=settings.keepalive_interval)
        except TimeoutError:
            continue


def write_summary(settings: Settings, stats: Stats, summary_path: Path) -> None:
    sorted_latencies = sorted(stats.latencies)
    lines = [
        f"proxy_url={settings.proxy_url or '-'}",
        f"duration={settings.duration}s",
        f"concurrency={settings.concurrency}",
        f"ramp_up={settings.ramp_up}s",
        f"connect_timeout={settings.connect_timeout:.1f}s",
        f"max_time={settings.max_time:.1f}s",
        f"targets={' '.join(settings.urls)}",
        f"total_requests={stats.total_requests}",
        f"timeout_requests={stats.timeout_requests}",
        f"network_error_requests={stats.network_error_requests}",
        "",
        "[http_code_distribution]",
    ]
    lines.extend(f"  {code:03d}\t{count}" for code, count in sorted(stats.http_codes.items()))
    lines.extend(
        [
            "",
            "[curl_exit_distribution]",
        ]
    )
    lines.extend(f"  {code}\t{count}" for code, count in sorted(stats.exit_codes.items()))
    lines.extend(["", "[latency_percentiles]"])
    if sorted_latencies:
        lines.append(
            "  "
            f"p50={percentile(sorted_latencies, 50):.2f}s  "
            f"p90={percentile(sorted_latencies, 90):.2f}s  "
            f"p95={percentile(sorted_latencies, 95):.2f}s  "
            f"p99={percentile(sorted_latencies, 99):.2f}s  "
            f"(n={len(sorted_latencies)})"
        )
    else:
        lines.append("  no data")
    lines.extend(["", "[error_summary]"])
    for error, count in stats.errors.most_common(10):
        lines.append(f"  {count}\t{error}")
    lines.extend(["", "[keepalive_health]"])
    lines.append(
        f"  total={stats.keepalive_total}  ok={stats.keepalive_ok}  fail={stats.keepalive_fail}"
    )
    if stats.keepalive_total:
        success_rate = (stats.keepalive_ok / stats.keepalive_total) * 100
        lines.append(f"  success_rate={success_rate:.1f}%")
    else:
        lines.append("  disabled")
    summary_path.write_text("\n".join(lines) + "\n", encoding="utf-8")


async def run_load_test(settings: Settings) -> RunResult:
    settings.out_dir.mkdir(parents=True, exist_ok=True)
    results_path = settings.out_dir / "results.tsv"
    summary_path = settings.out_dir / "summary.txt"
    health_path = settings.out_dir / "health.tsv"
    log_path = settings.out_dir / "test.log"

    results_path.write_text(
        "ts\treq_id\turl\tcurl_exit\thttp_code\ttime_total\ttime_connect\terror\n",
        encoding="utf-8",
    )
    health_path.write_text("ts\tstatus\tlatency_ms\n", encoding="utf-8")

    stats = Stats()

    log_file = log_path.open("a", encoding="utf-8", buffering=1)

    def log(message: str) -> None:
        line = f"[{time.strftime('%H:%M:%S')}] {message}"
        print(line)
        log_file.write(line + "\n")
        log_file.flush()

    limits = httpx.Limits(
        max_connections=max(100, settings.concurrency * 2),
        max_keepalive_connections=max(20, settings.concurrency),
    )
    timeout = httpx.Timeout(
        settings.max_time,
        connect=settings.connect_timeout,
        read=settings.max_time,
        write=settings.max_time,
        pool=settings.max_time,
    )

    client = httpx.AsyncClient(
        proxy=settings.proxy_url,
        verify=not settings.insecure,
        timeout=timeout,
        limits=limits,
        trust_env=False,
        follow_redirects=False,
    )

    stop_event = asyncio.Event()
    keepalive_task: asyncio.Task[None] | None = None
    if settings.keepalive_interval > 0:
        keepalive_task = asyncio.create_task(
            keepalive_loop(client, settings, stop_event, health_path, stats)
        )

    log("=== VPN Stability Test ===")
    log(
        "proxy="
        f"{settings.proxy_url or '-'}  duration={settings.duration}s  "
        f"concurrency={settings.concurrency}  ramp_up={settings.ramp_up}s"
    )
    log(
        "request_mode="
        + ("fresh_tcp_per_request" if settings.fresh_tcp_per_request else "shared_client")
    )
    log(f"targets: {' '.join(settings.urls)}")
    log(f"results={results_path}")
    log("")

    pending: set[asyncio.Task[RequestResult]] = set()
    started = time.perf_counter()
    last_report = started
    next_request_id = 0

    results_file = results_path.open("a", encoding="utf-8", buffering=1)
    try:
        while True:
            elapsed = time.perf_counter() - started
            if elapsed >= settings.duration:
                break

            if settings.report_interval > 0 and elapsed - (last_report - started) >= settings.report_interval:
                log(
                    f"  [{elapsed:.0f}s/{settings.duration:.0f}s] "
                    f"reqs={next_request_id} ok={stats.ok_requests} fail={stats.fail_requests} "
                    f"timeout={stats.timeout_requests} network={stats.network_error_requests} "
                    f"in_flight={len(pending)}"
                )
                last_report = time.perf_counter()

            want = current_concurrency(elapsed, settings)
            while len(pending) < want:
                next_request_id += 1
                url = settings.urls[(next_request_id - 1) % len(settings.urls)]
                if settings.fresh_tcp_per_request:
                    task = execute_request_with_fresh_client(
                        settings,
                        next_request_id,
                        url,
                        timeout,
                        limits,
                    )
                else:
                    task = execute_request(client, settings, next_request_id, url)
                pending.add(asyncio.create_task(task))

            if pending:
                done, pending = await asyncio.wait(
                    pending,
                    timeout=0.05,
                    return_when=asyncio.FIRST_COMPLETED,
                )
                for task in done:
                    result = task.result()
                    stats.add_request(result)
                    results_file.write(result.to_tsv())
            else:
                await asyncio.sleep(0.01)

        log("Duration reached, waiting for in-flight requests...")
        if pending:
            for task in asyncio.as_completed(pending):
                result = await task
                stats.add_request(result)
                results_file.write(result.to_tsv())

        log("")
        log("=== Results ===")
        write_summary(settings, stats, summary_path)
        log("")
        log(f"results_file={results_path}")
        log(f"summary_file={summary_path}")
        log(f"health_file={health_path}")
        log("Done.")
    finally:
        stop_event.set()
        if keepalive_task is not None:
            await keepalive_task
        await client.aclose()
        results_file.close()
        log_file.close()

    return RunResult(
        out_dir=settings.out_dir,
        results_path=results_path,
        summary_path=summary_path,
        health_path=health_path,
        log_path=log_path,
        total_requests=stats.total_requests,
        ok_requests=stats.ok_requests,
        fail_requests=stats.fail_requests,
        timeout_requests=stats.timeout_requests,
        network_error_requests=stats.network_error_requests,
    )


def main(argv: list[str] | None = None) -> int:
    settings = load_settings(list(sys.argv[1:] if argv is None else argv))
    try:
        asyncio.run(run_load_test(settings))
    except KeyboardInterrupt:
        print("Interrupted.", file=sys.stderr)
        return 130
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
