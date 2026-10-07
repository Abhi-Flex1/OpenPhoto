//! Routes the `log` facade to HarmonyOS `hilog`, so `hdc hilog` shows exactly what the desktop
//! build prints to stderr.
use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use std::sync::Once;

/// `hilog` truncates long lines; keep each record readable in `hdc hilog` output.
const MAX_RECORD: usize = 1024;

struct HilogLogger;

static LOGGER: HilogLogger = HilogLogger;
static INIT: Once = Once::new();

impl Log for HilogLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let full = format!("{} [{}] {}", record.target(), record.level(), record.args());
        let mut line = full;
        if line.len() > MAX_RECORD {
            line.truncate(line.floor_char_boundary(MAX_RECORD));
            line.push('…');
        }
        match record.level() {
            Level::Error => ohos_hilog_binding::error(line),
            Level::Warn => ohos_hilog_binding::warn(line),
            Level::Info => ohos_hilog_binding::info(line),
            Level::Debug | Level::Trace => ohos_hilog_binding::debug(line),
        }
    }

    fn flush(&self) {}
}

/// Installs the hilog logger once. Returns `Err` when another logger is already installed, which
/// is not fatal: the caller keeps running and only loses log output.
pub fn install() -> Result<(), SetLoggerError> {
    let mut first_error: Option<SetLoggerError> = None;
    INIT.call_once(|| {
        ohos_hilog_binding::set_global_options(ohos_hilog_binding::LogOptions { tag: crate::LOG_TAG, domain: 0x0000 });
        if let Err(error) = log::set_logger(&LOGGER) {
            first_error = Some(error);
        } else {
            log::set_max_level(LevelFilter::Info);
        }
    });
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
