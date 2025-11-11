use std::{
    io::{Read, Write},
    str::FromStr,
    time::{Duration, Instant},
};

use crossterm::terminal;

use crate::{cli::CommandLine, gfx::Size, utils::log};

// Unix-specific imports for advanced terminal querying
#[cfg(unix)]
use std::{fs::OpenOptions, os::fd::AsRawFd, mem::MaybeUninit};

/// A terminal window.
#[derive(Clone, Debug)]
pub struct Window {
    /// Device pixel ratio
    pub dpi: f32,
    /// Size of a terminal cell in pixels
    pub scale: Size<f32>,
    /// Size of the termina window in cells
    pub cells: Size,
    /// Size of the browser window in pixels
    pub browser: Size,
    /// Full terminal pixel geometry for graphics output
    pub graphics_px: Size,
    /// Device scale factor used by Chromium (integer preferred)
    pub dsf: f32,
    /// Command line arguments
    pub cmd: CommandLine,
}

impl Window {
    /// Read the window
    pub fn read() -> Window {
        let mut window = Self {
            dpi: 1.0,
            scale: (0.0, 0.0).into(),
            cells: (0, 0).into(),
            browser: (0, 0).into(),
            graphics_px: (0, 0).into(),
            dsf: 1.0,
            cmd: CommandLine::parse(),
        };

        window.update();

        window
    }

    pub fn update(&mut self) -> &Self {
        // Use crossterm for cross-platform terminal size detection
        let mut term = match terminal::size() {
            Ok((cols, rows)) => Size::new(cols, rows),
            Err(_) => Size::splat(0),
        };
        
        // Pixel dimensions aren't available through crossterm on all platforms
        // Fall back to query_cell_geometry() or defaults
        let cell = Size::splat(0);

        if term.width == 0 || term.height == 0 {
            let cols = match parse_var("COLUMNS").unwrap_or(0) {
                0 => 80,
                x => x,
            };
            let rows = match parse_var("LINES").unwrap_or(0) {
                0 => 24,
                x => x,
            };

            log::warning!(
                "TIOCGWINSZ returned an empty size ({}x{}), defaulting to {}x{}",
                term.width,
                term.height,
                cols,
                rows
            );

            term.width = cols;
            term.height = rows;
        }

        let mut cell_pixels =
            if term.width > 0 && term.height > 0 && cell.width > 0 && cell.height > 0 {
                Size::new(
                    cell.width as f32 / term.width.max(1) as f32,
                    cell.height as f32 / term.height.max(1) as f32,
                )
            } else {
                Size::new(0.0, 0.0)
            };

        if cell_pixels.width <= 0.0 || cell_pixels.height <= 0.0 {
            if let Some(win_px) = query_window_pixels() {
                cell_pixels = Size::new(
                    win_px.width / term.width.max(1) as f32,
                    win_px.height / term.height.max(1) as f32,
                );
            }

            if cell_pixels.width <= 0.0 || cell_pixels.height <= 0.0 {
                cell_pixels = query_cell_geometry().unwrap_or(Size::new(8.0, 16.0));
            }
        }
        // Normalize the cells dimensions for an aspect ratio of 1:2
        self.scale = cell_pixels;
        // Keep some space for the UI
        self.cells = Size::new(term.width.max(1), term.height.max(2) - 1).cast();
        self.graphics_px = Size::new(
            (self.cells.width as f32 * cell_pixels.width).round() as u32,
            (self.cells.height as f32 * cell_pixels.height).round() as u32,
        );

        // Choose an integer device scale factor (DSF) to avoid fractional raster scaling.
        let mut dsf = if cell_pixels.height >= 18.0 { 2.0 } else { 1.0 };
        if let Ok(value) = std::env::var("CARBONYL_DSF") {
            match value.trim() {
                "1" => dsf = 1.0,
                "2" => dsf = 2.0,
                "3" => dsf = 3.0,
                _ => {}
            }
        }
        self.dsf = dsf;
        self.dpi = self.dsf;

        self.browser = Size::new(
            (self.graphics_px.width as f32 / self.dsf).round() as u32,
            (self.graphics_px.height as f32 / self.dsf).round() as u32,
        );

        self
    }
}

fn parse_var<T: FromStr>(var: &str) -> Option<T> {
    std::env::var(var).ok()?.parse().ok()
}

#[cfg(unix)]
fn query_cell_geometry() -> Option<Size<f32>> {
    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let fd = tty.as_raw_fd();
    let mut term = MaybeUninit::<libc::termios>::uninit();

    unsafe {
        // SAFETY: fd is a valid file descriptor from the opened TTY file.
        // tcgetattr writes a termios struct to the provided pointer, which is safe
        // because we've allocated space via MaybeUninit. We check the return value
        // before using the data.
        if libc::tcgetattr(fd, term.as_mut_ptr()) != 0 {
            return None;
        }
    }

    // SAFETY: tcgetattr succeeded (returned 0), so the termios struct has been
    // properly initialized and can be safely read.
    let original = unsafe { term.assume_init() };
    let mut raw = original;
    let c_oflag = raw.c_oflag;

    unsafe {
        // SAFETY: cfmakeraw modifies the termios struct in place. The struct is valid
        // because we just initialized it from tcgetattr. This function performs bitwise
        // operations on the struct fields and is safe to call on valid termios structs.
        libc::cfmakeraw(&mut raw);
    }

    raw.c_oflag = c_oflag;

    // SAFETY: fd is still a valid file descriptor, and raw is a valid termios struct.
    // tcsetattr applies the terminal settings. TCSANOW means apply immediately.
    // This is safe because the struct contains valid terminal configuration.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
        return None;
    }

    struct Restore(libc::c_int, libc::termios);

    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe {
                // SAFETY: self.0 is the file descriptor saved during construction,
                // and self.1 is the original termios struct. Both are valid as they
                // were obtained successfully earlier. We restore the original terminal
                // settings to ensure cleanup even if the function exits early.
                libc::tcsetattr(self.0, libc::TCSANOW, &self.1);
            }
        }
    }

    let _restore = Restore(fd, original);

    if tty.write_all(b"\x1b[16t").is_err() || tty.flush().is_err() {
        return None;
    }

    let mut buffer = [0u8; 128];
    let mut length = 0usize;
    let deadline = Instant::now() + Duration::from_millis(100);

    while length < buffer.len() && Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout = remaining.as_millis().min(i32::MAX as u128) as libc::c_int;
        let mut fds = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };

        // SAFETY: poll is called with a valid pollfd struct and proper count (1).
        // The file descriptor in fds.fd is valid (from the opened TTY).
        // poll will block for at most timeout milliseconds waiting for input.
        // This is safe because we're only reading the return value and checking revents.
        let result = unsafe { libc::poll(&mut fds, 1, timeout) };

        if result <= 0 {
            break;
        }

        match tty.read(&mut buffer[length..]) {
            Ok(0) => break,
            Ok(read) => {
                length += read;

                if buffer[..length].contains(&b't') {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    if length == 0 {
        return None;
    }

    let response = std::str::from_utf8(&buffer[..length]).ok()?;
    let start = response.rfind("\u{1b}[6;")?;
    let rest = &response[start + 3..];
    let end = rest.find('t')?;
    let mut parts = rest[..end].split(';');
    let height = parts.next()?.parse::<f32>().ok()?;
    let width = parts.next()?.parse::<f32>().ok()?;

    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    Some(Size::new(width, height))
}

#[cfg(not(unix))]
fn query_cell_geometry() -> Option<Size<f32>> {
    None
}

#[cfg(unix)]
fn query_window_pixels() -> Option<Size<f32>> {
    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;

    tty.write_all(b"\x1b[14t").ok()?;
    tty.flush().ok()?;

    let mut buf = [0u8; 128];
    let n = tty.read(&mut buf).ok()?;

    if n == 0 {
        return None;
    }

    let response = std::str::from_utf8(&buf[..n]).ok()?;
    let start = response.rfind("\u{1b}[4;")?;
    let rest = &response[start + 3..];
    let end = rest.find('t')?;
    let mut parts = rest[..end].split(';');
    let height = parts.next()?.parse::<f32>().ok()?;
    let width = parts.next()?.parse::<f32>().ok()?;

    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    Some(Size::new(width, height))
}

#[cfg(not(unix))]
fn query_window_pixels() -> Option<Size<f32>> {
    None
}
