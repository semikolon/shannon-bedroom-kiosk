//! `shannon-kiosk-actions` — the Phase-3A action-daemon (Slice 2).
//!
//! A thin localhost (`127.0.0.1:8080`) axum service: a separate process
//! from the Bevy UI (per the project plan). It holds the pure context
//! `Engine` + the pure HA `plan_*` layer (both unit-tested in the lib)
//! and adds only the I/O edge: drive the engine from pushed signals, and
//! send the planned Home Assistant calls over authenticated HTTP.
//!
//! Secrets come from the environment at runtime (deploy-time, escalated)
//! — never the source. Endpoints:
//!   GET  /healthz                 liveness
//!   GET  /state                   current DisplayState
//!   POST /signal      {…}         feed the engine one tick, actuate
//!   POST /lights/:group/:action   direct lights proxy (on|off)
//!
//! NOT auto-deployed: cross-compile + Shannon deploy is gated on the
//! stability soak / 03:00-watch and is a per-host human decision.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use shannon_kiosk::context::{
    Action, ClockMinutes, Config, DisplayState, Engine, Inputs, Manual, Media,
};
use shannon_kiosk::ha::{apply_engine_action, plan_lights, plan_media, HaCall, HaConfig};
use shannon_kiosk::ha_poll::{
    binary_sensor_from_str, BinarySensorState, HaPollState, MediaPlayerState, PollResult,
};

struct AppState {
    engine: Mutex<Engine>,
    ha: HaConfig,
    http: reqwest::Client,
    /// HA polling cache (Slice 3d). Background task refreshes this every
    /// `poll_interval`; the `/ha-state` endpoint snapshots the current
    /// contents. Mutex keeps reads + the refresh task race-free.
    ha_poll: Mutex<HaPollState>,
    /// Configured HA entity to poll for media-player state. Empty string
    /// means "don't poll media_player".
    media_player_entity: String,
    /// Configured HA entity to poll for music-player state (Spotify /
    /// spotifyd typically). Polled separately from the TV media_player
    /// so the Music-NowPlaying view has its own title/artist/state
    /// independent of TV casting. Empty = skip.
    music_player_entity: String,
    /// Configured HA entity to poll for occupancy. Empty = skip.
    occupancy_entity: String,
    /// How often to refresh `ha_poll`. 30s is a reasonable default for
    /// kiosk needs (paused-media detection + presence don't need <30s
    /// granularity; HA itself can take 1-3s to propagate state).
    poll_interval: Duration,
}

#[tokio::main]
async fn main() {
    let music_entity = env_or("HA_MUSIC_PLAYER_ENTITY", "media_player.music");
    let ha = HaConfig {
        base_url: env_or("HA_BASE_URL", "http://localhost:8123"),
        token: std::env::var("HA_TOKEN").unwrap_or_default(),
        tv_plug_entity: env_or("HA_TV_PLUG_ENTITY", "switch.bedroom_tv_plug"),
        media_entities: vec![
            ("default".to_string(), music_entity.clone()),
            ("music".to_string(), music_entity),
        ],
        ..HaConfig::default()
    };
    let state = Arc::new(AppState {
        engine: Mutex::new(Engine::new(Config::default())),
        ha,
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .expect("reqwest client"),
        ha_poll: Mutex::new(HaPollState::default()),
        media_player_entity: env_or("HA_MEDIA_PLAYER_ENTITY", "media_player.tv"),
        music_player_entity: env_or("HA_MUSIC_PLAYER_ENTITY", "media_player.music"),
        occupancy_entity: env_or("HA_OCCUPANCY_ENTITY", ""),
        poll_interval: parse_secs("HA_POLL_INTERVAL_SECS", 30),
    });

    // Background HA-poll task (Slice 3d). Runs every poll_interval; on
    // failure, applies exponential backoff capped at 5 minutes (so a
    // briefly-unavailable HA doesn't starve the daemon AND so a long
    // outage doesn't hammer at full rate).
    let poll_state = state.clone();
    tokio::spawn(async move {
        ha_poll_loop(poll_state).await;
    });

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/state", get(state_handler))
        .route("/ha-state", get(ha_state_handler))
        .route("/signal", post(signal_handler))
        .route("/lights/:group/:action", post(lights_handler))
        .route("/media/:entity_key/:action", post(media_handler))
        .route("/watch", post(watch_handler))
        .route("/seek", post(seek_handler))
        .route("/transcribe", post(transcribe_handler))
        .route("/audio/sinks", get(audio_sinks_handler))
        .route(
            "/audio/sink",
            get(audio_sink_current_handler).post(audio_sink_set_handler),
        )
        .route("/bt/paired", get(bt_paired_handler))
        .route("/bt/scan", post(bt_scan_handler))
        .route("/bt/pair", post(bt_pair_handler))
        .route("/bt/connect", post(bt_connect_handler))
        .route("/bt/disconnect", post(bt_disconnect_handler))
        .route("/bt/forget", post(bt_forget_handler))
        .with_state(state);

    // Bind: default to LAN-accessible 0.0.0.0:8080 so the remote media
    // controller can POST /watch when target=shannon is picked. The
    // /watch handler's title validation (length ≤200, no control chars)
    // + Command::arg-not-shell-exec are the security posture. Tighten
    // via env SHANNON_KIOSK_ACTIONS_BIND if needed (e.g. to a specific
    // LAN-reserved IP, or back to "127.0.0.1:8080" if LAN access is
    // undesirable).
    let bind_addr = env_or("SHANNON_KIOSK_ACTIONS_BIND", "0.0.0.0:8080");
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("bind {}: {}", bind_addr, e));
    axum::serve(listener, app).await.expect("serve");
}

fn parse_secs(key: &str, default: u64) -> Duration {
    let secs = std::env::var(key)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(default);
    Duration::from_secs(secs)
}

/// HA-poll background loop. Updates `state.ha_poll` every
/// `state.poll_interval`; on failure, exponential backoff up to 5 min.
async fn ha_poll_loop(state: Arc<AppState>) {
    let mut backoff = state.poll_interval;
    let max_backoff = Duration::from_secs(300);

    loop {
        let result = poll_once(&state).await;
        match result {
            PollResult::Ok => {
                backoff = state.poll_interval;
            }
            PollResult::NeverPolled => {
                // Cache reset / shutdown signal — restore default cadence
                backoff = state.poll_interval;
            }
            _ => {
                backoff = (backoff * 2).min(max_backoff);
            }
        }
        tokio::time::sleep(backoff).await;
    }
}

/// One poll cycle: refresh each configured entity. Returns the WORST
/// observed result (so any error backs off the next sleep).
async fn poll_once(state: &Arc<AppState>) -> PollResult {
    let mut worst = PollResult::Ok;

    if !state.media_player_entity.is_empty() {
        let r = poll_media_player(state).await;
        if rank_result(r) > rank_result(worst) {
            worst = r;
        }
    }

    if !state.music_player_entity.is_empty() {
        let r = poll_music_player(state).await;
        if rank_result(r) > rank_result(worst) {
            worst = r;
        }
    }

    if !state.occupancy_entity.is_empty() {
        let r = poll_occupancy(state).await;
        if rank_result(r) > rank_result(worst) {
            worst = r;
        }
    }

    // Bookkeep last_poll_at + failure counts regardless of which entity
    // failed (the cache's view is "the last cycle finished at T").
    let mut cache = state.ha_poll.lock().await;
    cache.last_poll_at = Some(SystemTime::now());
    cache.last_poll_result = worst;
    if matches!(worst, PollResult::Ok) {
        cache.consecutive_failures = 0;
    } else {
        cache.consecutive_failures = cache.consecutive_failures.saturating_add(1);
    }
    worst
}

fn rank_result(r: PollResult) -> u8 {
    match r {
        PollResult::Ok => 0,
        PollResult::NeverPolled => 1,
        PollResult::ParseError => 2,
        PollResult::EntityNotFound => 3,
        PollResult::NetworkError => 4,
    }
}

async fn poll_media_player(state: &Arc<AppState>) -> PollResult {
    match fetch_entity(state, &state.media_player_entity).await {
        Ok(v) => match parse_media_player(&v) {
            Some(mp) => {
                let mut cache = state.ha_poll.lock().await;
                cache.media_player = Some(mp);
                PollResult::Ok
            }
            None => PollResult::ParseError,
        },
        Err(r) => r,
    }
}

async fn poll_music_player(state: &Arc<AppState>) -> PollResult {
    match fetch_entity(state, &state.music_player_entity).await {
        Ok(v) => match parse_media_player(&v) {
            Some(mp) => {
                let mut cache = state.ha_poll.lock().await;
                cache.music_player = Some(mp);
                PollResult::Ok
            }
            None => PollResult::ParseError,
        },
        Err(r) => r,
    }
}

async fn poll_occupancy(state: &Arc<AppState>) -> PollResult {
    match fetch_entity(state, &state.occupancy_entity).await {
        Ok(v) => {
            let bs = v
                .get("state")
                .and_then(|x| x.as_str())
                .map(binary_sensor_from_str)
                .unwrap_or(BinarySensorState::Unknown);
            let mut cache = state.ha_poll.lock().await;
            cache.occupancy = Some(bs);
            PollResult::Ok
        }
        Err(r) => r,
    }
}

/// GET `{HA_BASE_URL}/api/states/<entity_id>` with bearer token.
/// Returns the parsed JSON `Value` on success, or a categorized
/// `PollResult` on error so callers can attribute it to the entity.
async fn fetch_entity(state: &Arc<AppState>, entity_id: &str) -> Result<Value, PollResult> {
    let url = format!(
        "{}/api/states/{}",
        state.ha.base_url.trim_end_matches('/'),
        entity_id
    );
    let mut rb = state.http.get(&url);
    if !state.ha.token.is_empty() {
        rb = rb.bearer_auth(&state.ha.token);
    }
    let resp = rb.send().await.map_err(|_| PollResult::NetworkError)?;
    if resp.status().as_u16() == 404 {
        return Err(PollResult::EntityNotFound);
    }
    if !resp.status().is_success() {
        return Err(PollResult::NetworkError);
    }
    resp.json::<Value>()
        .await
        .map_err(|_| PollResult::ParseError)
}

/// Pure JSON → `MediaPlayerState` parser. Returns None only if the
/// `state` field is missing or non-string — every other attribute is
/// optional. Tested in unit tests below.
fn parse_media_player(v: &Value) -> Option<MediaPlayerState> {
    let state = v.get("state")?.as_str()?.to_string();
    let attrs = v.get("attributes");
    let media_title = attrs
        .and_then(|a| a.get("media_title"))
        .and_then(|x| x.as_str())
        .map(String::from);
    let media_artist = attrs
        .and_then(|a| a.get("media_artist"))
        .and_then(|x| x.as_str())
        .map(String::from);
    let media_content_type = attrs
        .and_then(|a| a.get("media_content_type"))
        .and_then(|x| x.as_str())
        .map(String::from);
    let media_position = attrs
        .and_then(|a| a.get("media_position"))
        .and_then(|x| x.as_f64())
        .map(|f| f as u32);
    let media_duration = attrs
        .and_then(|a| a.get("media_duration"))
        .and_then(|x| x.as_f64())
        .map(|f| f as u32);
    let app_name = attrs
        .and_then(|a| a.get("app_name"))
        .and_then(|x| x.as_str())
        .map(String::from);
    Some(MediaPlayerState {
        state,
        media_title,
        media_artist,
        media_content_type,
        media_position,
        media_duration,
        app_name,
    })
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[allow(clippy::unused_async)] // axum handlers must be async
async fn healthz() -> &'static str {
    "ok"
}

async fn state_handler(State(s): State<Arc<AppState>>) -> Json<Value> {
    let st = s.engine.lock().await.state();
    Json(json!({ "state": describe_state(st) }))
}

/// GET /ha-state — return the latest HA poll snapshot. Bevy consumers
/// (Slice 3e ribbon-offer wiring) call this to source `Inputs::media`
/// + presence + resume-title for the next engine tick.
async fn ha_state_handler(State(s): State<Arc<AppState>>) -> Json<Value> {
    let cache = s.ha_poll.lock().await;
    let media_player = cache.media_player.as_ref().map(|mp| {
        json!({
            "state": mp.state,
            "media_title": mp.media_title,
            "media_artist": mp.media_artist,
            "media_content_type": mp.media_content_type,
            "media_position": mp.media_position,
            "media_duration": mp.media_duration,
            "app_name": mp.app_name,
        })
    });
    let music_player = cache.music_player.as_ref().map(|mp| {
        json!({
            "state": mp.state,
            "media_title": mp.media_title,
            "media_artist": mp.media_artist,
            "media_content_type": mp.media_content_type,
            "media_position": mp.media_position,
            "media_duration": mp.media_duration,
            "app_name": mp.app_name,
        })
    });
    let occupancy = cache.occupancy.map(|bs| match bs {
        BinarySensorState::On => "on",
        BinarySensorState::Off => "off",
        BinarySensorState::Unknown => "unknown",
    });
    Json(json!({
        "media_player": media_player,
        "music_player": music_player,
        "occupancy": occupancy,
        "engine_media": match cache.engine_media() {
            Media::None => "none",
            Media::Music => "music",
            Media::Video => "video",
            Media::Game => "game",
        },
        "resumable_title": cache.resumable_title(),
        "occupancy_present": cache.occupancy_present(),
        "last_poll_at_unix": cache.last_poll_at.and_then(|t| {
            t.duration_since(SystemTime::UNIX_EPOCH).ok().map(|d| d.as_secs())
        }),
        "last_poll_result": match cache.last_poll_result {
            PollResult::NeverPolled => "never_polled",
            PollResult::Ok => "ok",
            PollResult::NetworkError => "network_error",
            PollResult::ParseError => "parse_error",
            PollResult::EntityNotFound => "entity_not_found",
        },
        "consecutive_failures": cache.consecutive_failures,
    }))
}

/// One engine tick from pushed signals; actuates any power transition.
async fn signal_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<SignalReq>,
) -> impl IntoResponse {
    let outcome = {
        let mut eng = s.engine.lock().await;
        eng.step(&req.to_inputs())
    };

    let mut actuated = Vec::new();
    for action in &outcome.actions {
        if let Some(call) = apply_engine_action(action, &s.ha) {
            let status = send(&s.http, &s.ha, &call).await;
            actuated.push(json!({ "call": describe_call(&call), "result": status }));
        }
    }

    Json(json!({
        "state": describe_state(outcome.state),
        "actions": outcome.actions.iter().map(describe_action).collect::<Vec<_>>(),
        "actuated": actuated,
    }))
}

async fn lights_handler(
    State(s): State<Arc<AppState>>,
    Path((group, action)): Path<(String, String)>,
) -> impl IntoResponse {
    // Resolve group → HA group entity (e.g. "group.bedroom_lights") + map
    // service → shannon-lights-daemon HTTP verb. Then POST to the
    // persistent daemon at 127.0.0.1:8081 (warm tinytuya OutletDevices,
    // ~400ms typical end-to-end vs ~2-2.5s for the v1 subprocess path
    // dominated by Python+tinytuya import). LocalTuya HA integration is
    // STILL bypassed here — direct tinytuya in the daemon. HA group
    // membership stays SSoT: lights-daemon queries HA REST for member
    // entity_ids (with TTL cache).
    //
    // History: v0 used HA REST → LocalTuya (silent-failed 2026-05-24).
    // v1 shelled out to shannon-lights script (~2.5s per call, Python
    // startup dominant). v2 (2026-05-24 evening) talks to persistent
    // shannon-lights-daemon over HTTP — eliminates startup + per-call
    // reconnect. See ~/dotfiles/system/shannon/README.md
    // § "LocalTuya HA integration silent-failure".
    match plan_lights(&group, &action, &s.ha) {
        Ok(call) => {
            let verb = match call.service.as_str() {
                "turn_on" => "on",
                "turn_off" => "off",
                "toggle" => "toggle",
                other => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": format!("unknown service: {}", other) })),
                    );
                }
            };
            let entity = call.entity_id.clone();
            // HA group entity_ids are `[a-z0-9_.]+` — all URL-safe per
            // RFC 3986 unreserved set. No encoding needed.
            let daemon_url = format!("http://127.0.0.1:8081/{}?group={}", verb, entity);
            // Reuse the AppState http client (already shared with other
            // handlers); a short timeout because lights-daemon should
            // respond in <1s.
            let req = s
                .http
                .post(&daemon_url)
                .timeout(std::time::Duration::from_secs(8));
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let body: serde_json::Value = resp
                        .json()
                        .await
                        .unwrap_or_else(|e| json!({ "parse_error": e.to_string() }));
                    (
                        StatusCode::OK,
                        Json(json!({
                            "via": "shannon-lights-daemon",
                            "group": entity,
                            "verb": verb,
                            "http_status": status.as_u16(),
                            "result": body,
                        })),
                    )
                }
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "via": "shannon-lights-daemon",
                        "group": entity,
                        "verb": verb,
                        "error": format!("daemon unreachable: {}", e),
                    })),
                ),
            }
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

/// Music tile + future-zone routing. `POST /media/{entity_key}/{action}`
/// maps `entity_key` (e.g., `"default"`, `"fredrik"`) to a configured
/// `media_player.*` entity and `action` to an HA `media_player.*`
/// service (`play_pause` / `play` / `pause` / `stop` / `next` / `prev`).
///
/// Mirrors `lights_handler` shape — sync HA call, returns 200 with the
/// rendered HaCall + HA HTTP status, 400 on unknown entity/action.
/// Body is empty (entity + action are path params, no payload needed).
async fn media_handler(
    State(s): State<Arc<AppState>>,
    Path((entity_key, action)): Path<(String, String)>,
) -> impl IntoResponse {
    match plan_media(&entity_key, &action, &s.ha) {
        Ok(call) => {
            let status = send(&s.http, &s.ha, &call).await;
            (
                StatusCode::OK,
                Json(json!({ "call": describe_call(&call), "result": status })),
            )
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

/// Phase-7 spela-thin-client Layer-4a — fire-and-forget shell spawn of
/// `spela-local <title>` on this host (Shannon). The shell client owns
/// the full data pipeline (Darwin spela /play → HLS → local mpv); we
/// only need to ignite it. Returns 202 immediately so the Bevy UI's
/// debounce/in-flight gate clears in <50ms instead of waiting on a
/// Chromecast-style cold-start.
///
/// Body: `{"title": "<search query>", "smoke"?: <seconds>}`
///   - `title` is forwarded verbatim to `spela-local` (which calls
///     spela's /search?q=<title>); spela's ranker picks result_id=1.
///   - `smoke` (optional) caps playback duration — passes
///     `--smoke <secs>` to `spela-local` so the TV isn't stranded with
///     a 2h movie during integration testing.
///
/// Implementation notes:
///   - `std::process::Command::spawn()` (NOT `tokio::process`) to avoid
///     adding the tokio `process` feature, which would dirty Cargo.lock
///     mid-Session-A-work and risk a merge conflict.
///   - The child is reaped in a `std::thread::spawn` closure to prevent
///     zombie accumulation (long daemon lifetime, potentially many
///     plays over weeks).
///   - Title length capped at 200 bytes (DoS hygiene + sane URL/log
///     budget); empty title rejected with 400.
async fn watch_handler(Json(req): Json<WatchReq>) -> impl IntoResponse {
    let title = req.title.trim();
    if title.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing or empty title" })),
        );
    }
    if title.len() > 200 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "title too long (>200 bytes)" })),
        );
    }
    // Reject control characters / NUL — defense at the shell-exec boundary
    // (`Command::arg` doesn't go through a shell, but stray control bytes
    // in spela-local's logs are noise we don't need).
    if title.chars().any(|c| c.is_control()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "title contains control characters" })),
        );
    }

    // Magnet validation (2026-05-26): defense at the daemon boundary even
    // though Darwin spela does its own validate_magnet_uri at /play time.
    // Reject control chars (same hygiene as title); cap at 4096 bytes
    // (typical magnet is 400-600 chars; 4096 is generous, blocks abuse).
    let magnet_owned = match req.magnet.as_deref() {
        None | Some("") => None,
        Some(m) if m.len() > 4096 => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "magnet too long (>4096 bytes)" })),
            );
        }
        Some(m) if m.chars().any(|c| c.is_control()) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "magnet contains control characters" })),
            );
        }
        Some(m) => Some(m.to_string()),
    };

    let title_owned = title.to_string();
    let smoke_secs = req.smoke;
    let file_index = req.file_index;

    let spawn_result = std::thread::Builder::new()
        .name(format!("watch-spawn-{}", title_owned.replace(' ', "-")))
        .spawn(move || {
            let mut cmd = std::process::Command::new("spela-local");
            cmd.arg(&title_owned);
            if let Some(secs) = smoke_secs {
                cmd.arg("--smoke").arg(secs.to_string());
            }
            if let Some(m) = magnet_owned.as_ref() {
                cmd.arg("--magnet").arg(m);
            }
            if let Some(idx) = file_index {
                cmd.arg("--file-index").arg(idx.to_string());
            }
            // Detach stdin/stdout/stderr from the daemon's tty so the
            // child can outlive any controlling terminal cleanly. mpv
            // writes to its own log under spela-local's control.
            cmd.stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            match cmd.spawn() {
                Ok(mut child) => {
                    // Block this OS thread on the child to reap it
                    // cleanly. axum's tokio runtime is unaffected.
                    let _ = child.wait();
                }
                Err(e) => {
                    eprintln!("watch_handler: failed to spawn spela-local: {e}");
                }
            }
        });

    match spawn_result {
        Ok(_) => (
            StatusCode::ACCEPTED,
            Json(json!({
                "spawned": true,
                "title": title,
                "smoke_secs": smoke_secs,
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("thread spawn failed: {e}") })),
        ),
    }
}

/// POST /seek — Phase 4 (2026-05-29). Forwards a seek command to the
/// spela-renderer control socket if a playback session is active. Two
/// shapes accepted:
///
///   { "delta": -30 }    seek relative N seconds (negative = backward)
///   { "absolute": 0 }   seek to absolute N seconds (e.g., 0 = restart)
///
/// Response shapes:
///   200 {"sent":"seek_relative -30","reply":"ok pos=120"}
///   503 {"error":"no active playback (socket not present)"}
///   500 {"error":"<detail>"} on IO / parse failures
///
/// The socket path follows the same XDG_RUNTIME_DIR resolution as the
/// renderer (`/run/cage-spela-local/spela-renderer.sock` when launched
/// by spela-local, `/tmp/spela-renderer.sock` ad-hoc). Both candidates
/// are tried in order; first existing wins.
async fn seek_handler(Json(req): Json<SeekReq>) -> impl IntoResponse {
    let command = match (req.delta, req.absolute) {
        (Some(d), None) => format!("seek_relative {d}\n"),
        (None, Some(a)) => format!("seek_absolute {a}\n"),
        (Some(_), Some(_)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "specify either 'delta' or 'absolute', not both" })),
            );
        }
        (None, None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "missing 'delta' or 'absolute'" })),
            );
        }
    };
    match send_renderer_command(&command).await {
        Ok(reply) => (
            StatusCode::OK,
            Json(json!({
                "sent": command.trim_end(),
                "reply": reply.trim_end(),
            })),
        ),
        Err(SeekError::SocketMissing) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "no active playback (socket not present)" })),
        ),
        Err(SeekError::Io(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("renderer ipc io: {e}") })),
        ),
    }
}

/// POST /transcribe — Phase 5 reactive voice search (2026-05-29).
/// Spawns `shannon-voice-capture` as a subprocess (records mic, POSTs
/// to Mac Mini transcribe endpoint, returns JSON). The JSON is parsed
/// here and re-emitted to the caller (the kiosk Bevy app), or wrapped
/// in an error envelope on failure.
///
/// Body: { "max_secs": 7 }   default 7 — push-to-talk window length
/// Response: { "text": "...", "language": "...", ... } on success
///           { "error": "...", ... }                  on failure
async fn transcribe_handler(Json(req): Json<TranscribeReq>) -> impl IntoResponse {
    let max_secs = req.max_secs.unwrap_or(7);
    if !(1..=30).contains(&max_secs) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "max_secs must be in [1,30]" })),
        );
    }

    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new("shannon-voice-capture")
            .arg(max_secs.to_string())
            .stderr(std::process::Stdio::piped())
            .output()
    })
    .await;
    let output = match output {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("spawn shannon-voice-capture: {e}") })),
            );
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("join_blocking: {e}") })),
            );
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": format!("shannon-voice-capture exit={:?}", output.status.code()),
                "stderr": stderr.trim_end(),
                "stdout": stdout.trim_end(),
            })),
        );
    }
    // shannon-voice-capture writes JSON to stdout — pass through.
    let body = String::from_utf8_lossy(&output.stdout).into_owned();
    match serde_json::from_str::<Value>(body.trim()) {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": format!("parse capture output: {e}"),
                "raw": body.trim(),
            })),
        ),
    }
}

// ─── Audio sink + Bluetooth routes — Phase-9 BT-audio (2026-05-29) ──────
//
// Thin wrappers over /usr/local/bin/shannon-audio-sink and /usr/local/bin/
// shannon-bt-pair. Used by both the kiosk Sound submenu (via Bevy) and
// directly by ssh/curl from the operator. Each handler shells out via
// tokio::task::spawn_blocking — the CLI helpers are sub-second except
// bt-scan (bounded by user-supplied secs param, capped at 30) and
// bt-pair (typically 5-15s for the BlueZ pairing dance).

async fn audio_sinks_handler() -> impl IntoResponse {
    match run_capture(&["shannon-audio-sink", "list"]).await {
        Ok(stdout) => {
            let sinks: Vec<Value> = stdout
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| {
                    let mut parts = l.splitn(3, '\t');
                    let id = parts.next()?.trim();
                    let name = parts.next()?.trim();
                    let flag = parts.next().unwrap_or("").trim();
                    Some(json!({
                        "id": id,
                        "name": name,
                        "default": flag == "default",
                    }))
                })
                .collect();
            (StatusCode::OK, Json(json!({ "sinks": sinks })))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        ),
    }
}

async fn audio_sink_current_handler() -> impl IntoResponse {
    match run_capture(&["shannon-audio-sink", "current"]).await {
        Ok(stdout) => {
            let mut parts = stdout.trim().splitn(2, '\t');
            let id = parts.next().unwrap_or("").to_string();
            let name = parts.next().unwrap_or("").to_string();
            (StatusCode::OK, Json(json!({ "id": id, "name": name })))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        ),
    }
}

async fn audio_sink_set_handler(Json(req): Json<AudioSinkSetReq>) -> impl IntoResponse {
    let cmd: Vec<String> = match (req.id, req.name) {
        (Some(id), _) => vec!["shannon-audio-sink".into(), "set".into(), id],
        (None, Some(name)) => vec!["shannon-audio-sink".into(), "set-name".into(), name],
        (None, None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "specify 'id' or 'name'" })),
            );
        }
    };
    let args: Vec<&str> = cmd.iter().map(|s| s.as_str()).collect();
    match run_capture(&args).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "status": "ok" }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        ),
    }
}

async fn bt_paired_handler() -> impl IntoResponse {
    match run_capture(&["shannon-bt-pair", "paired"]).await {
        Ok(stdout) => {
            let devices: Vec<Value> = stdout
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| {
                    let mut parts = l.splitn(2, ' ');
                    let mac = parts.next()?.trim();
                    let name = parts.next().unwrap_or("").trim();
                    Some(json!({ "mac": mac, "name": name }))
                })
                .collect();
            (StatusCode::OK, Json(json!({ "devices": devices })))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        ),
    }
}

async fn bt_scan_handler(Json(req): Json<BtScanReq>) -> impl IntoResponse {
    let secs = req.secs.unwrap_or(15).clamp(3, 30).to_string();
    match run_capture(&["shannon-bt-pair", "scan", &secs]).await {
        Ok(stdout) => {
            let devices: Vec<Value> = stdout
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| {
                    let mut parts = l.splitn(2, ' ');
                    let mac = parts.next()?.trim();
                    let name = parts.next().unwrap_or("").trim();
                    Some(json!({ "mac": mac, "name": name }))
                })
                .collect();
            (StatusCode::OK, Json(json!({ "devices": devices })))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        ),
    }
}

async fn bt_pair_handler(Json(req): Json<BtMacReq>) -> impl IntoResponse {
    bt_simple(&req.mac, "pair").await
}
async fn bt_connect_handler(Json(req): Json<BtMacReq>) -> impl IntoResponse {
    bt_simple(&req.mac, "connect").await
}
async fn bt_disconnect_handler(Json(req): Json<BtMacReq>) -> impl IntoResponse {
    bt_simple(&req.mac, "disconnect").await
}
async fn bt_forget_handler(Json(req): Json<BtMacReq>) -> impl IntoResponse {
    bt_simple(&req.mac, "forget").await
}

async fn bt_simple(mac: &str, action: &str) -> (StatusCode, Json<Value>) {
    if !is_valid_mac(mac) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid MAC address format" })),
        );
    }
    match run_capture(&["shannon-bt-pair", action, mac]).await {
        Ok(stdout) => (
            StatusCode::OK,
            Json(json!({ "status": "ok", "stdout": stdout.trim_end() })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        ),
    }
}

fn is_valid_mac(s: &str) -> bool {
    let s = s.trim();
    if s.len() != 17 {
        return false;
    }
    s.chars().enumerate().all(|(i, c)| {
        if i % 3 == 2 {
            c == ':'
        } else {
            c.is_ascii_hexdigit()
        }
    })
}

async fn run_capture(argv: &[&str]) -> Result<String, String> {
    let owned: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let out = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new(&owned[0]);
        for a in &owned[1..] {
            cmd.arg(a);
        }
        cmd.output()
    })
    .await
    .map_err(|e| format!("join_blocking: {e}"))?
    .map_err(|e| format!("spawn {}: {e}", argv[0]))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "{} exit={:?}: {}",
            argv.join(" "),
            out.status.code(),
            stderr.trim_end()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[derive(Debug, Deserialize)]
struct AudioSinkSetReq {
    id: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BtScanReq {
    secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct BtMacReq {
    mac: String,
}

#[derive(Debug, Deserialize)]
struct TranscribeReq {
    /// Maximum recording window in seconds. Default 7 — covers a typical
    /// "the boys season five" or "spela inception" phrase with margin.
    /// Capped at 30 to bound resource use.
    max_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SeekReq {
    /// Signed relative seconds (negative = backward). Mutually exclusive
    /// with `absolute`.
    delta: Option<i64>,
    /// Absolute target position in seconds (≥0). Mutually exclusive with
    /// `delta`. Set to 0 for restart-from-start.
    absolute: Option<u64>,
}

enum SeekError {
    /// Neither candidate socket path existed → no playback session.
    SocketMissing,
    /// Connect/read/write failed at the socket layer.
    Io(String),
}

/// Resolve the renderer's IPC socket path. The cage-spela-local path is
/// the live one when spela-local is the launcher (matches the renderer's
/// XDG_RUNTIME_DIR=/run/cage-spela-local); the /tmp fallback is for
/// ad-hoc operator runs.
fn renderer_socket_candidates() -> [&'static str; 2] {
    [
        "/run/cage-spela-local/spela-renderer.sock",
        "/tmp/spela-renderer.sock",
    ]
}

async fn send_renderer_command(line: &str) -> Result<String, SeekError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;
    use tokio::time::timeout;

    let mut last_io_err: Option<String> = None;
    let mut any_found = false;
    for path in renderer_socket_candidates() {
        if !std::path::Path::new(path).exists() {
            continue;
        }
        any_found = true;
        let connect = timeout(Duration::from_secs(2), UnixStream::connect(path)).await;
        let mut stream = match connect {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                last_io_err = Some(format!("connect {path}: {e}"));
                continue;
            }
            Err(_) => {
                last_io_err = Some(format!("connect {path}: timeout"));
                continue;
            }
        };
        if let Err(e) = timeout(Duration::from_secs(2), stream.write_all(line.as_bytes())).await {
            last_io_err = Some(format!("write timeout: {e}"));
            continue;
        }
        let _ = stream.shutdown().await;
        let mut buf = Vec::with_capacity(64);
        match timeout(Duration::from_secs(3), stream.read_to_end(&mut buf)).await {
            Ok(Ok(_)) => return Ok(String::from_utf8_lossy(&buf).to_string()),
            Ok(Err(e)) => {
                last_io_err = Some(format!("read: {e}"));
            }
            Err(_) => {
                last_io_err = Some("read timeout".to_string());
            }
        }
    }
    if !any_found {
        return Err(SeekError::SocketMissing);
    }
    Err(SeekError::Io(
        last_io_err.unwrap_or_else(|| "unknown io".to_string()),
    ))
}

#[derive(Debug, Deserialize)]
struct WatchReq {
    title: String,
    #[serde(default)]
    smoke: Option<u32>,
    // 2026-05-26 — magnet/file_index passthrough from Darwin spela's
    // target=shannon dispatch. When the web remote picks a specific
    // result_id, Darwin resolves it via its own last_search and sends
    // the exact magnet here so spela-local can SKIP its own /search
    // round-trip + POST /play with the exact release the user picked.
    // Without this, Shannon's own /search + /play 1 races against the
    // ranker (Torrentio order non-deterministic across requests).
    #[serde(default)]
    magnet: Option<String>,
    #[serde(default)]
    file_index: Option<u32>,
}

/// Send a planned HA call. Returns a short result tag (never panics; a
/// transport failure is reported, not unwrapped).
async fn send(http: &reqwest::Client, ha: &HaConfig, call: &HaCall) -> String {
    let url = format!(
        "{}/api/services/{}/{}",
        ha.base_url.trim_end_matches('/'),
        call.domain,
        call.service
    );
    let mut rb = http
        .post(&url)
        .json(&json!({ "entity_id": call.entity_id }));
    if !ha.token.is_empty() {
        rb = rb.bearer_auth(&ha.token);
    }
    match rb.send().await {
        Ok(resp) => format!("http {}", resp.status().as_u16()),
        Err(e) if e.is_timeout() => "error: timeout".to_string(),
        Err(e) if e.is_connect() => "error: connect".to_string(),
        Err(_) => "error: send".to_string(),
    }
}

#[derive(Debug, Deserialize)]
struct SignalReq {
    #[serde(default)]
    now_hour: u16,
    #[serde(default)]
    now_minute: u16,
    #[serde(default)]
    controller_connected: bool,
    #[serde(default)]
    since_input_secs: u64,
    #[serde(default)]
    fresh_controller_input: bool,
    /// "none" | "music" | "video" | "game"
    #[serde(default)]
    media: String,
    #[serde(default = "default_outdoor")]
    outdoor_brightness: f32,
    /// "on" | "off" | absent
    manual_press: Option<String>,
}

fn default_outdoor() -> f32 {
    0.5
}

impl SignalReq {
    fn to_inputs(&self) -> Inputs {
        Inputs {
            now: ClockMinutes::at(self.now_hour, self.now_minute),
            controller_connected: self.controller_connected,
            since_controller_input: Duration::from_secs(self.since_input_secs),
            fresh_controller_input: self.fresh_controller_input,
            media: match self.media.to_ascii_lowercase().as_str() {
                "music" => Media::Music,
                "video" => Media::Video,
                "game" => Media::Game,
                _ => Media::None,
            },
            outdoor_brightness: self.outdoor_brightness,
            manual_press: match self.manual_press.as_deref() {
                Some("on") => Some(Manual::ForceOn),
                Some("off") => Some(Manual::ForceOff),
                _ => None,
            },
        }
    }
}

fn describe_state(s: DisplayState) -> Value {
    match s {
        DisplayState::Off => json!("off"),
        DisplayState::Kiosk => json!("kiosk"),
        DisplayState::Content(a) => json!({ "content": format!("{a:?}").to_lowercase() }),
        DisplayState::Ambient(b) => json!({ "ambient": b.get() }),
    }
}

fn describe_action(a: &Action) -> Value {
    match a {
        Action::SetTvPower(on) => json!({ "set_tv_power": on }),
        Action::Show(s) => json!({ "show": describe_state(*s) }),
    }
}

fn describe_call(c: &HaCall) -> Value {
    json!({ "domain": c.domain, "service": c.service, "entity_id": c.entity_id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_media_player_full_payload() {
        let v = json!({
            "entity_id": "media_player.tv",
            "state": "playing",
            "attributes": {
                "media_title": "Show S01E03",
                "media_content_type": "tvshow",
                "media_position": 1234,
                "media_duration": 3600,
                "app_name": "Netflix"
            }
        });
        let mp = parse_media_player(&v).expect("parses");
        assert_eq!(mp.state, "playing");
        assert_eq!(mp.media_title.as_deref(), Some("Show S01E03"));
        assert_eq!(mp.media_content_type.as_deref(), Some("tvshow"));
        assert_eq!(mp.media_position, Some(1234));
        assert_eq!(mp.media_duration, Some(3600));
        assert_eq!(mp.app_name.as_deref(), Some("Netflix"));
    }

    #[test]
    fn parse_media_player_state_only_attrs_missing() {
        let v = json!({"state": "off"});
        let mp = parse_media_player(&v).expect("parses");
        assert_eq!(mp.state, "off");
        assert!(mp.media_title.is_none());
        assert!(mp.media_content_type.is_none());
        assert!(mp.media_position.is_none());
        assert!(mp.media_duration.is_none());
        assert!(mp.app_name.is_none());
    }

    #[test]
    fn parse_media_player_missing_state_returns_none() {
        let v = json!({"attributes": {"media_title": "x"}});
        assert!(parse_media_player(&v).is_none());
    }

    #[test]
    fn parse_media_player_non_string_state_returns_none() {
        // HA always returns string states; defensive against bad data.
        let v = json!({"state": 42});
        assert!(parse_media_player(&v).is_none());
    }

    #[test]
    fn parse_media_player_handles_unavailable() {
        let v = json!({"state": "unavailable"});
        let mp = parse_media_player(&v).expect("parses");
        assert_eq!(mp.state, "unavailable");
    }

    #[test]
    fn parse_media_player_position_as_float() {
        // HA sometimes returns float media_position (especially with
        // partial-second resolution). Verify we coerce.
        let v = json!({
            "state": "playing",
            "attributes": {"media_position": 123.7}
        });
        let mp = parse_media_player(&v).expect("parses");
        assert_eq!(mp.media_position, Some(123));
    }

    #[test]
    fn rank_result_orders_severity_correctly() {
        // Ranking is used in poll_once to track the WORST outcome of
        // multi-entity polls. Network errors should outrank parse
        // errors (network = full outage; parse = single entity glitch).
        assert!(rank_result(PollResult::Ok) < rank_result(PollResult::NeverPolled));
        assert!(rank_result(PollResult::NeverPolled) < rank_result(PollResult::ParseError));
        assert!(rank_result(PollResult::ParseError) < rank_result(PollResult::EntityNotFound));
        assert!(rank_result(PollResult::EntityNotFound) < rank_result(PollResult::NetworkError));
    }

    #[test]
    fn parse_secs_default_when_env_missing() {
        // SAFETY: this test reads env vars; std::env::var is safe to call.
        std::env::remove_var("PARSE_SECS_TEST_KEY_MISSING");
        assert_eq!(
            parse_secs("PARSE_SECS_TEST_KEY_MISSING", 42),
            Duration::from_secs(42)
        );
    }

    #[test]
    fn parse_secs_uses_env_when_valid() {
        std::env::set_var("PARSE_SECS_TEST_KEY_VALID", "7");
        assert_eq!(
            parse_secs("PARSE_SECS_TEST_KEY_VALID", 99),
            Duration::from_secs(7)
        );
        std::env::remove_var("PARSE_SECS_TEST_KEY_VALID");
    }

    #[test]
    fn parse_secs_falls_back_when_env_invalid() {
        std::env::set_var("PARSE_SECS_TEST_KEY_BAD", "not-a-number");
        assert_eq!(
            parse_secs("PARSE_SECS_TEST_KEY_BAD", 11),
            Duration::from_secs(11)
        );
        std::env::remove_var("PARSE_SECS_TEST_KEY_BAD");
    }
}
