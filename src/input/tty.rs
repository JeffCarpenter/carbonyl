use std::io;
use std::io::Write;

use crossterm::terminal;

use crate::utils::log;

pub struct Terminal {
    raw_mode_enabled: bool,
    alt_screen: bool,
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.teardown()
    }
}

impl Terminal {
    /// Setup the input stream to operate in raw mode.
    /// Returns an object that'll revert terminal settings.
    pub fn setup() -> Self {
        let raw_mode_enabled = if let Err(error) = terminal::enable_raw_mode() {
            log::error!("Failed to enable raw mode: {error}");
            false
        } else {
            true
        };

        let alt_screen = if let Err(error) = Self::enter_alt_screen() {
            log::error!("Failed to enter alternative screen: {error}");
            false
        } else {
            true
        };

        Self {
            raw_mode_enabled,
            alt_screen,
        }
    }

    pub fn teardown(&mut self) {
        if self.raw_mode_enabled {
            if let Err(error) = terminal::disable_raw_mode() {
                log::error!("Failed to disable raw mode: {error}");
            }
            self.raw_mode_enabled = false;
        }

        if self.alt_screen {
            if let Err(error) = Self::quit_alt_screen() {
                log::error!("Failed to quit alternative screen: {error}");
            }
            self.alt_screen = false;
        }
    }
}

const SEQUENCES: [(u32, bool); 4] = [(1049, true), (1003, true), (1006, true), (25, false)];

impl Terminal {
    fn enter_alt_screen() -> io::Result<()> {
        let mut out = io::stdout();

        for (sequence, enable) in SEQUENCES {
            write!(out, "\x1b[?{}{}", sequence, if enable { "h" } else { "l" })?;
        }

        // Set the current background color to black
        write!(out, "\x1b[48;2;0;0;0m")?;
        // Query current foreground color to for true-color support detection
        write!(out, "\x1bP$qm\x1b\\")?;
        // Query current terminal name
        write!(out, "\x1bP+q544e\x1b\\")?;
        // Query graphics capability (XTSMGRAPHICS). Some terminals expect DCS form;
        // use it first and fall back to CSI if ignored.
        write!(out, "\x1bP?2;1;0S\x1b\\")?;
        write!(out, "\x1b[?2;1;0S")?;

        out.flush()
    }

    fn quit_alt_screen() -> io::Result<()> {
        let mut out = io::stdout();

        for (sequence, enable) in SEQUENCES {
            write!(out, "\x1b[?{}{}", sequence, if enable { "l" } else { "h" })?;
        }

        out.flush()
    }
}
