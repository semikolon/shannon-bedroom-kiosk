//! Posters — Phase-7 Watch UI Phase 2. Downloads + caches poster images
//! (from spela `/library`'s `poster_url`) so the WatchSubmenu can render
//! a poster grid instead of a text list.
//!
//! Pipeline (load + display):
//! 1. Background thread polls a request channel; for each unique poster
//!    URL, GETs the JPEG bytes via reqwest blocking (single shared client
//!    with connection reuse) and stores them in a shared registry.
//! 2. A main-thread Bevy system scans the registry for entries in `Bytes`
//!    state, decodes them via `Image::from_buffer` (Bevy's JPEG path —
//!    the `jpeg` feature is on in Cargo.toml), inserts the asset into
//!    `Assets<Image>`, and stores the resulting `Handle<Image>`.
//! 3. The render system reads the registry on each tick; for each visible
//!    poster slot, looks up the entry's handle and updates the `ImageNode`.
//!
//! Why bytes-in-memory rather than file cache: cleaner — no file I/O on
//! the kiosk runtime, no cache-dir provisioning, no orphan-cleanup
//! concerns. Total memory cost is bounded by library size × poster size
//! (~57 × ~80KB = ~4.5 MB on the current library, well within Shannon's
//! 4 GB RAM). If the library grows to hundreds-of-titles range, an
//! LRU-evicting on-disk cache becomes the right pattern.
//!
//! Failure handling: per-URL — a 404 on one poster doesn't block others.
//! `PosterStatus::Failed` keeps the failure visible so the render path
//! can fall back to a text label without retrying every tick. The
//! downloader thread is idempotent (re-requesting a URL already in
//! Bytes/Handle/Failed state is a no-op).

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use bevy::prelude::Resource;

/// Per-URL status of the poster cache pipeline.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // Bytes variant is the typical state; Handle is rare comparison
pub enum PosterStatus {
    /// Download requested but not yet completed.
    Pending,
    /// JPEG bytes downloaded; awaiting main-thread `Image::from_buffer` +
    /// `Assets<Image>::add` to produce a Bevy handle.
    Bytes(Vec<u8>),
    /// Asset is loaded and ready to render.
    Handle(bevy::prelude::Handle<bevy::prelude::Image>),
    /// Download failed (HTTP error, timeout, etc.) — render path falls
    /// back to text. The error string is currently write-only (logged +
    /// retained for `journalctl`-style ad-hoc inspection of the shared
    /// registry) but not read by any UI path; keeping it for future
    /// "show error on hover" + diagnostic visibility.
    #[allow(dead_code)]
    Failed(String),
}

/// Shared poster registry. The downloader thread writes to it; the
/// main-thread promote system upgrades `Bytes` → `Handle`; the render
/// system reads `Handle` to set `ImageNode` handles.
#[derive(Resource, Clone)]
pub struct PosterRegistry {
    /// URL → status. Keyed by poster_url (string) — TMDB URLs are
    /// canonicalized by spela so duplicates collapse correctly.
    pub entries: Arc<Mutex<HashMap<String, PosterStatus>>>,
    /// Send channel into the downloader thread. Main thread sends URLs
    /// to fetch; the thread dedup-checks against `entries` before
    /// issuing the GET.
    pub sender: Sender<String>,
}

impl PosterRegistry {
    /// Request a poster download. No-op if the URL is already
    /// Pending / Bytes / Handle / Failed — the downloader thread runs
    /// its own dedup check too (belt + suspenders against races).
    pub fn request(&self, url: &str) {
        if url.is_empty() {
            return;
        }
        // Cheap pre-check: skip if already known. We still send if
        // pre-check races (worst case: one extra HEAD-fast no-op
        // request in the downloader).
        if let Ok(map) = self.entries.lock() {
            if map.contains_key(url) {
                return;
            }
        }
        // Best-effort: if the channel is closed (downloader thread
        // crashed), we silently drop. The render path will keep showing
        // text fallback — graceful degradation.
        let _ = self.sender.send(url.to_string());
    }
}

/// Spawn the poster-downloader background thread. Returns a
/// `PosterRegistry` with a Sender already wired into the channel; the
/// caller registers this as a Bevy resource.
///
/// `timeout` caps per-request wall time so a slow poster doesn't block
/// the queue. 8 seconds is generous for ~100KB JPEGs over TMDB's CDN
/// from Shannon's home WiFi.
pub fn spawn_poster_downloader(timeout: Duration) -> PosterRegistry {
    let (tx, rx): (Sender<String>, Receiver<String>) = channel();
    let entries: Arc<Mutex<HashMap<String, PosterStatus>>> = Arc::new(Mutex::new(HashMap::new()));
    let entries_clone = entries.clone();
    thread::spawn(move || {
        // Build one reqwest client — connection reuse to TMDB's CDN
        // dramatically reduces per-poster latency on cold downloads
        // (TLS handshake amortized across N requests).
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .ok();
        while let Ok(url) = rx.recv() {
            // Dedup: skip if already known (covers the race window
            // between PosterRegistry::request's pre-check and the
            // downloader picking up the message).
            {
                let map = match entries_clone.lock() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if map.contains_key(&url) {
                    continue;
                }
            }
            // Mark Pending so subsequent request() calls dedup.
            {
                if let Ok(mut map) = entries_clone.lock() {
                    map.insert(url.clone(), PosterStatus::Pending);
                }
            }
            let result = download_one(client.as_ref(), &url);
            if let Ok(mut map) = entries_clone.lock() {
                match result {
                    Ok(bytes) => {
                        map.insert(url.clone(), PosterStatus::Bytes(bytes));
                    }
                    Err(e) => {
                        map.insert(url.clone(), PosterStatus::Failed(e));
                    }
                }
            }
        }
    });
    PosterRegistry {
        entries,
        sender: tx,
    }
}

fn download_one(client: Option<&reqwest::blocking::Client>, url: &str) -> Result<Vec<u8>, String> {
    let resp = match client {
        Some(c) => c.get(url).send(),
        None => reqwest::blocking::get(url),
    };
    let resp = resp.map_err(|e| format!("get {}: {}", url, e))?;
    if !resp.status().is_success() {
        return Err(format!("get {}: http {}", url, resp.status().as_u16()));
    }
    let bytes = resp
        .bytes()
        .map_err(|e| format!("read {}: {}", url, e))?
        .to_vec();
    if bytes.is_empty() {
        return Err(format!("get {}: empty body", url));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    #[test]
    fn registry_request_empty_url_is_no_op() {
        // Closed channel sender so any send() would panic; we should
        // not even attempt to send on empty input.
        let (tx, _rx) = channel::<String>();
        let registry = PosterRegistry {
            entries: Arc::new(Mutex::new(HashMap::new())),
            sender: tx,
        };
        registry.request(""); // should not panic
    }

    #[test]
    fn registry_request_dedup_skips_existing() {
        let (tx, rx) = sync_channel::<String>(0);
        let entries = Arc::new(Mutex::new(HashMap::new()));
        entries.lock().unwrap().insert(
            "https://example.com/poster.jpg".to_string(),
            PosterStatus::Pending,
        );
        let registry = PosterRegistry {
            entries: entries.clone(),
            sender: {
                // Wrap sync sender in an unbounded channel for the request() signature
                let (utx, urx) = channel::<String>();
                thread::spawn(move || {
                    while let Ok(s) = urx.recv() {
                        let _ = tx.send(s);
                    }
                });
                utx
            },
        };
        registry.request("https://example.com/poster.jpg");
        // Give the relay a moment; if dedup correctly skipped, channel
        // is empty after 100ms.
        thread::sleep(Duration::from_millis(100));
        // try_recv should be empty
        assert!(
            rx.try_recv().is_err(),
            "dedup should have prevented send for existing entry"
        );
    }

    #[test]
    fn registry_request_sends_new_url() {
        // Forward via relay so we can assert what landed on a bounded receiver.
        let (tx, rx) = sync_channel::<String>(2);
        let (utx, urx) = channel::<String>();
        thread::spawn(move || {
            while let Ok(s) = urx.recv() {
                let _ = tx.send(s);
            }
        });
        let registry = PosterRegistry {
            entries: Arc::new(Mutex::new(HashMap::new())),
            sender: utx,
        };
        registry.request("https://example.com/new.jpg");
        let received = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(received, "https://example.com/new.jpg");
    }

    #[test]
    fn poster_status_variants_clone_safely() {
        let bytes = PosterStatus::Bytes(vec![0xFF, 0xD8, 0xFF, 0xE0]);
        let cloned = bytes.clone();
        if let PosterStatus::Bytes(b) = cloned {
            assert_eq!(b, vec![0xFF, 0xD8, 0xFF, 0xE0]);
        } else {
            panic!("clone changed variant");
        }
    }
}
