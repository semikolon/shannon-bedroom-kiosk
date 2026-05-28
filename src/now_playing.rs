//! Now-Playing view — Phase-7 Watch UI rank-1 Phase 3 slice. Polls spela's
//! `/status` + `/api/position` endpoints; renders title + play-state +
//! elapsed / total position on a full-screen view that supersedes the
//! WatchSubmenu list while a stream is dispatched.
//!
//! Surfaces (mirrors spela web remote's now-playing card):
//! - Title (from `/status` `.current.title`, truncated to fit)
//! - Play state (idle / streaming / process_dead / starting)
//! - Elapsed / duration (HH:MM:SS via `format_hms`)
//!
//! Lifecycle:
//! - WatchSubmenu A-press → try_dispatch_watch + menu_level=NowPlaying
//! - NowPlaying B-press → menu_level=WatchSubmenu (cursor restored)
//! - Stream stops mid-view: state shows "STOPPED" but view stays until
//!   user presses B (don't surprise-pop the screen — same philosophy as
//!   `paused_seen_in_session` in spela's cast_health_monitor: explicit
//!   user actions trump inferred state).
//!
//! Why a sibling module to `watch_ui.rs`: same separation pattern. The
//! poller cadence here is 2s (vs library's 120s) because position changes
//! constantly; spela's `/api/position` is cheap (no HA round-trip).

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bevy::prelude::Resource;
use serde_json::Value;

/// Snapshot of spela's current play state. Updated by the background
/// poller; read by `now_playing_render_system` every Bevy tick.
#[derive(Debug, Clone, Default)]
pub struct NowPlayingSnapshot {
    /// Spela `/status` `.status` field. Common values:
    /// `"idle"` | `"streaming"` | `"process_dead"`.
    /// `None` until first successful poll.
    pub status: Option<String>,
    /// Title of the currently-playing stream (from `.current.title`).
    /// `None` when status is idle or pre-first-poll.
    pub title: Option<String>,
    /// Current playback position in seconds (from `/api/position` `.t`).
    /// 0.0 until first successful poll OR when no stream is active.
    pub position_secs: f64,
    /// Stream duration in seconds (from `.current.duration`).
    /// `None` when duration is unknown (e.g., live HLS or pre-probe).
    pub duration_secs: Option<f64>,
    /// Optional IMDB id from `/api/position` `.imdb_id`. Used to detect
    /// stream identity changes (future: could trigger title-refresh).
    pub imdb_id: Option<String>,
    /// Wall-clock when this snapshot was last refreshed; lets the render
    /// system interpolate position between polls for smoother UI.
    pub fetched_at: Option<Instant>,
    /// Most recent poll error message, if any. Cleared on next success.
    pub last_error: Option<String>,
}

#[derive(Resource, Clone)]
pub struct NowPlayingSnapshotRes(pub Arc<Mutex<NowPlayingSnapshot>>);

/// Spawn a background poller that fetches `<spela_base>/status` and
/// `<spela_base>/api/position` every `interval`. Returns the shared
/// snapshot resource.
///
/// Failure mode: on transient HTTP/JSON error records the error in
/// `last_error` but KEEPS the previous fields so a brief network blip
/// doesn't blank the now-playing view. On clean success, `last_error`
/// is cleared.
pub fn spawn_now_playing_poller(
    spela_base_url: String,
    interval: Duration,
) -> NowPlayingSnapshotRes {
    let snap = Arc::new(Mutex::new(NowPlayingSnapshot::default()));
    let snap_clone = snap.clone();
    let base = spela_base_url.trim_end_matches('/').to_string();
    thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()
            .ok();
        loop {
            let result = poll_once(&base, client.as_ref());
            {
                let mut s = snap_clone.lock().unwrap();
                match result {
                    Ok(next) => {
                        s.status = next.status;
                        s.title = next.title;
                        s.position_secs = next.position_secs;
                        s.duration_secs = next.duration_secs;
                        s.imdb_id = next.imdb_id;
                        s.fetched_at = Some(Instant::now());
                        s.last_error = None;
                    }
                    Err(e) => {
                        s.last_error = Some(e);
                    }
                }
            }
            thread::sleep(interval);
        }
    });
    NowPlayingSnapshotRes(snap)
}

/// Internal: combined `/status` + `/api/position` poll result.
struct PollResult {
    status: Option<String>,
    title: Option<String>,
    position_secs: f64,
    duration_secs: Option<f64>,
    imdb_id: Option<String>,
}

fn poll_once(base: &str, client: Option<&reqwest::blocking::Client>) -> Result<PollResult, String> {
    // /status — primary source of truth for play-state + title + duration
    let status_url = format!("{}/status", base);
    let status_resp = match client {
        Some(c) => c.get(&status_url).send(),
        None => reqwest::blocking::get(&status_url),
    };
    let status_resp = status_resp.map_err(|e| format!("get {}: {}", status_url, e))?;
    if !status_resp.status().is_success() {
        return Err(format!(
            "get {}: http {}",
            status_url,
            status_resp.status().as_u16()
        ));
    }
    let status_val: Value = status_resp
        .json()
        .map_err(|e| format!("parse {}: {}", status_url, e))?;
    let status_str = status_val
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let (title, duration_secs) = extract_current(&status_val);

    // /api/position — current position; cheap fast endpoint
    let pos_url = format!("{}/api/position", base);
    let pos_resp = match client {
        Some(c) => c.get(&pos_url).send(),
        None => reqwest::blocking::get(&pos_url),
    };
    let pos_resp = pos_resp.map_err(|e| format!("get {}: {}", pos_url, e))?;
    if !pos_resp.status().is_success() {
        return Err(format!(
            "get {}: http {}",
            pos_url,
            pos_resp.status().as_u16()
        ));
    }
    let pos_val: Value = pos_resp
        .json()
        .map_err(|e| format!("parse {}: {}", pos_url, e))?;
    let position_secs = pos_val.get("t").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let imdb_id = pos_val
        .get("imdb_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(PollResult {
        status: status_str,
        title,
        position_secs,
        duration_secs,
        imdb_id,
    })
}

/// Extract title + duration from the `/status` response's nested `.current`
/// object. Tolerant — missing fields return None rather than failing the
/// whole parse (matches `watch_ui::extract_entry`'s philosophy).
pub fn extract_current(status_val: &Value) -> (Option<String>, Option<f64>) {
    let current = match status_val.get("current") {
        Some(c) if c.is_object() => c,
        _ => return (None, None),
    };
    let title = current
        .get("title")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let duration = current.get("duration").and_then(|v| v.as_f64());
    (title, duration)
}

/// Format seconds as `H:MM:SS` for durations >= 1h, `M:SS` otherwise.
/// Handles negative inputs (clamps to 0) + NaN (renders `--:--`).
pub fn format_hms(seconds: f64) -> String {
    if seconds.is_nan() {
        return "--:--".to_string();
    }
    let s = seconds.max(0.0) as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, sec)
    } else {
        format!("{}:{:02}", m, sec)
    }
}

/// Phase 4 — compute the scrubber fill width in pixels given elapsed
/// seconds, total duration, and the track's full pixel width. Returns
/// 0.0 when duration is missing/zero/NaN/negative (we don't know the
/// total length, so the bar is empty). Clamps to [0, track_width_px]
/// for the well-formed case so frame-level interpolation past the end
/// of duration (a poll-cycle race) doesn't overflow the bar.
pub fn scrubber_fill_px(elapsed: f64, duration: Option<f64>, track_width_px: f32) -> f32 {
    let d = match duration {
        Some(d) if d.is_finite() && d > 0.0 => d,
        _ => return 0.0,
    };
    if !elapsed.is_finite() {
        return 0.0;
    }
    let frac = (elapsed / d).clamp(0.0, 1.0);
    (frac as f32) * track_width_px
}

/// User-facing label for the spela `status` field. Falls back to the raw
/// string for unknown values so future spela states surface meaningfully
/// instead of being silently hidden.
pub fn status_label(status: Option<&str>, has_position: bool) -> &'static str {
    match status {
        Some("streaming") => "▶ STREAMING",
        Some("process_dead") => "✕ PROCESS DEAD",
        Some("idle") if has_position => "■ STOPPED",
        Some("idle") => "STARTING…",
        Some(_) => "PLAYING…",
        None => "STARTING…",
    }
}

/// Truncate a long title for the now-playing surface so it fits the
/// 1600 px-wide title row at font_size 64. Empirically ~70 chars at
/// SharpSans Bold; we pad downward for the ellipsis. Matches the pattern
/// used by `pending_watch_overlay_system`'s 56-char overlay truncation
/// (this is wider because NowPlaying has more horizontal real estate
/// than the 1120 px overlay card).
pub fn truncate_title(title: &str, max_chars: usize) -> String {
    if title.chars().count() <= max_chars {
        return title.to_string();
    }
    let mut out: String = title.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_hms_seconds_only() {
        assert_eq!(format_hms(0.0), "0:00");
        assert_eq!(format_hms(7.0), "0:07");
        assert_eq!(format_hms(59.4), "0:59");
    }

    #[test]
    fn format_hms_minutes_seconds() {
        assert_eq!(format_hms(60.0), "1:00");
        assert_eq!(format_hms(125.0), "2:05");
        assert_eq!(format_hms(3599.0), "59:59");
    }

    #[test]
    fn format_hms_hours_minutes_seconds() {
        assert_eq!(format_hms(3600.0), "1:00:00");
        assert_eq!(format_hms(3661.0), "1:01:01");
        assert_eq!(format_hms(7200.0), "2:00:00");
        assert_eq!(format_hms(7325.0), "2:02:05");
    }

    #[test]
    fn format_hms_negative_clamps_to_zero() {
        assert_eq!(format_hms(-5.0), "0:00");
    }

    #[test]
    fn format_hms_nan_renders_placeholder() {
        assert_eq!(format_hms(f64::NAN), "--:--");
    }

    #[test]
    fn status_label_streaming() {
        assert_eq!(status_label(Some("streaming"), false), "▶ STREAMING");
        assert_eq!(status_label(Some("streaming"), true), "▶ STREAMING");
    }

    #[test]
    fn status_label_idle_with_position_means_stopped() {
        // After a stream ended (process_dead → idle), position from last
        // poll is non-zero. We render STOPPED so the user sees the prior
        // session ended cleanly (vs STARTING which would imply a new one).
        assert_eq!(status_label(Some("idle"), true), "■ STOPPED");
    }

    #[test]
    fn status_label_idle_no_position_means_starting() {
        // Fresh dispatch with no prior position: spela's status is still
        // "idle" briefly while the cast pipeline cold-starts.
        assert_eq!(status_label(Some("idle"), false), "STARTING…");
    }

    #[test]
    fn status_label_process_dead() {
        assert_eq!(status_label(Some("process_dead"), true), "✕ PROCESS DEAD");
    }

    #[test]
    fn status_label_none_means_starting() {
        // First polls before any /status response — show STARTING so the
        // user has feedback during cold-start of the poller itself.
        assert_eq!(status_label(None, false), "STARTING…");
    }

    #[test]
    fn truncate_title_short_unchanged() {
        assert_eq!(truncate_title("Aladdin", 70), "Aladdin");
    }

    #[test]
    fn truncate_title_long_truncated_with_ellipsis() {
        let long =
            "Some Really Long Movie Title That Goes On And On And On And On Forever Without End";
        let t = truncate_title(long, 30);
        assert_eq!(t.chars().count(), 30);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn truncate_title_exact_boundary_unchanged() {
        let exact = "x".repeat(30);
        assert_eq!(truncate_title(&exact, 30), exact);
    }

    #[test]
    fn truncate_title_unicode_safe() {
        // Multi-byte chars (Swedish + emoji) shouldn't slice mid-codepoint.
        let title = "Den Tjyvskytten å häxan 🌲 möter Ödön";
        let t = truncate_title(title, 10);
        assert_eq!(t.chars().count(), 10);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn extract_current_idle_response() {
        let v: Value = serde_json::from_str(r#"{"status":"idle"}"#).unwrap();
        assert_eq!(extract_current(&v), (None, None));
    }

    #[test]
    fn extract_current_streaming_response() {
        let v: Value = serde_json::from_str(
            r#"{"status":"streaming","current":{"title":"Aladdin (1992) 1080p.mkv","duration":4581.0,"pid":42}}"#,
        )
        .unwrap();
        let (title, dur) = extract_current(&v);
        assert_eq!(title.as_deref(), Some("Aladdin (1992) 1080p.mkv"));
        assert_eq!(dur, Some(4581.0));
    }

    #[test]
    fn extract_current_missing_duration() {
        // Live HLS or pre-probe: title present, duration absent.
        let v: Value =
            serde_json::from_str(r#"{"status":"streaming","current":{"title":"Foo"}}"#).unwrap();
        let (title, dur) = extract_current(&v);
        assert_eq!(title.as_deref(), Some("Foo"));
        assert_eq!(dur, None);
    }

    #[test]
    fn scrubber_fill_px_no_duration_is_zero() {
        assert_eq!(scrubber_fill_px(120.0, None, 1240.0), 0.0);
    }

    #[test]
    fn scrubber_fill_px_zero_duration_is_zero() {
        assert_eq!(scrubber_fill_px(120.0, Some(0.0), 1240.0), 0.0);
    }

    #[test]
    fn scrubber_fill_px_negative_duration_is_zero() {
        assert_eq!(scrubber_fill_px(120.0, Some(-100.0), 1240.0), 0.0);
    }

    #[test]
    fn scrubber_fill_px_nan_or_inf_is_zero() {
        assert_eq!(scrubber_fill_px(f64::NAN, Some(100.0), 1240.0), 0.0);
        assert_eq!(scrubber_fill_px(120.0, Some(f64::NAN), 1240.0), 0.0);
        assert_eq!(scrubber_fill_px(120.0, Some(f64::INFINITY), 1240.0), 0.0);
    }

    #[test]
    fn scrubber_fill_px_half_is_half_width() {
        let px = scrubber_fill_px(60.0, Some(120.0), 1000.0);
        assert!((px - 500.0).abs() < 0.01);
    }

    #[test]
    fn scrubber_fill_px_full_is_full_width() {
        let px = scrubber_fill_px(120.0, Some(120.0), 1000.0);
        assert!((px - 1000.0).abs() < 0.01);
    }

    #[test]
    fn scrubber_fill_px_overshoot_clamps_to_full_width() {
        // Position interpolation past EOD (poll-cycle race) — fill
        // should not overflow the track. Clamped to track_width.
        let px = scrubber_fill_px(150.0, Some(120.0), 1000.0);
        assert!((px - 1000.0).abs() < 0.01);
    }

    #[test]
    fn scrubber_fill_px_at_start_is_zero() {
        assert_eq!(scrubber_fill_px(0.0, Some(120.0), 1000.0), 0.0);
    }

    #[test]
    fn extract_current_empty_title_falls_back_to_none() {
        let v: Value =
            serde_json::from_str(r#"{"status":"streaming","current":{"title":""}}"#).unwrap();
        let (title, _dur) = extract_current(&v);
        assert_eq!(title, None);
    }
}
