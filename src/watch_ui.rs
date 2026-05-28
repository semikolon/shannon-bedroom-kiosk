//! Watch UI — native Bevy mirror of spela web remote's library browse.
//!
//! Polls spela's `/library` endpoint in a background thread; UI reads the
//! shared snapshot every Bevy tick. Mirrors the `spawn_sarpetorp_poller`
//! pattern in `main.rs`.
//!
//! Phase 1 (this version): text list of library titles. Phases 2-5 (posters,
//! now-playing, scrubber, search) deferred — see
//! `~/dotfiles/docs/shannon_watch_ui_design_2026_05_24.md`.
//!
//! Why direct here instead of in main.rs: main.rs is ~3000 lines; keeping
//! the poller + types in a sibling module follows the same separation as
//! `ha.rs` and keeps the diff to main.rs minimal.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bevy::prelude::Resource;
use serde_json::Value;

/// One library entry (subset of spela's `/library` response). Schema is
/// reverse-engineered from `~/Projects/spela/static/remote.html`'s
/// rendering code — we only need the fields used by Phase 1 + 2.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // poster_url/year/kind consumed by Posters slice (Phase 2)
pub struct LibraryEntry {
    /// Canonical filesystem-derived title; pass as `/play` body's `title`.
    pub raw_name: String,
    /// Human-readable (often `raw_name` cleaned up). Falls back to raw_name.
    pub display_name: String,
    pub poster_url: Option<String>,
    pub year: Option<u32>,
    /// "movie" | "tv" | similar. Optional — drives icon choice in future.
    pub kind: Option<String>,
}

/// Snapshot of spela's library at last successful poll. UI reads on each
/// Bevy tick. `last_error` exposed so the render can show a status line
/// when polling fails.
#[derive(Debug, Clone, Default)]
pub struct LibrarySnapshot {
    pub entries: Vec<LibraryEntry>,
    pub fetched_at: Option<Instant>,
    pub last_error: Option<String>,
}

#[derive(Resource, Clone)]
pub struct LibrarySnapshotRes(pub Arc<Mutex<LibrarySnapshot>>);

/// Extract a `LibraryEntry` from an arbitrary serde_json::Value object.
/// Tolerant parsing — any field that's missing, wrong-typed, or unparseable
/// silently defaults. This way one weird library entry doesn't break the
/// whole poll. (The previous strict serde::Deserialize approach hit
/// "error decoding response body" 2026-05-24 when spela's /library
/// returned an unexpected shape; switching to Value-based extraction
/// avoids that brittleness.)
fn extract_entry(v: &Value) -> Option<LibraryEntry> {
    let obj = v.as_object()?;
    let raw_name = obj
        .get("raw_name")
        .or_else(|| obj.get("name"))
        .or_else(|| obj.get("title"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if raw_name.is_empty() {
        return None;
    }
    let display_name = obj
        .get("display_name")
        .or_else(|| obj.get("displayName"))
        .or_else(|| obj.get("title")) // spela's "title" often differs from raw_name (cleaned)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| raw_name.clone());
    let poster_url = obj
        .get("poster_url")
        .or_else(|| obj.get("posterUrl"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let year = obj.get("year").and_then(|x| {
        x.as_u64()
            .map(|n| n as u32)
            .or_else(|| x.as_str().and_then(|s| s.parse::<u32>().ok()))
    });
    let kind = obj
        .get("kind")
        .or_else(|| obj.get("type"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    Some(LibraryEntry {
        raw_name,
        display_name,
        poster_url,
        year,
        kind,
    })
}

/// Spawn a background poller that fetches `<spela_base>/library` every
/// `interval`. Returns the shared snapshot resource.
///
/// Failure handling: on any error (network, JSON parse, etc.), records
/// the error string in `last_error` and KEEPS the previous `entries` so
/// the UI doesn't suddenly go blank on a transient blip.
pub fn spawn_library_poller(spela_base_url: String, interval: Duration) -> LibrarySnapshotRes {
    let snap = Arc::new(Mutex::new(LibrarySnapshot::default()));
    let snap_clone = snap.clone();
    let base = spela_base_url.trim_end_matches('/').to_string();
    thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .ok();
        loop {
            let result = poll_once(&base, client.as_ref());
            {
                let mut s = snap_clone.lock().unwrap();
                match result {
                    Ok(entries) => {
                        s.entries = entries;
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
    LibrarySnapshotRes(snap)
}

fn poll_once(
    base: &str,
    client: Option<&reqwest::blocking::Client>,
) -> Result<Vec<LibraryEntry>, String> {
    let url = format!("{}/library", base);
    let resp = match client {
        Some(c) => c.get(&url).send(),
        None => reqwest::blocking::get(&url),
    };
    let resp = resp.map_err(|e| format!("get {}: {}", url, e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("get {}: http {}", url, status.as_u16()));
    }
    // Tolerant parse — Value first, then extract per-entry. Handles both
    // bare-array and wrapped-object responses (spela returns `{"library":
    // [...]}` today but the format has evolved over time).
    let parsed: Value = resp.json().map_err(|e| format!("parse {}: {}", url, e))?;
    let array = if parsed.is_array() {
        parsed.as_array().cloned().unwrap_or_default()
    } else if let Some(obj) = parsed.as_object() {
        // Try common wrapper keys
        for key in &["library", "entries", "items", "results"] {
            if let Some(arr) = obj.get(*key).and_then(|v| v.as_array()) {
                return Ok(arr.iter().filter_map(extract_entry).collect());
            }
        }
        return Err(format!(
            "parse {}: object with no known wrapper key (keys={:?})",
            url,
            obj.keys().collect::<Vec<_>>()
        ));
    } else {
        return Err(format!("parse {}: not an array or object", url));
    };
    Ok(array.iter().filter_map(extract_entry).collect())
}

/// Visible-slot count for the Watch list view. Sized to fit the
/// kiosk's UI height at the standard font; entries beyond this scroll
/// (cursor stays in middle 1/3 when possible).
pub const WATCH_VISIBLE_SLOTS: usize = 10;

/// Window the library entries to `WATCH_VISIBLE_SLOTS` centered around
/// the cursor. Returns (start_index, visible_entries_slice_indices).
pub fn window_around(cursor: usize, total: usize) -> (usize, Vec<usize>) {
    if total == 0 {
        return (0, vec![]);
    }
    let half = WATCH_VISIBLE_SLOTS / 2;
    let start = if total <= WATCH_VISIBLE_SLOTS || cursor < half {
        // Either the entire list fits inside one window, or the cursor
        // is in the top `half` so the window starts at the top.
        0
    } else if cursor + (WATCH_VISIBLE_SLOTS - half) > total {
        total.saturating_sub(WATCH_VISIBLE_SLOTS)
    } else {
        cursor - half
    };
    let end = (start + WATCH_VISIBLE_SLOTS).min(total);
    (start, (start..end).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_empty() {
        let (start, idx) = window_around(0, 0);
        assert_eq!(start, 0);
        assert!(idx.is_empty());
    }

    #[test]
    fn window_short_list() {
        let (start, idx) = window_around(2, 5);
        assert_eq!(start, 0);
        assert_eq!(idx, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn window_centered() {
        let (start, idx) = window_around(20, 50);
        assert_eq!(start, 15);
        assert_eq!(idx.len(), 10);
        assert_eq!(idx[0], 15);
        assert_eq!(idx[9], 24);
    }

    #[test]
    fn window_at_start() {
        let (start, idx) = window_around(2, 50);
        assert_eq!(start, 0);
        assert_eq!(idx, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn window_at_end() {
        let (start, idx) = window_around(48, 50);
        assert_eq!(start, 40);
        assert_eq!(idx, (40..50).collect::<Vec<_>>());
    }
}
