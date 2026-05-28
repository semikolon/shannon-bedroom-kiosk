//! Home Assistant **state polling** (Slice 3d) — pure parsing, std-only.
//!
//! The kiosk needs real-time HA state for:
//!   - `media_player.tv` (playing / paused / idle / off) — feeds
//!     `Media::Video` into the engine + drives the ribbon-offer for
//!     resume-last-watched (Slice 3e).
//!   - `binary_sensor.bedroom_occupancy` (presence-service via HA) —
//!     supplementary presence signal beyond the controller-BT oracle.
//!
//! This module is the **parsing + state-cache** layer; the live
//! reqwest poll loop lives in the `shannon-kiosk-actions` daemon binary
//! (the same pure-core / thin-edge split as `ha.rs` and `context.rs`).
//! Every signal is injected; every function is a pure transform; tests
//! run on Mac Mini with no live HA.
//!
//! HA REST shape (from `/api/states/<entity_id>`):
//!   ```json
//!   {
//!     "entity_id": "media_player.tv",
//!     "state": "playing",
//!     "attributes": {
//!       "media_title": "Some Episode",
//!       "media_content_type": "movie",
//!       "media_position": 1234,
//!       "media_duration": 3600,
//!       "app_name": "Netflix"
//!     }
//!   }
//!   ```

use crate::context::Media;
use std::time::{Duration, SystemTime};

/// One snapshot of an HA media_player entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPlayerState {
    /// The HA state field — `"playing"` / `"paused"` / `"idle"` / `"off"`
    /// / `"on"` / `"unknown"` / `"unavailable"`.
    pub state: String,
    pub media_title: Option<String>,
    pub media_content_type: Option<String>,
    /// Optional position + duration (seconds) — drives resume-offer text.
    pub media_position: Option<u32>,
    pub media_duration: Option<u32>,
    pub app_name: Option<String>,
}

/// One snapshot of an HA binary_sensor (`"on"` = detected, `"off"` = clear).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinarySensorState {
    On,
    Off,
    /// `unknown` / `unavailable` — sensor unreachable; treat as no signal.
    Unknown,
}

/// Aggregated HA poll-state cache. The daemon's background task refreshes
/// this every N seconds; the Bevy consumer reads a snapshot via the
/// `/ha-state` endpoint.
#[derive(Debug, Clone, Default)]
pub struct HaPollState {
    pub media_player: Option<MediaPlayerState>,
    pub occupancy: Option<BinarySensorState>,
    pub last_poll_at: Option<SystemTime>,
    pub last_poll_result: PollResult,
    /// Per-entity error counts (resets on success). Useful for backoff
    /// decisions + observability.
    pub consecutive_failures: u32,
}

/// One poll outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PollResult {
    #[default]
    NeverPolled,
    Ok,
    NetworkError,
    /// Got a response but couldn't parse the JSON shape.
    ParseError,
    /// Entity not found (404) — usually a config issue.
    EntityNotFound,
}

impl HaPollState {
    /// Best-effort translation to the engine's `Media` enum. Returns
    /// `Media::None` whenever the HA media_player is missing, off,
    /// unavailable, unknown, or paused/idle (the engine treats those as
    /// "no active content" — Content state is for ACTIVE playback only).
    #[must_use]
    pub fn engine_media(&self) -> Media {
        let Some(mp) = self.media_player.as_ref() else {
            return Media::None;
        };
        match mp.state.as_str() {
            // "playing" is the only state that signals active content
            // to the engine. Paused content is in-progress but the room
            // isn't actively engaged — Ambient/Kiosk should still apply.
            "playing" => match mp.media_content_type.as_deref() {
                Some("music") | Some("audio") => Media::Music,
                Some("game") => Media::Game,
                // Default to Video for "movie" | "tvshow" | "video" |
                // None (most casters report no content_type, but a
                // "playing" state means we're consuming content).
                _ => Media::Video,
            },
            _ => Media::None,
        }
    }

    /// Should the ribbon offer "Resume {title}" — yes only when paused
    /// (or "idle") with a title and last position. Playing already has
    /// the screen busy; off/unknown have nothing to resume.
    #[must_use]
    pub fn resumable_title(&self) -> Option<&str> {
        let mp = self.media_player.as_ref()?;
        match mp.state.as_str() {
            "paused" | "idle" => mp.media_title.as_deref().filter(|s| !s.is_empty()),
            _ => None,
        }
    }

    /// True iff the occupancy sensor reports `On` (someone present).
    /// Unknown / Off / no-sensor → false (avoid false positives).
    #[must_use]
    pub fn occupancy_present(&self) -> bool {
        matches!(self.occupancy, Some(BinarySensorState::On))
    }

    /// Is the cache fresh enough to trust? `max_age` is the staleness
    /// threshold. Returns false if we've never polled or the last poll
    /// is older than `max_age`.
    #[must_use]
    pub fn is_fresh(&self, now: SystemTime, max_age: Duration) -> bool {
        match self.last_poll_at {
            Some(t) => now.duration_since(t).map(|d| d < max_age).unwrap_or(true),
            None => false,
        }
    }
}

/// Lightweight string-state mapper used by the daemon's polling task
/// to convert the raw `state` field from HA into `BinarySensorState`
/// without needing a serde_json dep in the lib crate.
#[must_use]
pub fn binary_sensor_from_str(state: &str) -> BinarySensorState {
    match state {
        "on" => BinarySensorState::On,
        "off" => BinarySensorState::Off,
        _ => BinarySensorState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_750_000_000)
    }

    #[test]
    fn engine_media_none_when_no_media_player() {
        let s = HaPollState::default();
        assert_eq!(s.engine_media(), Media::None);
    }

    #[test]
    fn engine_media_video_when_playing() {
        let s = HaPollState {
            media_player: Some(MediaPlayerState {
                state: "playing".into(),
                media_title: Some("Some Show".into()),
                media_content_type: Some("tvshow".into()),
                media_position: Some(120),
                media_duration: Some(2400),
                app_name: Some("Netflix".into()),
            }),
            ..Default::default()
        };
        assert_eq!(s.engine_media(), Media::Video);
    }

    #[test]
    fn engine_media_music_for_audio_content_type() {
        let s = HaPollState {
            media_player: Some(MediaPlayerState {
                state: "playing".into(),
                media_title: None,
                media_content_type: Some("music".into()),
                media_position: None,
                media_duration: None,
                app_name: Some("Spotify".into()),
            }),
            ..Default::default()
        };
        assert_eq!(s.engine_media(), Media::Music);
    }

    #[test]
    fn engine_media_video_when_no_content_type_but_playing() {
        // Most casters omit content_type. Playing = consuming content =
        // Video (the safe default — Video drives the engine into Content
        // state which protects the room from auto-OFF).
        let s = HaPollState {
            media_player: Some(MediaPlayerState {
                state: "playing".into(),
                media_title: Some("Untitled".into()),
                media_content_type: None,
                media_position: None,
                media_duration: None,
                app_name: None,
            }),
            ..Default::default()
        };
        assert_eq!(s.engine_media(), Media::Video);
    }

    #[test]
    fn engine_media_none_for_paused_idle_off_unknown() {
        for state in &["paused", "idle", "off", "on", "unknown", "unavailable", ""] {
            let s = HaPollState {
                media_player: Some(MediaPlayerState {
                    state: (*state).into(),
                    media_title: Some("X".into()),
                    media_content_type: Some("tvshow".into()),
                    media_position: None,
                    media_duration: None,
                    app_name: None,
                }),
                ..Default::default()
            };
            assert_eq!(
                s.engine_media(),
                Media::None,
                "state {state:?} should map to Media::None"
            );
        }
    }

    #[test]
    fn resumable_title_only_when_paused_or_idle_with_title() {
        // paused with title: yes
        let s = HaPollState {
            media_player: Some(MediaPlayerState {
                state: "paused".into(),
                media_title: Some("Awesome Show".into()),
                media_content_type: None,
                media_position: Some(300),
                media_duration: Some(3600),
                app_name: None,
            }),
            ..Default::default()
        };
        assert_eq!(s.resumable_title(), Some("Awesome Show"));

        // idle with title: yes
        let s2 = HaPollState {
            media_player: Some(MediaPlayerState {
                state: "idle".into(),
                media_title: Some("Other Show".into()),
                ..s.media_player.clone().unwrap()
            }),
            ..Default::default()
        };
        assert_eq!(s2.resumable_title(), Some("Other Show"));

        // playing: no — already on, nothing to offer
        let s3 = HaPollState {
            media_player: Some(MediaPlayerState {
                state: "playing".into(),
                ..s.media_player.clone().unwrap()
            }),
            ..Default::default()
        };
        assert_eq!(s3.resumable_title(), None);

        // off: no
        let s4 = HaPollState {
            media_player: Some(MediaPlayerState {
                state: "off".into(),
                ..s.media_player.clone().unwrap()
            }),
            ..Default::default()
        };
        assert_eq!(s4.resumable_title(), None);

        // paused but no title: no — can't offer "Resume <nothing>"
        let s5 = HaPollState {
            media_player: Some(MediaPlayerState {
                state: "paused".into(),
                media_title: None,
                ..s.media_player.clone().unwrap()
            }),
            ..Default::default()
        };
        assert_eq!(s5.resumable_title(), None);

        // paused but empty title: no (treated same as missing)
        let s6 = HaPollState {
            media_player: Some(MediaPlayerState {
                state: "paused".into(),
                media_title: Some("".into()),
                ..s.media_player.clone().unwrap()
            }),
            ..Default::default()
        };
        assert_eq!(s6.resumable_title(), None);
    }

    #[test]
    fn occupancy_present_only_on() {
        let mut s = HaPollState::default();
        assert!(!s.occupancy_present(), "no sensor → not present");
        s.occupancy = Some(BinarySensorState::Off);
        assert!(!s.occupancy_present(), "Off → not present");
        s.occupancy = Some(BinarySensorState::Unknown);
        assert!(!s.occupancy_present(), "Unknown → not present");
        s.occupancy = Some(BinarySensorState::On);
        assert!(s.occupancy_present(), "On → present");
    }

    #[test]
    fn is_fresh_threshold() {
        let t = now();
        let s_never = HaPollState::default();
        assert!(!s_never.is_fresh(t, Duration::from_secs(30)));

        let s_just_polled = HaPollState {
            last_poll_at: Some(t),
            ..Default::default()
        };
        assert!(s_just_polled.is_fresh(t, Duration::from_secs(30)));
        assert!(s_just_polled.is_fresh(t + Duration::from_secs(29), Duration::from_secs(30)));
        assert!(!s_just_polled.is_fresh(t + Duration::from_secs(30), Duration::from_secs(30)));
        assert!(!s_just_polled.is_fresh(t + Duration::from_secs(60), Duration::from_secs(30)));
    }

    #[test]
    fn default_poll_result_never_polled() {
        let s = HaPollState::default();
        assert_eq!(s.last_poll_result, PollResult::NeverPolled);
        assert_eq!(s.consecutive_failures, 0);
        assert!(s.last_poll_at.is_none());
    }

    #[test]
    fn binary_sensor_from_str_maps_known_states() {
        assert_eq!(binary_sensor_from_str("on"), BinarySensorState::On);
        assert_eq!(binary_sensor_from_str("off"), BinarySensorState::Off);
        assert_eq!(
            binary_sensor_from_str("unknown"),
            BinarySensorState::Unknown
        );
        assert_eq!(
            binary_sensor_from_str("unavailable"),
            BinarySensorState::Unknown
        );
        assert_eq!(binary_sensor_from_str(""), BinarySensorState::Unknown);
        // Case-sensitive: HA always returns lowercase, but document the
        // contract for callers.
        assert_eq!(binary_sensor_from_str("ON"), BinarySensorState::Unknown);
    }
}
