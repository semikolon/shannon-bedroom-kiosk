//! Home Assistant actuator **planning** (Slice 2) — pure, std-only.
//!
//! The context engine (`crate::context`) emits abstract `Action`s. This
//! module turns those — and direct lights requests from the kiosk — into
//! concrete HA service calls (`HaCall`), with **zero I/O**. The actual
//! authenticated HTTP send lives in the `shannon-kiosk-actions` daemon
//! binary, so this layer stays deterministically unit-testable with no
//! live HA, no token, no network (the pure-core / thin-edge split used by
//! the engine in Slice 1).
//!
//! Secrets never live here: `HaConfig::token` defaults empty and is
//! populated from the environment at the daemon (deploy-time, escalated).

use crate::context::Action;

/// One concrete Home Assistant service call. The daemon POSTs this to
/// `{base_url}/api/services/{domain}/{service}` with `{"entity_id": …}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HaCall {
    pub domain: String,
    pub service: String,
    pub entity_id: String,
}

/// Planning failures (no network involved — these are input errors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HaError {
    UnknownLightGroup(String),
    UnknownMediaEntity(String),
    UnknownAction(String),
}

impl std::fmt::Display for HaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HaError::UnknownLightGroup(g) => write!(f, "unknown light group: {g}"),
            HaError::UnknownMediaEntity(e) => write!(f, "unknown media entity: {e}"),
            HaError::UnknownAction(a) => write!(f, "unknown action: {a}"),
        }
    }
}

impl std::error::Error for HaError {}

/// Deploy-time configuration. Entity ids are placeholders until the real
/// bedroom devices are confirmed on Shannon's HA; `token`/`base_url` are
/// supplied from the environment by the daemon (never committed).
#[derive(Debug, Clone)]
pub struct HaConfig {
    pub base_url: String,
    /// Bearer token — empty in source; injected at runtime (escalated).
    pub token: String,
    /// The smart-plug switch entity that powers the bedroom TV.
    pub tv_plug_entity: String,
    /// Friendly kiosk group name → HA group entity. Mirrors the lights-v2
    /// / presence-service groups in `docs/personal_iot.md`.
    ///
    /// **Codified principle — excluded by design** (Fredrik 2026-05-21):
    /// Tuya device-firmware sub-features that integrations surface as
    /// their own switch entities — e.g. `switch.desk_child_lock` (the
    /// desk plug's physical-button-disable safety feature) — look like
    /// "switches" to HA but are NOT user-facing light/plug controls.
    /// They're device-level configuration. Any kiosk action that says
    /// "toggle lights" / "toggle all" MUST go through these named
    /// groups, never sweep-by-entity-type, so these stay safely out.
    /// Forward-looking guard against any future "toggle all switches"
    /// composite action accidentally grabbing them. See also
    /// `docs/personal_iot.md` § "Auxiliary (NOT a lamp)".
    pub light_groups: Vec<(String, String)>,
    /// Friendly kiosk media-entity name → HA `media_player.*` entity.
    /// `"default"` is the Music tile's target (toggle play/pause); today
    /// that's `media_player.fredrik` (spotifyd Spotify-Connect on
    /// Shannon, advertised via mDNS). Future per-zone routing extends
    /// the table (vardagsrum / atelier / etc.) without changing the
    /// resolver shape.
    pub media_entities: Vec<(String, String)>,
}

impl Default for HaConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:8123".to_string(),
            token: String::new(),
            tv_plug_entity: "switch.bedroom_tv_plug".to_string(),
            light_groups: vec![
                ("bedroom".to_string(), "group.bedroom_lights".to_string()),
                ("office".to_string(), "group.office_lights".to_string()),
                ("hallway".to_string(), "group.hallway_indicator".to_string()),
            ],
            media_entities: vec![
                ("default".to_string(), "media_player.fredrik".to_string()),
                ("fredrik".to_string(), "media_player.fredrik".to_string()),
            ],
        }
    }
}

impl HaConfig {
    fn resolve_group(&self, group: &str) -> Option<&str> {
        self.light_groups
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(group))
            .map(|(_, entity)| entity.as_str())
    }

    fn resolve_media_entity(&self, key: &str) -> Option<&str> {
        self.media_entities
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, entity)| entity.as_str())
    }
}

fn service_for_onoff(on: bool) -> &'static str {
    if on {
        "turn_on"
    } else {
        "turn_off"
    }
}

/// Plan the TV smart-plug power call (autonomous power: the engine
/// decided, this only translates).
#[must_use]
pub fn plan_tv_power(on: bool, cfg: &HaConfig) -> HaCall {
    HaCall {
        domain: "switch".to_string(),
        service: service_for_onoff(on).to_string(),
        entity_id: cfg.tv_plug_entity.clone(),
    }
}

/// Plan a media_player action from a kiosk request. `entity_key` is a
/// friendly kiosk-side name (e.g., `"fredrik"` or `"default"`) that
/// resolves to a configured `media_player.*` entity in HA. `action` maps
/// to HA's `media_player.*` services (`play_pause` / `play` / `pause` /
/// `stop` / `next` / `prev`, case-insensitive). The Music tile uses
/// `"default" + "play_pause"` to toggle the spotifyd Spotify-Connect
/// entity (`media_player.fredrik`); future per-zone routing extends
/// `resolve_media_entity` to map additional keys.
pub fn plan_media(entity_key: &str, action: &str, cfg: &HaConfig) -> Result<HaCall, HaError> {
    let entity = cfg
        .resolve_media_entity(entity_key)
        .ok_or_else(|| HaError::UnknownMediaEntity(entity_key.to_string()))?;
    let service = match action.to_ascii_lowercase().as_str() {
        "play_pause" | "toggle" => "media_play_pause",
        "play" => "media_play",
        "pause" => "media_pause",
        "stop" => "media_stop",
        "next" | "next_track" => "media_next_track",
        "prev" | "previous" | "previous_track" => "media_previous_track",
        other => return Err(HaError::UnknownAction(other.to_string())),
    };
    Ok(HaCall {
        domain: "media_player".to_string(),
        service: service.to_string(),
        entity_id: entity.to_string(),
    })
}

/// Plan a lights group call from a kiosk request. `action` is `"on"`,
/// `"off"`, or `"toggle"` (case-insensitive). Uses the `homeassistant`
/// domain so the same call works whether the entity is a `group.`,
/// `light.`, or scene. `toggle` lets the kiosk fire a single button to
/// flip a group without needing to track the current HA state — the
/// `homeassistant.toggle` service handles state-flip server-side.
pub fn plan_lights(group: &str, action: &str, cfg: &HaConfig) -> Result<HaCall, HaError> {
    let entity = cfg
        .resolve_group(group)
        .ok_or_else(|| HaError::UnknownLightGroup(group.to_string()))?;
    let service = match action.to_ascii_lowercase().as_str() {
        "on" => "turn_on",
        "off" => "turn_off",
        "toggle" => "toggle",
        other => return Err(HaError::UnknownAction(other.to_string())),
    };
    Ok(HaCall {
        domain: "homeassistant".to_string(),
        service: service.to_string(),
        entity_id: entity.to_string(),
    })
}

/// Translate one engine action into an HA call, if it needs one.
/// `Action::Show(_)` is the host's job (paint the screen), not HA's —
/// only power transitions actuate Home Assistant.
#[must_use]
pub fn apply_engine_action(action: &Action, cfg: &HaConfig) -> Option<HaCall> {
    match action {
        Action::SetTvPower(on) => Some(plan_tv_power(*on, cfg)),
        Action::Show(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::DisplayState;

    #[test]
    fn tv_power_maps_to_switch_service() {
        let c = HaConfig::default();
        assert_eq!(
            plan_tv_power(true, &c),
            HaCall {
                domain: "switch".into(),
                service: "turn_on".into(),
                entity_id: "switch.bedroom_tv_plug".into(),
            }
        );
        assert_eq!(plan_tv_power(false, &c).service, "turn_off");
    }

    #[test]
    fn lights_resolve_known_groups_case_insensitively() {
        let c = HaConfig::default();
        let call = plan_lights("BedRoom", "On", &c).expect("known group");
        assert_eq!(call.domain, "homeassistant");
        assert_eq!(call.service, "turn_on");
        assert_eq!(call.entity_id, "group.bedroom_lights");
        assert_eq!(
            plan_lights("office", "off", &c).unwrap().entity_id,
            "group.office_lights"
        );
    }

    #[test]
    fn lights_reject_unknown_group_and_action() {
        let c = HaConfig::default();
        assert_eq!(
            plan_lights("kitchen", "on", &c),
            Err(HaError::UnknownLightGroup("kitchen".into()))
        );
        assert_eq!(
            plan_lights("bedroom", "dim", &c),
            Err(HaError::UnknownAction("dim".into()))
        );
    }

    #[test]
    fn lights_toggle_dispatches_homeassistant_toggle() {
        // X-button bedroom-quick-toggle (Fredrik 2026-05-21) flows through
        // this code path; the daemon POSTs /lights/bedroom/toggle and we
        // emit `homeassistant.toggle` on the bedroom group — HA flips the
        // current state server-side so the kiosk never has to track it.
        let c = HaConfig::default();
        let call = plan_lights("bedroom", "toggle", &c).expect("toggle is valid");
        assert_eq!(call.domain, "homeassistant");
        assert_eq!(call.service, "toggle");
        assert_eq!(call.entity_id, "group.bedroom_lights");
        // case-insensitive on action too
        assert_eq!(
            plan_lights("office", "TOGGLE", &c).unwrap().service,
            "toggle"
        );
    }

    #[test]
    fn engine_action_translation_only_actuates_power() {
        let c = HaConfig::default();
        assert_eq!(
            apply_engine_action(&Action::SetTvPower(true), &c),
            Some(plan_tv_power(true, &c))
        );
        // Showing a screen state is the host's job, not HA's.
        assert_eq!(
            apply_engine_action(&Action::Show(DisplayState::Kiosk), &c),
            None
        );
    }

    #[test]
    fn no_secret_baked_in_source() {
        // Guard: the committed default must never carry a token.
        assert!(HaConfig::default().token.is_empty());
    }

    #[test]
    fn ha_error_displays() {
        assert_eq!(
            HaError::UnknownLightGroup("x".into()).to_string(),
            "unknown light group: x"
        );
        assert_eq!(
            HaError::UnknownMediaEntity("y".into()).to_string(),
            "unknown media entity: y"
        );
    }

    #[test]
    fn media_play_pause_maps_to_default_entity() {
        let c = HaConfig::default();
        let call = plan_media("default", "play_pause", &c).expect("default key resolves");
        assert_eq!(call.domain, "media_player");
        assert_eq!(call.service, "media_play_pause");
        assert_eq!(call.entity_id, "media_player.fredrik");
        // Aliases
        assert_eq!(
            plan_media("Fredrik", "toggle", &c).unwrap().service,
            "media_play_pause"
        );
    }

    #[test]
    fn media_actions_map_correctly() {
        let c = HaConfig::default();
        for (action, service) in [
            ("play", "media_play"),
            ("pause", "media_pause"),
            ("stop", "media_stop"),
            ("next", "media_next_track"),
            ("next_track", "media_next_track"),
            ("prev", "media_previous_track"),
            ("previous", "media_previous_track"),
            ("previous_track", "media_previous_track"),
        ] {
            let call = plan_media("default", action, &c)
                .unwrap_or_else(|_| panic!("action {action:?} should map"));
            assert_eq!(call.service, service, "action {action:?}");
        }
    }

    #[test]
    fn media_rejects_unknown_entity_and_action() {
        let c = HaConfig::default();
        assert_eq!(
            plan_media("vardagsrum", "play", &c),
            Err(HaError::UnknownMediaEntity("vardagsrum".into()))
        );
        assert_eq!(
            plan_media("default", "rewind", &c),
            Err(HaError::UnknownAction("rewind".into()))
        );
    }
}
