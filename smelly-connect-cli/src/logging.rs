use std::fs::OpenOptions;
#[cfg(any(test, debug_assertions))]
use std::io::Write;
use std::io::{self};
use std::path::Path;
#[cfg(any(test, debug_assertions))]
use std::sync::{Arc, Mutex};

#[cfg(any(test, debug_assertions))]
use tracing::dispatcher::with_default;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::time::UtcTime;
#[cfg(any(test, debug_assertions))]
use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::fmt::writer::{BoxMakeWriter, MakeWriterExt};
use tracing_subscriber::prelude::*;

use crate::config::{LoggingConfig, LoggingLevel, LoggingMode};

pub struct LoggingGuard {
    _file_guard: Option<WorkerGuard>,
}

pub fn emit_fatal_stderr(message: &str) {
    eprintln!("ERROR smelly_connect_cli {message}");
}

pub fn init_logging(cfg: &LoggingConfig) -> Result<LoggingGuard, String> {
    let (writer, guard) = build_writer(cfg)?;
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_target(true)
            .with_timer(UtcTime::rfc_3339())
            .with_writer(writer)
            .with_span_events(FmtSpan::NONE)
            .with_filter(level_filter(&cfg.level)),
    );
    tracing::subscriber::set_global_default(subscriber).map_err(|err| err.to_string())?;
    Ok(LoggingGuard { _file_guard: guard })
}

#[cfg(any(test, debug_assertions))]
pub fn init_for_test(mode: &str, level: &str, file: Option<&str>) -> Result<(), String> {
    let cfg = LoggingConfig {
        mode: parse_mode(mode)?,
        level: parse_level(level)?,
        file: file.unwrap_or("smelly-connect.log").to_string(),
    };
    let _ = build_writer(&cfg)?;
    Ok(())
}

#[cfg(any(test, debug_assertions))]
pub fn capture_level_filter_for_test(level: &str) -> Vec<String> {
    capture_lines(
        parse_mode("stdout").unwrap(),
        parse_level(level).unwrap(),
        || {
            tracing::info!(target: "smelly_connect_cli::logging_test", "suppressed maybe");
            tracing::error!(target: "smelly_connect_cli::logging_test", "always visible");
        },
    )
}

#[cfg(any(test, debug_assertions))]
pub fn capture_one_info_line_for_test() -> String {
    capture_lines(parse_mode("stdout").unwrap(), LoggingLevel::Info, || {
        tracing::info!(target: "smelly_connect_cli::logging_test", "hello");
    })
    .into_iter()
    .find(|line| line.contains(" INFO "))
    .unwrap_or_default()
}

#[cfg(any(test, debug_assertions))]
pub fn capture_pool_events_for_test() -> Vec<String> {
    capture_lines(parse_mode("stdout").unwrap(), LoggingLevel::Info, || {
        tracing::info!(configured = 2, prewarm = 1, "pool prewarm start");
        tracing::info!(account = "acct-01", "account ready");
        tracing::info!(configured = 2, ready = 1, "pool startup summary");
    })
}

#[cfg(any(test, debug_assertions))]
pub fn capture_http_request_log_for_test() -> Vec<String> {
    capture_async_lines(parse_mode("stdout").unwrap(), LoggingLevel::Info, async {
        let _ = crate::proxy::http::proxy_http_for_test().await;
    })
}

#[cfg(any(test, debug_assertions))]
pub fn capture_http_connect_log_for_test() -> Vec<String> {
    capture_async_lines(parse_mode("stdout").unwrap(), LoggingLevel::Info, async {
        let _ = crate::proxy::http::proxy_connect_for_test().await;
    })
}

#[cfg(any(test, debug_assertions))]
pub fn capture_socks5_request_log_for_test() -> Vec<String> {
    capture_async_lines(parse_mode("stdout").unwrap(), LoggingLevel::Info, async {
        let _ = crate::proxy::socks5::proxy_socks5_for_test().await;
    })
}

#[cfg(any(test, debug_assertions))]
pub fn capture_no_ready_session_warn_for_test() -> Vec<String> {
    capture_async_lines(parse_mode("stdout").unwrap(), LoggingLevel::Warn, async {
        let _ = crate::proxy::http::proxy_http_no_ready_session_for_test().await;
    })
}

#[cfg(any(test, debug_assertions))]
pub fn capture_config_load_error_for_test(path: &str) -> Vec<String> {
    capture_lines(parse_mode("stdout").unwrap(), LoggingLevel::Error, || {
        if let Err(err) = crate::config::load(path) {
            tracing::error!(error = %err, path, "configuration load failed");
        }
    })
}

#[cfg(any(test, debug_assertions))]
pub fn capture_invalid_logging_config_error_for_test() -> Vec<String> {
    capture_lines(parse_mode("stdout").unwrap(), LoggingLevel::Error, || {
        tracing::error!("invalid logging config");
    })
}

#[cfg(any(test, debug_assertions))]
fn capture_lines<F>(mode: LoggingMode, level: LoggingLevel, emit: F) -> Vec<String>
where
    F: FnOnce(),
{
    let capture = CaptureBuffer::default();
    let writer = match mode {
        LoggingMode::Off => BoxMakeWriter::new(io::sink),
        LoggingMode::Stdout | LoggingMode::File | LoggingMode::StdoutAndFile => {
            BoxMakeWriter::new(capture.clone())
        }
    };
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_target(true)
            .with_timer(UtcTime::rfc_3339())
            .with_writer(writer)
            .with_span_events(FmtSpan::NONE)
            .with_filter(level_filter(&level)),
    );
    let dispatch = tracing::Dispatch::new(subscriber);
    with_default(&dispatch, emit);
    capture.lines()
}

#[cfg(any(test, debug_assertions))]
fn capture_async_lines<Fut>(mode: LoggingMode, level: LoggingLevel, emit: Fut) -> Vec<String>
where
    Fut: std::future::Future<Output = ()>,
{
    capture_lines(mode, level, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        rt.block_on(emit);
    })
}

fn build_writer(cfg: &LoggingConfig) -> Result<(BoxMakeWriter, Option<WorkerGuard>), String> {
    match cfg.mode {
        LoggingMode::Off => Ok((BoxMakeWriter::new(io::sink), None)),
        LoggingMode::Stdout => Ok((BoxMakeWriter::new(io::stderr), None)),
        LoggingMode::File => match open_file_writer(&cfg.file) {
            Ok((writer, guard)) => Ok((BoxMakeWriter::new(writer), Some(guard))),
            Err(err) => {
                eprintln!("WARN logging file open failed, falling back to stderr: {err}");
                Ok((BoxMakeWriter::new(io::stderr), None))
            }
        },
        LoggingMode::StdoutAndFile => match open_file_writer(&cfg.file) {
            Ok((writer, guard)) => {
                let dual = io::stderr.and(writer);
                Ok((BoxMakeWriter::new(dual), Some(guard)))
            }
            Err(err) => {
                eprintln!("WARN logging file open failed, falling back to stderr: {err}");
                Ok((BoxMakeWriter::new(io::stderr), None))
            }
        },
    }
}

fn open_file_writer(
    path: impl AsRef<Path>,
) -> Result<(tracing_appender::non_blocking::NonBlocking, WorkerGuard), String> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| err.to_string())?;
    Ok(tracing_appender::non_blocking(file))
}

fn level_filter(level: &LoggingLevel) -> LevelFilter {
    match level {
        LoggingLevel::Error => LevelFilter::ERROR,
        LoggingLevel::Warn => LevelFilter::WARN,
        LoggingLevel::Info => LevelFilter::INFO,
        LoggingLevel::Debug => LevelFilter::DEBUG,
    }
}

#[cfg(any(test, debug_assertions))]
fn parse_mode(value: &str) -> Result<LoggingMode, String> {
    match value {
        "stdout" => Ok(LoggingMode::Stdout),
        "file" => Ok(LoggingMode::File),
        "stdout+file" => Ok(LoggingMode::StdoutAndFile),
        "off" => Ok(LoggingMode::Off),
        other => Err(format!("invalid logging mode: {other}")),
    }
}

#[cfg(any(test, debug_assertions))]
fn parse_level(value: &str) -> Result<LoggingLevel, String> {
    match value {
        "error" => Ok(LoggingLevel::Error),
        "warn" => Ok(LoggingLevel::Warn),
        "info" => Ok(LoggingLevel::Info),
        "debug" => Ok(LoggingLevel::Debug),
        other => Err(format!("invalid logging level: {other}")),
    }
}

#[cfg(any(test, debug_assertions))]
#[derive(Clone, Default)]
struct CaptureBuffer(Arc<Mutex<Vec<u8>>>);

#[cfg(any(test, debug_assertions))]
impl CaptureBuffer {
    fn lines(&self) -> Vec<String> {
        let bytes = self.0.lock().expect("capture mutex poisoned").clone();
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(|line| line.to_string())
            .collect()
    }
}

#[cfg(any(test, debug_assertions))]
impl<'a> MakeWriter<'a> for CaptureBuffer {
    type Writer = CaptureGuard;

    fn make_writer(&'a self) -> Self::Writer {
        CaptureGuard {
            inner: Arc::clone(&self.0),
        }
    }
}

#[cfg(any(test, debug_assertions))]
struct CaptureGuard {
    inner: Arc<Mutex<Vec<u8>>>,
}

#[cfg(any(test, debug_assertions))]
impl Write for CaptureGuard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner
            .lock()
            .expect("capture mutex poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
