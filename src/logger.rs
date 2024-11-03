// TODO: Disabling logging for the entire HTTP module

use log::{Level, LevelFilter, Metadata, Record, SetLoggerError};

pub fn init(level_filter: LevelFilter, enable_colors: bool) -> Result<(), SetLoggerError> {
    log::set_boxed_logger(Box::new(NGMPLogger {
        level_filter,
        enable_colors,
    }))
    .map(|()| log::set_max_level(level_filter))
}

struct NGMPLogger {
    level_filter: LevelFilter,
    enable_colors: bool,
}

impl log::Log for NGMPLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level_filter
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let level = record.level();
            let filler = if level == Level::Info || level == Level::Warn {
                " "
            } else {
                ""
            };
            let color = if self.enable_colors {
                match level {
                    Level::Warn => "\x1b[0;33m",
                    Level::Error => "\x1b[0;31m",
                    Level::Debug => "\x1b[0;36m",
                    Level::Trace => "\x1b[0;35m",
                    _ => "\x1b[0;37m",
                }
            } else {
                ""
            };
            let color_reset = if self.enable_colors { "\x1b[0;37m" } else { "" };
            println!(
                "{}[{}]{}{} {}",
                color,
                level,
                color_reset,
                filler,
                record.args()
            )
        }
    }

    fn flush(&self) {}
}
