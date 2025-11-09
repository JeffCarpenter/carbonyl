use std::fs::File;
use std::io;
use std::io::Write;
use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::os::unix::prelude::AsRawFd;

use crate::utils::log;

// This module requires Unix-like operating systems for terminal control operations
#[cfg(not(unix))]
compile_error!("Terminal TTY operations require Unix platform");

pub struct Terminal {
    settings: Option<TerminalSettings>,
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
        Self {
            settings: match TerminalSettings::open_raw() {
                Ok(settings) => Some(settings),
                Err(error) => {
                    log::error!("Failed to setup terminal: {error}");

                    None
                }
            },
            alt_screen: if let Err(error) = TTY::enter_alt_screen() {
                log::error!("Failed to enter alternative screen: {error}");

                false
            } else {
                true
            },
        }
    }

    pub fn teardown(&mut self) {
        if let Some(ref settings) = self.settings {
            if let Err(error) = settings.apply() {
                log::error!("Failed to revert terminal settings: {error}");
            }

            self.settings = None;
        }

        if self.alt_screen {
            if let Err(error) = TTY::quit_alt_screen() {
                log::error!("Failed to quit alternative screen: {error}");
            }

            self.alt_screen = false;
        }
    }
}

enum TTY {
    Raw(RawFd),
    File(File),
}

const SEQUENCES: [(u32, bool); 4] = [(1049, true), (1003, true), (1006, true), (25, false)];

impl TTY {
    fn stdin() -> TTY {
        // SAFETY: STDIN_FILENO is a constant (0) representing standard input.
        // isatty is safe to call with any file descriptor - it returns 1 if the
        // file descriptor refers to a terminal, 0 otherwise. No memory is modified.
        let isatty = unsafe { libc::isatty(libc::STDIN_FILENO) };

        if isatty != 1 {
            if let Ok(file) = File::open("/dev/tty") {
                return TTY::File(file);
            }
        }

        TTY::Raw(libc::STDIN_FILENO)
    }

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

    fn as_raw_fd(self) -> RawFd {
        match self {
            TTY::Raw(fd) => fd,
            TTY::File(file) => file.as_raw_fd(),
        }
    }
}

trait ToErr {
    fn to_err(self) -> io::Result<()>;
}
impl ToErr for libc::c_int {
    fn to_err(self) -> io::Result<()> {
        if self == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

/// Safe wrapper around libc::termios
#[derive(Clone)]
struct TerminalSettings {
    data: libc::termios,
}

impl TerminalSettings {
    /// Fetch settings from the current TTY
    fn open() -> io::Result<Self> {
        let tty = TTY::stdin();
        let mut term = MaybeUninit::uninit();
        let data = unsafe {
            // SAFETY: tty.as_raw_fd() returns a valid file descriptor (either STDIN_FILENO
            // or a descriptor from an opened /dev/tty file). tcgetattr writes a termios
            // struct to the provided pointer. We've allocated space via MaybeUninit.
            // We check the return value via to_err() before calling assume_init().
            libc::tcgetattr(tty.as_raw_fd(), term.as_mut_ptr()).to_err()?;

            // SAFETY: tcgetattr succeeded (to_err() didn't return Err), so the termios
            // struct has been properly initialized and can be safely read.
            term.assume_init()
        };

        Ok(Self { data })
    }

    fn open_raw() -> io::Result<TerminalSettings> {
        let mut raw = Self::open()?;
        let settings = raw.clone();

        raw.make_raw();
        raw.apply()?;

        Ok(settings)
    }

    /// Enable raw input
    fn make_raw(&mut self) {
        let c_oflag = self.data.c_oflag;

        // Set the terminal to raw mode
        // SAFETY: cfmakeraw modifies the termios struct in place by setting
        // appropriate flags for raw mode. The struct is valid because it was
        // initialized via tcgetattr. This function performs bitwise operations
        // on the struct fields and is safe to call on valid termios structs.
        unsafe { libc::cfmakeraw(&mut self.data) }

        // Restore output flags, ensures carriage returns are consistent
        self.data.c_oflag = c_oflag;
    }

    /// Apply the settings to the current TTY
    fn apply(&self) -> io::Result<()> {
        let tty = TTY::stdin();

        // SAFETY: tty.as_raw_fd() returns a valid file descriptor. self.data is a
        // valid termios struct (either from tcgetattr or properly modified via make_raw).
        // tcsetattr applies the terminal settings. TCSANOW means apply immediately.
        // This is safe because the struct contains valid terminal configuration.
        unsafe { libc::tcsetattr(tty.as_raw_fd(), libc::TCSANOW, &self.data).to_err() }
    }
}
