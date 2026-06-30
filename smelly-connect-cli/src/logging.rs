use std::fs::OpenOptions;
use std::io::{self};
use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::time::UtcTime;
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


