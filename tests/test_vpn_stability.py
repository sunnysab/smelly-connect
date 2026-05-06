from __future__ import annotations

import asyncio
import http.server
import importlib.util
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = ROOT / "scripts" / "vpn_stability.py"


def load_module():
    spec = importlib.util.spec_from_file_location("vpn_stability", MODULE_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class _Handler(http.server.BaseHTTPRequestHandler):
    status_code = 200
    delay_secs = 0.0

    def do_GET(self):
        if self.delay_secs:
            time.sleep(self.delay_secs)
        self.send_response(self.status_code)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_HEAD(self):
        self.do_GET()

    def log_message(self, format, *args):  # noqa: A003
        return


class LocalHttpServer:
    def __init__(self, *, status_code: int = 200, delay_secs: float = 0.0):
        handler = type(
            "CustomHandler",
            (_Handler,),
            {"status_code": status_code, "delay_secs": delay_secs},
        )
        self._server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)

    @property
    def url(self) -> str:
        host, port = self._server.server_address
        return f"http://{host}:{port}/"

    def __enter__(self):
        self._thread.start()
        return self

    def __exit__(self, exc_type, exc, tb):
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=2)


def test_settings_default_timeout_is_30_seconds():
    module = load_module()

    first = module.load_settings([], {}, now=1_700_000_000.0)
    second = module.load_settings([], {}, now=1_700_000_001.0)

    assert first.urls == ("https://xg.sit.edu.cn/",)
    assert first.keepalive_url == "https://jwxt.sit.edu.cn/"
    assert first.connect_timeout == 30.0
    assert first.max_time == 30.0
    assert first.out_dir != second.out_dir


def test_run_load_test_writes_summary_and_results(tmp_path):
    module = load_module()

    with LocalHttpServer(status_code=200) as server:
        settings = module.load_settings(
            [server.url],
            {
                "PROXY_URL": "",
                "KEEPALIVE_INT": "0",
                "DURATION": "0.35",
                "CONCURRENCY": "3",
                "RAMP_UP": "0",
                "OUT_DIR": str(tmp_path / "success"),
            },
            now=1_700_000_000.0,
        )

        result = asyncio.run(module.run_load_test(settings))

    assert result.total_requests > 0
    assert result.ok_requests == result.total_requests
    assert result.fail_requests == 0

    summary_text = result.summary_path.read_text()
    results_text = result.results_path.read_text()

    assert "connect_timeout=30.0s" in summary_text
    assert "max_time=30.0s" in summary_text
    assert "\t200\t" in results_text


def test_run_load_test_records_timeout_failures(tmp_path):
    module = load_module()

    with LocalHttpServer(status_code=200, delay_secs=0.2) as server:
        settings = module.load_settings(
            [server.url],
            {
                "PROXY_URL": "",
                "KEEPALIVE_INT": "0",
                "DURATION": "0.12",
                "CONCURRENCY": "1",
                "RAMP_UP": "0",
                "CONNECT_TIMEOUT": "0.05",
                "MAX_TIME": "0.05",
                "OUT_DIR": str(tmp_path / "timeout"),
            },
            now=1_700_000_000.0,
        )

        result = asyncio.run(module.run_load_test(settings))

    assert result.fail_requests > 0
    assert result.timeout_requests == result.fail_requests
    assert "timed out" in result.results_path.read_text().lower()
