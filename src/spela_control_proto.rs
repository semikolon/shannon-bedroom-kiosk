//! Pure protocol module for the spela-renderer control IPC. Defines the
//! `Command` enum and its line-based parser. Lives in the library (not in
//! `src/bin/spela-renderer.rs`) so the parser is unit-testable on any
//! host, without the gstreamer-1.0 system dependency that the renderer
//! binary needs. Per the lib.rs convention (engine layer = pure + std-
//! only, testable on Mac), this module follows the same pattern.
//!
//! Wire format (line-delimited text over a Unix socket):
//!
//!   seek_relative <signed-seconds>\n  → seek N seconds from current
//!   seek_absolute <seconds>\n         → seek to absolute position
//!   quit\n                            → graceful EOS + shutdown
//!
//! Responses are also line-delimited:
//!
//!   ok pos=<secs>\n                   → command accepted; current pos
//!   err <message>\n                   → bad parse / no pipeline / etc.

/// Control commands the renderer accepts over the IPC socket.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Command {
    /// Seek N seconds from current position; negative = backward. The
    /// renderer clamps the resulting position to [0, duration].
    SeekRelative(i64),
    /// Seek to absolute N seconds from stream start.
    SeekAbsolute(u64),
    /// Toggle play/pause. Phase 4 (2026-05-29): when paused, the
    /// pipeline is in `Paused` GStreamer state — frame is held, audio
    /// is silent, position freezes. Next `PlayPause` resumes.
    PlayPause,
    /// Stop pipeline gracefully + exit.
    Quit,
}

/// Parse a single command line. Returns `Ok(Some(cmd))` on a known
/// command, `Ok(None)` on blank/empty line, `Err` on parse error.
///
/// Tolerant of trailing newlines + multi-whitespace. Unknown verbs
/// produce a descriptive error so the operator can see what was
/// rejected.
pub fn parse_command(line: &str) -> Result<Option<Command>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let mut parts = trimmed.split_whitespace();
    let verb = parts.next().ok_or_else(|| "empty line".to_string())?;
    match verb {
        "seek_relative" => {
            let arg = parts
                .next()
                .ok_or_else(|| "seek_relative: missing seconds arg".to_string())?;
            let n: i64 = arg
                .parse()
                .map_err(|e| format!("seek_relative: parse seconds '{arg}': {e}"))?;
            Ok(Some(Command::SeekRelative(n)))
        }
        "seek_absolute" => {
            let arg = parts
                .next()
                .ok_or_else(|| "seek_absolute: missing seconds arg".to_string())?;
            let n: u64 = arg
                .parse()
                .map_err(|e| format!("seek_absolute: parse seconds '{arg}': {e}"))?;
            Ok(Some(Command::SeekAbsolute(n)))
        }
        "play_pause" => Ok(Some(Command::PlayPause)),
        "quit" => Ok(Some(Command::Quit)),
        other => Err(format!("unknown command '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_seek_relative_positive() {
        assert_eq!(
            parse_command("seek_relative 30").unwrap(),
            Some(Command::SeekRelative(30))
        );
    }

    #[test]
    fn parse_seek_relative_negative() {
        assert_eq!(
            parse_command("seek_relative -30").unwrap(),
            Some(Command::SeekRelative(-30))
        );
    }

    #[test]
    fn parse_seek_relative_trailing_newline() {
        assert_eq!(
            parse_command("seek_relative 15\n").unwrap(),
            Some(Command::SeekRelative(15))
        );
    }

    #[test]
    fn parse_seek_absolute_zero() {
        assert_eq!(
            parse_command("seek_absolute 0").unwrap(),
            Some(Command::SeekAbsolute(0))
        );
    }

    #[test]
    fn parse_seek_absolute_positive() {
        assert_eq!(
            parse_command("seek_absolute 600").unwrap(),
            Some(Command::SeekAbsolute(600))
        );
    }

    #[test]
    fn parse_seek_absolute_negative_rejected() {
        // u64 parse rejects '-'; that's the right behavior — absolute
        // can't be negative. The error must be informative.
        let err = parse_command("seek_absolute -5").unwrap_err();
        assert!(err.contains("seek_absolute"));
    }

    #[test]
    fn parse_quit() {
        assert_eq!(parse_command("quit").unwrap(), Some(Command::Quit));
    }

    #[test]
    fn parse_play_pause() {
        assert_eq!(
            parse_command("play_pause").unwrap(),
            Some(Command::PlayPause)
        );
        assert_eq!(
            parse_command("play_pause\n").unwrap(),
            Some(Command::PlayPause)
        );
    }

    #[test]
    fn parse_empty_is_ignored() {
        assert_eq!(parse_command("").unwrap(), None);
        assert_eq!(parse_command("\n").unwrap(), None);
        assert_eq!(parse_command("   ").unwrap(), None);
    }

    #[test]
    fn parse_unknown_verb_errors() {
        let err = parse_command("dance 5").unwrap_err();
        assert!(err.contains("dance"));
    }

    #[test]
    fn parse_seek_relative_missing_arg_errors() {
        let err = parse_command("seek_relative").unwrap_err();
        assert!(err.contains("seconds"));
    }

    #[test]
    fn parse_seek_relative_bad_arg_errors() {
        let err = parse_command("seek_relative abc").unwrap_err();
        assert!(err.contains("parse seconds"));
    }

    #[test]
    fn parse_extra_whitespace_tolerated() {
        // Single+multiple spaces between verb and arg are both fine.
        assert_eq!(
            parse_command("seek_relative    -10").unwrap(),
            Some(Command::SeekRelative(-10))
        );
    }
}
