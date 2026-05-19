//! Slice-2 composition integration test: drive the real `context::Engine`
//! through realistic bedroom sequences and assert the HA call sequence
//! that `ha::apply_engine_action` produces from its emitted actions.
//!
//! Slice 1 unit-tests the engine; Slice 2 unit-tests the HA planners
//! separately. This proves the *composition* through the public API
//! (Built ≠ Verified): the autonomous-power path actually yields the
//! correct switch on/off calls, and screen states never actuate HA.
//! Pure + std-only — no async, no network, deterministic.

use std::time::Duration;

use shannon_kiosk::context::{Config, Engine, Inputs, Manual, Media};
use shannon_kiosk::ha::{apply_engine_action, HaConfig};

/// Feed one tick; return the HA calls (domain, service) the host would
/// fire for the engine's emitted actions this tick.
fn tick(eng: &mut Engine, ha: &HaConfig, i: &Inputs) -> Vec<(String, String)> {
    eng.step(i)
        .actions
        .iter()
        .filter_map(|a| apply_engine_action(a, ha))
        .map(|c| (c.domain, c.service))
        .collect()
}

fn base() -> Inputs {
    Inputs {
        now: shannon_kiosk::context::ClockMinutes::at(15, 0),
        controller_connected: true,
        since_controller_input: Duration::from_secs(1),
        fresh_controller_input: false,
        media: Media::None,
        outdoor_brightness: 0.6,
        manual_press: None,
    }
}

#[test]
fn day_lifecycle_powers_on_once_and_off_once() {
    let ha = HaConfig::default();
    let mut e = Engine::new(Config::default());
    let mut i = base();

    // Off -> Kiosk: exactly one switch/turn_on, nothing else.
    assert_eq!(
        tick(&mut e, &ha, &i),
        vec![("switch".to_string(), "turn_on".to_string())]
    );

    // Kiosk -> Ambient (idle, day): still powered → NO HA call
    // (Show is the host's job, not Home Assistant's).
    i.since_controller_input = Duration::from_secs(10 * 60);
    assert!(tick(&mut e, &ha, &i).is_empty());

    // Ambient -> Off (past the ~2.5 h day leash): one switch/turn_off.
    i.since_controller_input = Duration::from_secs(200 * 60);
    assert_eq!(
        tick(&mut e, &ha, &i),
        vec![("switch".to_string(), "turn_off".to_string())]
    );

    // Idempotent: still Off, no further calls.
    assert!(tick(&mut e, &ha, &i).is_empty());
}

#[test]
fn music_powers_tv_off_then_controller_brings_it_back() {
    let ha = HaConfig::default();
    let mut e = Engine::new(Config::default());
    let mut i = base();
    assert_eq!(
        tick(&mut e, &ha, &i).first().map(|c| c.1.as_str()),
        Some("turn_on")
    );

    // Music with no active controller → TV off.
    i.media = Media::Music;
    i.since_controller_input = Duration::from_secs(30 * 60);
    assert_eq!(
        tick(&mut e, &ha, &i),
        vec![("switch".to_string(), "turn_off".to_string())]
    );

    // Grab the controller again → back to Kiosk → powered on.
    i.since_controller_input = Duration::from_secs(1);
    assert_eq!(
        tick(&mut e, &ha, &i),
        vec![("switch".to_string(), "turn_on".to_string())]
    );
}

#[test]
fn night_content_guardrail_keeps_tv_on_no_off_call() {
    let ha = HaConfig::default();
    let mut e = Engine::new(Config::default());
    let mut i = base();
    i.media = Media::Video;
    i.now = shannon_kiosk::context::ClockMinutes::at(23, 30);
    i.since_controller_input = Duration::from_secs(180 * 60);
    // Deep evening, very long idle, but Content is engaged → power on,
    // and crucially the night leash must NOT emit a turn_off.
    let calls = tick(&mut e, &ha, &i);
    assert_eq!(calls, vec![("switch".to_string(), "turn_on".to_string())]);
    // Hold the same state another tick: no spurious off.
    assert!(tick(&mut e, &ha, &i).is_empty());
}

#[test]
fn manual_force_off_drives_one_off_and_holds() {
    let ha = HaConfig::default();
    let mut e = Engine::new(Config::default());
    let mut i = base();

    // A fresh engine is Off; bring the TV on first (Off -> Kiosk), else
    // ForceOff-from-Off is a correct no-op (no spurious turn_off on an
    // already-off plug — the intended non-churn behaviour).
    assert_eq!(
        tick(&mut e, &ha, &i),
        vec![("switch".to_string(), "turn_on".to_string())]
    );

    // Now ForceOff while content is playing → exactly one turn_off
    // (manual override beats Content).
    i.media = Media::Video;
    i.manual_press = Some(Manual::ForceOff);
    assert_eq!(
        tick(&mut e, &ha, &i),
        vec![("switch".to_string(), "turn_off".to_string())]
    );

    // Sticky: release the press, no churn (no repeated turn_off).
    i.manual_press = None;
    assert!(tick(&mut e, &ha, &i).is_empty());
}
