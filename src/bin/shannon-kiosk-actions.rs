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
use shannon_kiosk::ha::{apply_engine_action, plan_lights, HaCall, HaConfig};
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
    /// Configured HA entity to poll for occupancy. Empty = skip.
    occupancy_entity: String,
    /// How often to refresh `ha_poll`. 30s is a reasonable default for
    /// kiosk needs (paused-media detection + presence don't need <30s
    /// granularity; HA itself can take 1-3s to propagate state).
    poll_interval: Duration,
}

#[tokio::main]
async fn main() {
    let ha = HaConfig {
        base_url: env_or("HA_BASE_URL", "http://localhost:8123"),
        token: std::env::var("HA_TOKEN").unwrap_or_default(),
        tv_plug_entity: env_or("HA_TV_PLUG_ENTITY", "switch.bedroom_tv_plug"),
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
        media_player_entity: env_or("HA_MEDIA_PLAYER_ENTITY", "media_player.fredriks_tv"),
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
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .expect("bind 127.0.0.1:8080");
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
    match plan_lights(&group, &action, &s.ha) {
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
            "entity_id": "media_player.fredriks_tv",
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
