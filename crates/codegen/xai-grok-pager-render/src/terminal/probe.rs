//! Shared startup terminal-probe primitive: write a query, and (OSC 11 only) raw-fd poll/read stdin until a terminator or deadline.
//! XTVERSION uses only `write_query`; its reply is handled by the event loop's response filter.
//!
//! Safety invariants (timed-read path):
//! - Startup-only: must run before crossterm's `EventStream` exists (both compete for stdin).
//! - Keystrokes typed inside the read window are consumed and dropped; no portable re-injection exists (TIOCSTI is blocked), an accepted loss.

use std::io::Write;
use std::time::Duration;

/// Bounds the reply buffer against terminals that stream without a terminator.
#[cfg(unix)]
pub(crate) const MAX_PROBE_RESPONSE: usize = 256;

/// Hard cap on post-deadline consumption of an in-flight reply.
#[cfg(unix)]
const LATE_REPLY_GRACE: Duration = Duration::from_millis(100);

/// Per-byte quiet window during the grace period.
#[cfg(unix)]
const LATE_REPLY_QUIET_MS: i32 = 25;

/// CSI 6n (Device Status Report — cursor position). Crossterm's
/// `cursor::position()` uses the same query with a hard-coded **2 second**
/// poll timeout; under hosts that never answer (dtach without an attached
/// client, CI PTYs, `script(1)`), that stalls TUI first-paint by the full
/// budget. Call [`cursor_position_responsive`] before constructing an
/// `Inline` viewport so we can fall back to `Fixed` within a short window.
pub const CURSOR_POSITION_QUERY: &[u8] = b"\x1b[6n";

/// Default budget for the snappy preflight. Long enough for a real terminal
/// (local PTY round-trip is typically under 10 ms) and short enough that
/// headless/dtach launches stay under the 1 s first-paint target.
pub const CURSOR_POSITION_PREFLIGHT_TIMEOUT: Duration = Duration::from_millis(100);

/// Returns `true` when the terminal answers CSI 6n within `timeout`.
///
/// Startup-only (must run before crossterm's input reader owns stdin). On
/// non-unix platforms always returns `true` so callers keep the existing
/// Inline path (Windows cursor position is not a CSI round-trip).
pub fn cursor_position_responsive(timeout: Duration) -> bool {
    #[cfg(unix)]
    {
        if !write_query(CURSOR_POSITION_QUERY) {
            return false;
        }
        let Some(buf) = read_tty_reply(timeout, is_cursor_position_reply) else {
            return false;
        };
        is_cursor_position_complete(&buf)
    }
    #[cfg(not(unix))]
    {
        let _ = timeout;
        true
    }
}

/// CSI `…R` terminator for cursor-position replies (`ESC [ row ; col R`).
#[cfg(unix)]
fn is_cursor_position_reply(buf: &[u8], byte: u8) -> bool {
    byte == b'R' && buf.windows(2).any(|w| w == b"\x1b[")
}

#[cfg(unix)]
fn is_cursor_position_complete(buf: &[u8]) -> bool {
    // Minimal well-formed reply: ESC [ digits ; digits R
    let s = std::str::from_utf8(buf).unwrap_or("");
    let Some(rest) = s.strip_prefix("\u{1b}[") else {
        return false;
    };
    let Some(body) = rest.strip_suffix('R') else {
        return false;
    };
    let mut parts = body.split(';');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(r), Some(c), None) if !r.is_empty() && !c.is_empty()
            && r.bytes().all(|b| b.is_ascii_digit())
            && c.bytes().all(|b| b.is_ascii_digit())
    )
}

/// Write a probe query via the shared stderr lock; `false` if the TUI fd is not a TTY or the write fails.
pub(crate) fn write_query(query: &[u8]) -> bool {
    use std::io::IsTerminal;

    let write_result: std::io::Result<()> = xai_grok_shared::stderr::with_locked_stderr(|stderr| {
        // fd 2 is /dev/null-redirected; the TTY check must run on the dup'd render fd inside the lock, not on std::io::stderr()
        if !stderr.is_terminal() {
            return Err(std::io::Error::other("TUI output is not a TTY"));
        }
        stderr.write_all(query)?;
        stderr.flush()
    });
    write_result.is_ok()
}

/// Read stdin until `is_terminated`, the size cap, or the deadline.
///
/// Returns `Some(buf)` whenever bytes were consumed (even partial, so a half-read reply is never left for the EventStream).
/// Returns `None` when nothing arrived or stdin errored before any byte.
#[cfg(unix)]
pub(crate) fn read_tty_reply(
    timeout: Duration,
    mut is_terminated: impl FnMut(&[u8], u8) -> bool,
) -> Option<Vec<u8>> {
    use std::os::unix::io::AsRawFd;

    let fd = std::io::stdin().as_raw_fd();
    let start = std::time::Instant::now();
    let mut buf: Vec<u8> = Vec::with_capacity(64);

    loop {
        let Some(remaining) = timeout.checked_sub(start.elapsed()) else {
            return finish_after_deadline(fd, buf, is_terminated);
        };
        let remaining_ms = remaining.as_millis().min(i32::MAX as u128) as i32;

        match poll_read_byte(fd, remaining_ms) {
            PollRead::Byte(byte) => {
                buf.push(byte);
                if buf.len() >= MAX_PROBE_RESPONSE || is_terminated(&buf, byte) {
                    return Some(buf);
                }
            }
            // Re-entry recomputes the deadline, so EINTR cannot extend it.
            PollRead::Interrupted => continue,
            PollRead::Timeout => return finish_after_deadline(fd, buf, is_terminated),
            PollRead::Error => return if buf.is_empty() { None } else { Some(buf) },
        }
    }
}

/// Deadline expiry: an in-flight reply (ESC byte seen; replies are DCS/CSI/OSC, plain keystrokes aren't) is consumed until quiet.
/// That keeps its tail from reaching the EventStream as typed garbage.
/// Otherwise return immediately to avoid eating keystrokes at a silent terminal.
#[cfg(unix)]
fn finish_after_deadline(
    fd: i32,
    mut buf: Vec<u8>,
    mut is_terminated: impl FnMut(&[u8], u8) -> bool,
) -> Option<Vec<u8>> {
    if buf.is_empty() {
        return None;
    }
    if !buf.contains(&0x1b) {
        return Some(buf);
    }
    let grace_start = std::time::Instant::now();
    while grace_start.elapsed() < LATE_REPLY_GRACE {
        match poll_read_byte(fd, LATE_REPLY_QUIET_MS) {
            PollRead::Byte(byte) => {
                buf.push(byte);
                if buf.len() >= MAX_PROBE_RESPONSE || is_terminated(&buf, byte) {
                    break;
                }
            }
            PollRead::Interrupted => continue,
            PollRead::Timeout | PollRead::Error => break,
        }
    }
    Some(buf)
}

#[cfg(unix)]
enum PollRead {
    Byte(u8),
    Interrupted,
    Timeout,
    Error,
}

/// One EINTR-retrying poll-then-read step for a single byte.
#[cfg(unix)]
fn poll_read_byte(fd: i32, timeout_ms: i32) -> PollRead {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: pfd is a valid pollfd struct with a valid fd.
    let ret = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    if ret == 0 {
        return PollRead::Timeout;
    }
    if ret < 0 {
        return if last_errno_is_eintr() {
            PollRead::Interrupted
        } else {
            PollRead::Error
        };
    }

    loop {
        let mut byte = [0u8; 1];
        // SAFETY: byte is a valid buffer of length 1.
        let n = unsafe { libc::read(fd, byte.as_mut_ptr().cast(), 1) };
        if n == 1 {
            return PollRead::Byte(byte[0]);
        }
        if n < 0 && last_errno_is_eintr() {
            continue;
        }
        return PollRead::Error;
    }
}

#[cfg(unix)]
fn last_errno_is_eintr() -> bool {
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn cursor_position_complete_accepts_well_formed_dsr() {
        assert!(is_cursor_position_complete(b"\x1b[20;1R"));
        assert!(is_cursor_position_complete(b"\x1b[1;1R"));
        assert!(is_cursor_position_complete(b"\x1b[999;80R"));
    }

    #[test]
    fn cursor_position_complete_rejects_garbage() {
        assert!(!is_cursor_position_complete(b""));
        assert!(!is_cursor_position_complete(b"\x1b[6n"));
        assert!(!is_cursor_position_complete(b"\x1b[R"));
        assert!(!is_cursor_position_complete(b"\x1b[20R"));
        assert!(!is_cursor_position_complete(b"hello"));
        assert!(!is_cursor_position_complete(b"\x1b]11;?\x07"));
    }

    #[test]
    fn cursor_position_reply_terminator_requires_csi_prefix() {
        assert!(is_cursor_position_reply(b"\x1b[20;1R", b'R'));
        // Lone R without CSI must not terminate (could be user keystroke).
        assert!(!is_cursor_position_reply(b"R", b'R'));
        assert!(!is_cursor_position_reply(b"xR", b'R'));
    }
}
