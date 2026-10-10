#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PROCESS_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[test]
fn proxy_process_shuts_down_gracefully_on_sigterm() {
    assert_graceful_shutdown("TERM");
}

#[test]
fn proxy_process_shuts_down_gracefully_on_sigint() {
    assert_graceful_shutdown("INT");
}

fn assert_graceful_shutdown(signal: &str) {
    let mut proxy = ProxyProcess::start();
    proxy.wait_for_listener();

    // Unix 测试使用系统 kill 发送真实信号，无需新增依赖。
    let sent = Command::new("kill")
        .args(["-s", signal, &proxy.child.id().to_string()])
        .output()
        .expect("run Unix kill command");
    assert!(
        sent.status.success(),
        "failed to send SIG{signal}: {}",
        String::from_utf8_lossy(&sent.stderr)
    );

    let status = proxy.wait_for_exit();
    let output = proxy.output();
    assert!(
        status.success(),
        "proxy should exit successfully after SIG{signal}, got {status}\n{output}"
    );
    assert!(
        output.contains("shutdown complete"),
        "proxy should finish graceful shutdown after SIG{signal}\n{output}"
    );
}

struct ProxyProcess {
    child: Child,
    directory: TempDirectory,
}

impl ProxyProcess {
    fn start() -> Self {
        let directory = TempDirectory::new();
        let config_path = directory.0.join("config.toml");
        fs::write(
            &config_path,
            r#"
            accounts = []

            [vpn]
            server = "127.0.0.1"
            enable_icmp_keepalive = false

            [pool]
            min_pool_size = 0

            [proxy.http]
            enabled = true
            listen = "127.0.0.1:0"

            [proxy.socks5]
            enabled = false
            listen = "127.0.0.1:0"

            [logging]
            mode = "stdout"
            level = "info"
            "#,
        )
        .expect("write proxy test config");
        let output = fs::File::create(directory.0.join("output.log"))
            .expect("create proxy process output file");
        let child = Command::new(env!("CARGO_BIN_EXE_smelly-connect-cli"))
            .arg("--config")
            .arg(&config_path)
            .arg("proxy")
            .current_dir(&directory.0)
            .stdin(Stdio::null())
            .stdout(output.try_clone().expect("clone process output file"))
            .stderr(output)
            .spawn()
            .expect("start proxy process");
        Self { child, directory }
    }

    fn wait_for_listener(&mut self) {
        let deadline = Instant::now() + PROCESS_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().expect("check proxy process") {
                panic!(
                    "proxy exited before its listener was ready: {status}\n{}",
                    self.output()
                );
            }
            // 监听地址使用端口 0；bind 成功后输出此日志，避免预占端口的竞争。
            if self.output().contains("http proxy listening") {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "proxy listener did not become ready\n{}",
                self.output()
            );
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn wait_for_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + PROCESS_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().expect("check proxy process") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "proxy did not exit within {PROCESS_TIMEOUT:?}\n{}",
                self.output()
            );
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn output(&self) -> String {
        fs::read_to_string(self.directory.0.join("output.log")).expect("read proxy process output")
    }
}

impl Drop for ProxyProcess {
    fn drop(&mut self) {
        // 超时或断言失败时也终止并回收进程，随后 TempDirectory 清理诊断文件。
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "smelly-connect-cli-signals-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create proxy test directory");
        Self(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
