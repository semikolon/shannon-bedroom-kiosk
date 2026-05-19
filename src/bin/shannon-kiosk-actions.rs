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
use std::time::Duration;

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

struct AppState {
    engine: Mutex<Engine>,
    ha: HaConfig,
    http: reqwest::Client,
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
    });

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/state", get(state_handler))
        .route("/signal", post(signal_handler))
        .route("/lights/:group/:action", post(lights_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .expect("bind 127.0.0.1:8080");
    axum::serve(listener, app).await.expect("serve");
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
