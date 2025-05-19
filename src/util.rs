//! Utility functions and macros for debugging

/// Debugging macro that prints the current time (ms since program start) and the thread name or ID,
/// along with the provided message. Integrates with the tracing framework.
#[macro_export]
macro_rules! tdbg {
    ($($arg:tt)*) => {{
        use std::time::{SystemTime, UNIX_EPOCH};
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
        tracing::debug!("[{:>11} ms][{:?}] {}", ms, std::thread::current().name().unwrap_or("unnamed"), format_args!($($arg)*));
    }};
}

// The macro is exported at crate root already, no need for re-export
