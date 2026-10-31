//! Optional logging. Off by default; the level and sink are process-wide.

use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    Off = 0,
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

type Sink = Box<dyn Fn(Level, &str) + Send + Sync>;

struct Logger {
    level: Level,
    sink: Option<Sink>,
}

fn logger() -> &'static Mutex<Logger> {
    static LOGGER: OnceLock<Mutex<Logger>> = OnceLock::new();
    LOGGER.get_or_init(|| {
        Mutex::new(Logger {
            level: Level::Off,
            sink: None,
        })
    })
}

pub fn set_level(level: Level) {
    if let Ok(mut logger) = logger().lock() {
        logger.level = level;
    }
}

pub fn level() -> Level {
    logger()
        .lock()
        .map(|logger| logger.level)
        .unwrap_or(Level::Off)
}

pub fn set_sink(sink: Option<Sink>) {
    if let Ok(mut logger) = logger().lock() {
        logger.sink = sink;
    }
}

pub fn enabled(level: Level) -> bool {
    level <= self::level()
}

pub fn log(level: Level, message: impl AsRef<str>) {
    let Ok(logger) = logger().lock() else {
        return;
    };
    if level > logger.level {
        return;
    }
    if let Some(sink) = logger.sink.as_ref() {
        sink(level, message.as_ref());
    }
}

#[macro_export]
macro_rules! sdk_log {
    ($level:expr, $($arg:tt)*) => {
        if $crate::log::enabled($level) {
            $crate::log::log($level, format!($($arg)*));
        }
    };
}
