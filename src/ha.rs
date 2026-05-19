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
    UnknownAction(String),
}

impl std::fmt::Display for HaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HaError::UnknownLightGroup(g) => write!(f, "unknown light group: {g}"),
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
    pub light_groups: Vec<(String, String)>,
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

/// Plan a lights group call from a kiosk request. `action` is `"on"` or
/// `"off"` (case-insensitive). Uses the `homeassistant` domain so the
/// same call works whether the entity is a `group.`, `light.`, or scene.
pub fn plan_lights(group: &str, action: &str, cfg: &HaConfig) -> Result<HaCall, HaError> {
    let entity = cfg
        .resolve_group(group)
        .ok_or_else(|| HaError::UnknownLightGroup(group.to_string()))?;
    let service = match action.to_ascii_lowercase().as_str() {
        "on" => "turn_on",
        "off" => "turn_off",
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
    }
}
