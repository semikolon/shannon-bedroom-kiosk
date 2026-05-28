//! `shannon-kiosk` library — the bedroom display/power **context engine**
//! plus the **Home Assistant actuator planning** layer.
//!
//! The Bevy binary (`main.rs`) renders the UI. The axum action-daemon
//! binary (`shannon-kiosk-actions`) is the host that applies the engine's
//! emitted actions (TV power + lights) via Home Assistant REST.
//!
//! Both modules are pure + std-only (zero Bevy / async / network / clock
//! dependency): every signal is injected, every decision and every HA
//! *plan* is a pure function, so the whole core is unit-testable on any
//! host (Mac Mini dev today — no smart-plug, no Shannon, no live HA, no
//! GPU). The live reqwest/HTTP send lives only in the daemon binary.
//!
//! Design hub:
//! `~/dotfiles/docs/shannon_kiosk_phase3a_display_power_engine_design_2026_05_19.md`

pub mod context;
pub mod ha;
pub mod ha_poll;
pub mod spela_control_proto;
