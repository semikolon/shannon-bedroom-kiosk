//! Shannon bedroom kiosk — Phase 3 production UI on Bevy 0.18.1.
//!
//! Migrated from Bevy 0.14 → 0.18.1 (2026-05-20 user-greenlit work).
//! Engine-driven: the Slice-1 `context::Engine` decides DisplayState
//! (Off/Kiosk/Content/Ambient) each tick; the Slice-3a `Engine::hint`
//! predicts cursor + ribbon for the Kiosk state. Bevy renders the
//! current state.
//!
//! Visual direction (canonical: dotfiles/docs/
//! shannon_kiosk_phase3a_display_power_engine_design_2026_05_19.md § 13):
//! - **No display title** ("let the other stuff take focus")
//! - **Sharp Sans typography** + Lucide monochrome icons (single-accent
//!   amber discipline; no rainbow icon-tinting)
//! - **Sarpetorp forest palette** mirrored from the dashboard
//! - **Six tiles**: Games / Music / Lights / Watch / Sensors / Sleep
//! - **Y button = ALL OFF** (engine `Manual::ForceOff`)
//!
//! Render path: HW-GLES via Mesa Panfrost on Mali T860 (Shannon target).
//! The vendored wgpu-hal-0.21 Mali patch from the Bevy-0.14 era is
//! commented out in Cargo.toml; Bevy 0.18 pulls wgpu 27, which may have
//! the relevant upstream fixes (wgpu PRs #7952 + #9153). If Mali HW-GLES
//! breaks on Shannon, the patch needs porting to wgpu-hal 27 as a
//! follow-up commit (see design hub § 13.17).
//!
//! Bevy 0.15-0.18 API migration applied here:
//! - Camera2dBundle → Camera2d (Required Components)
//! - TextBundle → tuple-spawn (Text + TextFont + TextColor + Node)
//! - NodeBundle → tuple-spawn (Node + BackgroundColor + BorderColor)
//! - SpriteBundle → tuple-spawn (Sprite + Transform)
//! - Style { ... } merged into Node { ... } (sibling component instead
//!   of nested-on-bundle)
//! - text.sections[0].value → text.0; .sections[0].style.color → query
//!   the sibling TextColor component
//! - Query::get_single_mut() → single_mut()
//! - Res<Gamepads> → Query<&Gamepad>; GamepadButtonType → GamepadButton;
//!   GamepadAxisType → GamepadAxis; event.button_type → event.button
//! - WindowResolution::new() instead of (f32, f32).into()
//! - RenderAssetUsages + CompressedImageFormats moved to bevy::image
//!   (Bevy 0.18 split bevy_image into its own crate)

use bevy::asset::RenderAssetUsages;
use bevy::input::gamepad::{GamepadAxisChangedEvent, GamepadButtonChangedEvent};
use bevy::log::{info, warn};
use bevy::prelude::*;
#[cfg(target_os = "linux")]
use bevy::render::settings::Backends;
use bevy::render::settings::{WgpuLimits, WgpuSettings, WgpuSettingsPriority};
use bevy::render::RenderPlugin;
use bevy::window::WindowResolution;
use bevy::winit::WinitSettings;
use shannon_kiosk::context::{
    Action, BlackoutTvPower, ClockMinutes, Config, DisplayState, Engine, Inputs, Manual, Media,
    MenuItem, TvPower,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ─── Sarpetorp forest palette (mirrors the dashboard) ────────────────
const FOREST_BG: Color = Color::srgb(0.071, 0.118, 0.078); // rgb(18,30,20)
const OAT_MILK: Color = Color::srgb(0.957, 0.937, 0.898); // primary text
const OAT_DIM: Color = Color::srgb(0.55, 0.56, 0.49); // secondary text
const OAT_FAINT: Color = Color::srgb(0.38, 0.39, 0.36); // tertiary text
const AMBER_ACCENT: Color = Color::srgb(0.94, 0.71, 0.18); // selected + [A]

// ─── Embedded font assets (commit-time bundled into the binary) ──────
const SHARP_SANS_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/SharpSans-Semibold.otf");
const SHARP_SANS_BOLD: &[u8] = include_bytes!("../assets/fonts/SharpSans-Bold.otf");
const LUCIDE: &[u8] = include_bytes!("../assets/fonts/Lucide.ttf");

// ─── Embedded background image ──────────────────────────────────────
const BG_IMAGE: &[u8] = include_bytes!("../assets/backgrounds/sarpetorp-clock-bg.jpg");
const BG_FIT_WIDTH: f32 = 1920.0;
const BG_FIT_HEIGHT: f32 = 3063.0;
const BG_OPACITY: f32 = 0.20;

// ─── Sidebar wood-panel background (mirrors Sarpetorp dashboard
// INOMHUS widget, App.tsx:97). The dashboard's 0.12 opacity sits on a
// light glass-card surface; here the underlying FOREST_BG is much
// darker, so we need higher opacity for the wood to read at all while
// preserving a "green-faded" appearance per Fredrik 2026-05-21.
//
// 2026-05-21 lessons stacked on Bevy 0.18 ImageNode:
// 1. `NodeImageMode::Auto` (default) renders the image at its NATURAL
//    pixel dimensions and IGNORES the Node's width/height for sizing.
// 2. `NodeImageMode::Stretch` SHOULD stretch to Node bounds but
//    empirically overshoots slightly on Mali Panfrost (observed ~11 px
//    right overlap + ~73 px short vertically, screenshot 2026-05-21).
// Workaround: pre-resize the asset to EXACTLY the sidebar dimensions
// (sips → 540×1080) so Auto mode renders 1:1 with the Node — no stretch
// math, no rendering quirks. The intermediate portrait crop is kept for
// reference but the in-binary include points at the 540×1080 final. ──
const SIDEBAR_WOOD_IMAGE: &[u8] =
    include_bytes!("../assets/backgrounds/wood-panel-bg-540x1080.jpg");
const SIDEBAR_WIDTH: f32 = 540.0;
const SIDEBAR_HEIGHT: f32 = 1080.0;
const SIDEBAR_OPACITY: f32 = 0.40;

// ─── Lucide codepoints for the six menu tiles ────────────────────────
const ICON_GAMES: char = '\u{e0df}'; // gamepad-2
const ICON_MUSIC: char = '\u{e122}'; // music
const ICON_LIGHTS: char = '\u{e1c2}'; // lightbulb
const ICON_WATCH: char = '\u{e481}'; // play-square
const ICON_SENSORS: char = '\u{e038}'; // activity
const ICON_SLEEP: char = '\u{e11e}'; // moon

// ─── Menu definition (six tiles per design § 13.1) ───────────────────
struct TileSpec {
    item: MenuItem,
    label: &'static str,
    icon: char,
}

const MENU: &[TileSpec] = &[
    TileSpec {
        item: MenuItem::Games,
        label: "GAMES",
        icon: ICON_GAMES,
    },
    TileSpec {
        item: MenuItem::Music,
        label: "MUSIC",
        icon: ICON_MUSIC,
    },
    TileSpec {
        item: MenuItem::Lights,
        label: "LIGHTS",
        icon: ICON_LIGHTS,
    },
    TileSpec {
        item: MenuItem::Watch,
        label: "WATCH",
        icon: ICON_WATCH,
    },
    TileSpec {
        item: MenuItem::Sensors,
        label: "SENSORS",
        icon: ICON_SENSORS,
    },
    TileSpec {
        item: MenuItem::Sleep,
        label: "SLEEP",
        icon: ICON_SLEEP,
    },
];

fn menu_index_of(item: MenuItem) -> usize {
    MENU.iter()
        .position(|t| t.item == item)
        .expect("MENU must contain every MenuItem variant")
}

// ─── Lights submenu (Fredrik 2026-05-21: A on LIGHTS opens a group
// picker; A on a group toggles it; B returns to root). ──────────────
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
enum MenuLevel {
    #[default]
    Root,
    LightsSubmenu,
}

struct SubmenuTile {
    /// Kiosk-group name passed to the daemon's /lights/:group/:action.
    /// Must be a key in `HaConfig::light_groups` on the daemon side
    /// (bedroom / office / hallway in the current config).
    group: &'static str,
    label: &'static str,
    icon: char,
}

const LIGHTS_SUBMENU: &[SubmenuTile] = &[
    SubmenuTile {
        group: "bedroom",
        label: "BEDROOM",
        icon: ICON_LIGHTS,
    },
    SubmenuTile {
        group: "office",
        label: "OFFICE",
        icon: ICON_LIGHTS,
    },
    SubmenuTile {
        group: "hallway",
        label: "HALLWAY",
        icon: ICON_LIGHTS,
    },
];

// ─── Bevy resources ──────────────────────────────────────────────────

#[derive(Resource)]
struct EngineRes {
    engine: Engine,
    state: DisplayState,
    cursor: MenuItem,
    since_input: Duration,
    fresh_controller_input: bool,
    manual_press: Option<Manual>,
    dev_keyboard_active: bool,
    // Slice 3d/3e — populated by the HA-state poller thread from the
    // shannon-kiosk-actions daemon's /ha-state endpoint. media drives
    // the engine's Content/Ambient precedence; ha_ribbon_title supplies
    // the resume-last-watched ribbon offer text; ha_occupancy is a
    // future presence signal (currently unused by the engine — Slice
    // 3e wires it into the controller-BT oracle as a supplementary
    // signal when bedroom_occupancy is configured in HA).
    media: Media,
    outdoor_brightness: f32,
    ha_ribbon_title: Option<String>,
    ha_occupancy: bool,
    /// Shared snapshot read on every Bevy tick. None until first
    /// successful poll (daemon down on dev iteration). Mac dev runs
    /// fine without a daemon — engine just uses defaults.
    ha_snapshot: Option<Arc<Mutex<HaSnapshot>>>,
    /// Computed by `engine_tick_system` (Slice 3e); read by
    /// `ribbon_render_system`. `None` keeps the ribbon hidden.
    ribbon_text: Option<String>,
    /// Slice 3f: BlackoutTvPower actuator. The default TV-off path —
    /// paints a black scene instead of cutting HDMI signal (preserves
    /// Argon DA2 keepalive). When `is_on()` is false, all kiosk UI is
    /// hidden via `BlackoutRoot` visibility. The engine's
    /// SetTvPower(_) Action drives this on `step()`.
    tv_power: BlackoutTvPower,
    /// Tracks whether the engine was in `DisplayState::Kiosk` on the
    /// previous tick. Used to gate cursor-prediction-pre-positioning
    /// (design hub § 13.7) so it fires only on TRANSITION INTO Kiosk
    /// (from Off/Ambient/Content), not every non-fresh frame nor on
    /// an arbitrary idle timeout. The "snap-back" bug fix.
    was_in_kiosk: bool,
    /// Current menu level — Root shows the 6-tile main menu; LightsSubmenu
    /// shows the 3 light groups (bedroom/office/hallway). Transitions:
    /// A on cursor=Lights at Root → LightsSubmenu (saved_root_cursor =
    /// Lights). B at LightsSubmenu → Root (cursor restored). Exit-Kiosk
    /// resets to Root (don't carry submenu state across visibility gaps).
    menu_level: MenuLevel,
    /// Cursor index within the current submenu (0..LIGHTS_SUBMENU.len()
    /// when menu_level == LightsSubmenu; unused at Root).
    submenu_cursor: usize,
    /// Timestamp of the last lights-toggle dispatch — used to debounce
    /// rapid presses (Fredrik 2026-05-21: barrage of X crashed the
    /// kiosk by exhausting thread/runtime resources in
    /// `spawn_daemon_lights_post`). Caps dispatch rate to ~1 per
    /// `LIGHTS_DEBOUNCE_MS`; faster presses are logged + dropped. HA's
    /// own toggle round-trip is the floor on perceivable speed anyway.
    last_lights_action_at: Option<std::time::Instant>,
    /// Timestamp of the last Watch dispatch (POST /watch → daemon spawns
    /// `spela-local`) — separate debounce field from lights because a
    /// Watch action kicks off a long-lived playback pipeline (search →
    /// Darwin NVENC cold-start → mpv launch under cage) whose
    /// cost-of-double-press is far higher than a lights toggle. Capped
    /// at `WATCH_DEBOUNCE_MS` (2 s) so two rapid A-presses don't fire
    /// two parallel spela-locals — Darwin spela serves one stream at a
    /// time; duplicate triggers would race the stream-replacement path.
    last_watch_action_at: Option<std::time::Instant>,
    /// Timestamp of the last Music dispatch (POST /media → daemon HA
    /// `media_player.media_play_pause`). Separate debounce field from
    /// lights/watch — Music's HA round-trip is ~500 ms (audio backend
    /// has its own response latency); rapid A-presses on the Music tile
    /// would race the play↔pause flip. `MUSIC_DEBOUNCE_MS` matches.
    last_media_action_at: Option<std::time::Instant>,
}

/// One snapshot of HA state from the daemon's /ha-state endpoint.
/// Updated by the poller thread; consumed by `engine_tick_system`.
#[derive(Clone, Debug, Default)]
struct HaSnapshot {
    media: Media,
    resumable_title: Option<String>,
    occupancy_present: bool,
    /// Wall-clock time the snapshot was last refreshed (for staleness
    /// detection — Bevy renders without HA data if last poll > 5×
    /// poll_interval old).
    refreshed_at: Option<std::time::Instant>,
}

impl Default for EngineRes {
    fn default() -> Self {
        Self {
            engine: Engine::new(Config::default()),
            state: DisplayState::Off,
            cursor: MenuItem::default_fallback(),
            since_input: Duration::from_secs(60),
            fresh_controller_input: false,
            manual_press: None,
            dev_keyboard_active: false,
            media: Media::None,
            outdoor_brightness: 0.6,
            ha_ribbon_title: None,
            ha_occupancy: false,
            ha_snapshot: None,
            ribbon_text: None,
            // Initial TV state: ON (the kiosk renders content). The
            // engine's first step() emits SetTvPower(_) based on its
            // computed state, so this default is only the boot-time
            // value before the first tick.
            tv_power: {
                let mut tv = BlackoutTvPower::default();
                tv.set_power(true);
                tv
            },
            // Start `was_in_kiosk = false` so the FIRST transition into
            // Kiosk (from initial Off state) fires the cursor pre-
            // position. After that, only re-entries pre-position.
            was_in_kiosk: false,
            menu_level: MenuLevel::Root,
            submenu_cursor: 0,
            last_lights_action_at: None,
            last_watch_action_at: None,
            last_media_action_at: None,
        }
    }
}

#[derive(Resource)]
struct FontHandles {
    semibold: Handle<Font>,
    bold: Handle<Font>,
    lucide: Handle<Font>,
}

#[derive(Component)]
struct MenuLabel {
    index: usize,
}

#[derive(Component)]
struct MenuIcon {
    index: usize,
}

/// Legacy marker — cursor markers were removed 2026-05-21 (per Fredrik
/// the text-color contrast is sufficient selection indicator). No
/// entities are spawned with this; menu_render_system still includes
/// an empty-iter query for trivial future re-introduction.
#[derive(Component)]
struct MenuCursorMarker;

/// Marker for the Lights submenu's icon entities (spawned alongside the
/// root menu icons at the same y positions; visibility flipped by
/// menu_render_system based on engine_res.menu_level).
#[derive(Component)]
struct LightsSubmenuIcon {
    index: usize,
}

/// Marker for the Lights submenu's label entities (BEDROOM / OFFICE /
/// HALLWAY). Paired with LightsSubmenuIcon — sibling visibility.
#[derive(Component)]
struct LightsSubmenuLabel {
    index: usize,
}

#[derive(Component)]
struct StateBadge;

/// Marker for the resume-offer ribbon text (Slice 3e). One line above
/// the controller chrome bar; visibility flips on/off per the engine's
/// KioskHint.ribbon (confident-or-quiet — design § 1).
#[derive(Component)]
struct RibbonLabel;

/// Marker for the full-screen black overlay (Slice 3f). When the
/// engine drives `tv_power` OFF (Off/Content/Ambient transitions
/// that set TV power), this overlay covers the entire kiosk UI with
/// pure black — preserves HDMI signal so cage stays the compositor
/// and Argon DA2's keepalive isn't broken (design § 13.6).
#[derive(Component)]
struct BlackoutOverlay;

/// Marker for the Ambient scene root (Slice 3g). Full-screen amber
/// canvas modulated by the engine's adaptive-dim brightness (design
/// § 5: outdoor-solar-curve × time-of-day, monotonic + bounded).
/// Visible only when DisplayState::Ambient(_); hidden otherwise.
#[derive(Component)]
struct AmbientCanvas;

/// Marker for the three preview-pane text spawns; lets one query update
/// all of them via `match` on the variant. Single-component-with-variant
/// keeps the system signature simple (clippy-friendly) vs three
/// independent marker types.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum PreviewElement {
    Icon,
    Label,
    Subtitle,
}

fn preview_for(item: MenuItem) -> (char, &'static str, &'static str) {
    match item {
        MenuItem::Games => (ICON_GAMES, "GAMES", "Recently played"),
        MenuItem::Music => (ICON_MUSIC, "MUSIC", "Now playing"),
        MenuItem::Lights => (ICON_LIGHTS, "LIGHTS", "Bedroom · Office · Hallway"),
        MenuItem::Watch => (ICON_WATCH, "WATCH", "Continue watching"),
        MenuItem::Sensors => (ICON_SENSORS, "SENSORS", "Inomhus · Väder · Tank"),
        MenuItem::Sleep => (ICON_SLEEP, "SLEEP", "Goodnight"),
    }
}

// ─── App entry ──────────────────────────────────────────────────────

fn main() {
    App::new()
        .insert_resource({
            // Reactive {wait:10ms} — the empirically-chosen default after
            // the 2026-05-21 latency A/B (design hub § 13.28). The previous
            // 33ms wait was the upper bound on gamepad-input delivery in
            // Reactive mode because gilrs events don't trigger winit
            // wake-up — they sit in the gilrs queue until the next idle
            // tick fires the Bevy loop. The continuous-mode (game())
            // alternative shifted natural-pace median latency 125→67 ms but
            // the slow tail (140-192 ms outliers) survived the mode swap,
            // confirming the bottleneck is upstream of Bevy (likely Xbox
            // controller BT radio jitter; deferred to a dedicated arc).
            // 10ms wait captures the fast-cluster improvement without the
            // continuous-loop CPU floor — best-of-both per Shannon's freq-
            // capped (1.008 GHz / 400 MHz GPU) thermal budget.
            //
            // The prior SHANNON_KIOSK_CONTINUOUS env-gate was dropped as
            // dead code post-A/B: continuous mode delivered only marginal
            // median win and the slow-tail wasn't Bevy-flag-solvable.
            WinitSettings {
                focused_mode: bevy::winit::UpdateMode::Reactive {
                    wait: Duration::from_millis(10),
                    react_to_device_events: true,
                    react_to_user_events: true,
                    react_to_window_events: true,
                },
                unfocused_mode: bevy::winit::UpdateMode::Reactive {
                    wait: Duration::from_millis(10),
                    react_to_device_events: true,
                    react_to_user_events: true,
                    react_to_window_events: true,
                },
            }
        })
        .insert_resource(ClearColor(FOREST_BG))
        .insert_resource({
            // Slice 3e: start the HA-state poller before the engine
            // resource initializes — so EngineRes can hold the shared
            // snapshot handle from the start. SHANNON_KIOSK_DAEMON_URL
            // overrides the default localhost daemon (defaults to the
            // daemon binary's bind address). HA_POLL_INTERVAL_SECS
            // controls Bevy-side cadence; the daemon has its own poll
            // interval — pick this one to match HA-state freshness
            // requirements (3s is responsive for paused-media → ribbon
            // updates while staying cheap on the daemon).
            let daemon_url = std::env::var("SHANNON_KIOSK_DAEMON_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
            let interval_secs = std::env::var("HA_POLL_INTERVAL_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(3);
            let snap =
                spawn_ha_state_poller(daemon_url.clone(), Duration::from_secs(interval_secs));
            EngineRes {
                ha_snapshot: Some(snap),
                ..Default::default()
            }
        })
        .insert_resource({
            // Daemon URL also kept as its own resource so outbound POST
            // helpers (X-toggle, future Lights submenu A-select) can
            // access it from gamepad_event_system without threading it
            // through EngineRes.
            let daemon_url = std::env::var("SHANNON_KIOSK_DAEMON_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
            DaemonUrl(daemon_url)
        })
        .add_plugins(
            DefaultPlugins
                .set(RenderPlugin {
                    render_creation: WgpuSettings {
                        priority: WgpuSettingsPriority::WebGL2,
                        limits: {
                            let mut l = WgpuLimits::downlevel_webgl2_defaults();
                            l.max_texture_dimension_2d = 4096;
                            l
                        },
                        #[cfg(target_os = "linux")]
                        backends: Some(Backends::GL),
                        ..default()
                    }
                    .into(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Shannon".to_string(),
                        resolution: WindowResolution::new(1920, 1080),
                        resizable: true,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_systems(Startup, (load_fonts, setup_background, setup_ui).chain())
        .add_systems(
            Update,
            (
                gamepad_event_system,
                keyboard_event_system,
                engine_tick_system,
                menu_render_system,
                preview_render_system,
                ribbon_render_system,
                ambient_render_system,
                blackout_render_system,
                state_badge_system,
            ),
        )
        .run();
}

/// Image handle for the sidebar wood-panel background. Loaded in
/// setup_background, consumed by setup_ui's sidebar ImageNode spawn.
#[derive(Resource)]
struct SidebarBgHandle(Handle<Image>);

/// URL of the local shannon-kiosk-actions daemon. Read once at startup
/// and copied into outbound POST helpers (`spawn_daemon_lights_post`).
/// Mirrors the value used by `spawn_ha_state_poller` for inbound polling.
#[derive(Resource, Clone)]
struct DaemonUrl(String);

/// Debounce window for lights-toggle dispatch. Caps the rate at which
/// the X-button (and Lights submenu A) can fire daemon POSTs. Each call
/// spawns a thread + reqwest tokio runtime that holds for the HA toggle
/// round-trip; barraging without a debounce piles up threads until
/// `pthread_create` returns EAGAIN and the kiosk panics. 300 ms is also
/// the floor on what the user can perceive as a separate light change.
const LIGHTS_DEBOUNCE_MS: u64 = 300;

/// Fire-and-forget POST to the daemon's `/lights/:group/:action`. Used by
/// the X-button bedroom-quick-toggle (Fredrik 2026-05-21: *"X when Lights
/// is selected (or really why not regardless of choice) can be a quick
/// btn to toggle bedroom lights off/on"*) and by the Lights submenu's A-
/// select. Spawns a short-lived std::thread + reqwest::blocking with a 5s
/// timeout — never blocks the Bevy main loop. Result is logged but no
/// retry / no callback; the daemon already has its own retry semantics
/// for HA failures (see ha_state_poll_loop for the pattern this mirrors).
///
/// Thread spawn failures (EAGAIN under load) are logged and dropped —
/// the previous `.expect` panicked the Bevy main thread, which is how a
/// rapid-fire X barrage crashed the kiosk on 2026-05-21. Callers should
/// rate-limit via `try_dispatch_lights` rather than calling this
/// directly.
fn spawn_daemon_lights_post(daemon_url: String, group: String, action: String) {
    let result = std::thread::Builder::new()
        .name(format!("daemon-post-lights-{group}-{action}"))
        .spawn(move || {
            let url = format!(
                "{}/lights/{}/{}",
                daemon_url.trim_end_matches('/'),
                group,
                action
            );
            let client = match reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    warn!("daemon-post: client build failed: {e}");
                    return;
                }
            };
            match client.post(&url).send() {
                Ok(resp) => info!("daemon POST {} → {}", url, resp.status()),
                Err(e) => warn!("daemon POST {} failed: {}", url, e),
            }
        });
    if let Err(e) = result {
        // Resource exhaustion (EAGAIN/ENOMEM from pthread_create). Drop
        // the action rather than panic — the user can press again.
        warn!("daemon-post: thread spawn failed (resource limit?): {e}");
    }
}

/// Debounced dispatch helper. Returns true if the toggle was sent;
/// false if throttled (within `LIGHTS_DEBOUNCE_MS` of the previous
/// dispatch). Wraps `spawn_daemon_lights_post` so every call site goes
/// through the same gate — the crash on 2026-05-21 came from three
/// independent call sites (X global toggle, Lights-submenu A, future
/// per-tile X) sharing no rate limit.
fn try_dispatch_lights(
    engine_res: &mut EngineRes,
    daemon_url: &DaemonUrl,
    group: &str,
    action: &str,
) -> bool {
    let now = std::time::Instant::now();
    if let Some(last) = engine_res.last_lights_action_at {
        if now.duration_since(last) < Duration::from_millis(LIGHTS_DEBOUNCE_MS) {
            info!("lights {} {} throttled (rapid press)", group, action);
            return false;
        }
    }
    engine_res.last_lights_action_at = Some(now);
    spawn_daemon_lights_post(daemon_url.0.clone(), group.to_string(), action.to_string());
    true
}

/// Multi-group debounced dispatch — atomic across all groups in one
/// debounce window. Used by the X-button "toggle all lights" action
/// (Fredrik 2026-05-21: *"make it ALL the lights"*) and any future
/// composite-action call site. A single 300 ms gate covers the whole
/// burst of N daemon POSTs; faster presses log "throttled" and drop.
///
/// Spawns N independent threads — each one a separate POST to
/// `/lights/<group>/<action>`. The `spawn_daemon_lights_post` helper
/// is panic-proof on thread-spawn failure (logs `warn!`, drops the
/// individual call) so even if pthread_create returns EAGAIN under
/// load, the kiosk continues.
///
/// Returns true on first call within the debounce window (any in the
/// group set), false if throttled.
fn try_dispatch_lights_multi(
    engine_res: &mut EngineRes,
    daemon_url: &DaemonUrl,
    groups: &[&str],
    action: &str,
) -> bool {
    let now = std::time::Instant::now();
    if let Some(last) = engine_res.last_lights_action_at {
        if now.duration_since(last) < Duration::from_millis(LIGHTS_DEBOUNCE_MS) {
            info!(
                "lights multi-{} ({} groups) throttled (rapid press)",
                action,
                groups.len()
            );
            return false;
        }
    }
    engine_res.last_lights_action_at = Some(now);
    for group in groups {
        spawn_daemon_lights_post(daemon_url.0.clone(), group.to_string(), action.to_string());
    }
    true
}

/// Groups touched by the X-button "toggle all lights" action (Fredrik
/// 2026-05-21). Hallway intentionally EXCLUDED because
/// `group.hallway_indicator` is presence-service-driven and an X press
/// would race the automation. The child-lock-style entities (Tuya
/// device-firmware sub-features that integrations surface as their own
/// switch entities) are structurally excluded — they're not in any
/// kiosk-known group. See `ha.rs::HaConfig::light_groups` for the
/// codified principle.
const X_ALL_TOGGLE_GROUPS: &[&str] = &["bedroom", "office"];

/// Debounce window for Watch dispatch (Phase-7 spela-thin-client
/// Layer-4b). Far longer than `LIGHTS_DEBOUNCE_MS` because a Watch
/// action kicks off a long playback pipeline; pressing A twice in
/// quick succession should not fire two spela-locals (Darwin spela
/// serves one stream at a time — duplicate triggers race the
/// stream-replacement path). 2 s is generous enough for "did it
/// register?" double-presses without imposing perceptible lag.
const WATCH_DEBOUNCE_MS: u64 = 2000;

/// Default smoke-test title for the Watch tile (Phase-7 Layer-4b v1).
/// Fredrik's canonical spela search example — used throughout spela
/// docs as `spela search "Good Luck Have Fun Dont Die"`. Hardcoded for
/// first-light; the title-picker UX (resume-last vs context-engine
/// vs in-Bevy search per kiosk research § 7 Ranks 1+7) is a separate
/// arc. To override before that arc lands: flip this const and
/// rebuild.
const WATCH_SMOKE_TITLE: &str = "Good Luck Have Fun Dont Die";

/// Fire-and-forget POST to the daemon's `/watch` endpoint with a JSON
/// body `{"title": "<query>"}`. The daemon's `watch_handler` returns
/// 202 immediately after spawning `spela-local <title>` detached, so
/// this call clears in <50 ms; the actual playback pipeline (Darwin
/// transcode cold-start + cage/mpv launch) runs server-side.
///
/// Mirrors `spawn_daemon_lights_post`'s shape — short-lived
/// std::thread + reqwest::blocking with a 5s timeout, never blocks the
/// Bevy main loop. Thread-spawn failures (EAGAIN under load) are
/// logged + dropped rather than panicking.
fn spawn_daemon_watch_post(daemon_url: String, title: String) {
    let result = std::thread::Builder::new()
        .name(format!("daemon-post-watch-{}", title.replace(' ', "-")))
        .spawn(move || {
            let url = format!("{}/watch", daemon_url.trim_end_matches('/'));
            let client = match reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    warn!("daemon-post-watch: client build failed: {e}");
                    return;
                }
            };
            let body = serde_json::json!({ "title": title });
            match client.post(&url).json(&body).send() {
                Ok(resp) => info!(
                    "daemon POST {} (title={:?}) → {}",
                    url,
                    title,
                    resp.status()
                ),
                Err(e) => warn!("daemon POST {} (title={:?}) failed: {}", url, title, e),
            }
        });
    if let Err(e) = result {
        warn!("daemon-post-watch: thread spawn failed (resource limit?): {e}");
    }
}

/// Debounced dispatch helper for Watch. Returns true if the request
/// was sent, false if throttled (within `WATCH_DEBOUNCE_MS` of the
/// previous dispatch). Wraps `spawn_daemon_watch_post` so the
/// match-arm call site stays one line. Separate debounce field
/// (`last_watch_action_at`) from lights — see field doc for why.
fn try_dispatch_watch(engine_res: &mut EngineRes, daemon_url: &DaemonUrl, title: &str) -> bool {
    let now = std::time::Instant::now();
    if let Some(last) = engine_res.last_watch_action_at {
        if now.duration_since(last) < Duration::from_millis(WATCH_DEBOUNCE_MS) {
            info!("watch {:?} throttled (rapid press)", title);
            return false;
        }
    }
    engine_res.last_watch_action_at = Some(now);
    spawn_daemon_watch_post(daemon_url.0.clone(), title.to_string());
    true
}

/// Music tile (`MenuItem::Music`) debounce window. Caps the rate at
/// which the Music A-arm can fire daemon POSTs. HA's `media_player.*`
/// services have ~500 ms response latency on the spotifyd Spotify-
/// Connect path, so two rapid A-presses would race the play↔pause
/// flip. 500 ms slightly above HA's typical reply, well below
/// perceptible lag.
const MUSIC_DEBOUNCE_MS: u64 = 500;

/// Default Music entity key — kiosk identifier resolved by the daemon
/// against `HaConfig::media_entities`. `"default"` maps to
/// `media_player.fredrik` (spotifyd Spotify-Connect on Shannon).
/// Future per-zone routing extends the entity table on the daemon side
/// without touching the kiosk.
const MUSIC_DEFAULT_ENTITY: &str = "default";

/// Fire-and-forget POST to the daemon's `/media/{entity_key}/{action}`
/// endpoint. Mirrors `spawn_daemon_lights_post`'s shape — short-lived
/// std::thread + reqwest::blocking with a 5s timeout, panic-proof on
/// thread-spawn failure. The daemon translates `entity_key`+`action`
/// into an HA `media_player.*` service call via `plan_media`.
fn spawn_daemon_media_post(daemon_url: String, entity_key: String, action: String) {
    let result = std::thread::Builder::new()
        .name(format!("daemon-post-media-{entity_key}-{action}"))
        .spawn(move || {
            let url = format!(
                "{}/media/{}/{}",
                daemon_url.trim_end_matches('/'),
                entity_key,
                action
            );
            let client = match reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    warn!("daemon-post-media: client build failed: {e}");
                    return;
                }
            };
            match client.post(&url).send() {
                Ok(resp) => info!("daemon POST {} → {}", url, resp.status()),
                Err(e) => warn!("daemon POST {} failed: {}", url, e),
            }
        });
    if let Err(e) = result {
        warn!("daemon-post-media: thread spawn failed (resource limit?): {e}");
    }
}

/// Debounced dispatch helper for Music. Returns true if the request
/// was sent, false if throttled (within `MUSIC_DEBOUNCE_MS` of the
/// previous dispatch). Wraps `spawn_daemon_media_post` so the
/// match-arm call site stays one line. Separate debounce field
/// (`last_media_action_at`) from lights/watch — see field doc.
fn try_dispatch_media(
    engine_res: &mut EngineRes,
    daemon_url: &DaemonUrl,
    entity_key: &str,
    action: &str,
) -> bool {
    let now = std::time::Instant::now();
    if let Some(last) = engine_res.last_media_action_at {
        if now.duration_since(last) < Duration::from_millis(MUSIC_DEBOUNCE_MS) {
            info!("media {} {} throttled (rapid press)", entity_key, action);
            return false;
        }
    }
    engine_res.last_media_action_at = Some(now);
    spawn_daemon_media_post(
        daemon_url.0.clone(),
        entity_key.to_string(),
        action.to_string(),
    );
    true
}

fn setup_background(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};

    // Global atmospheric background (full screen, deepest layer).
    // The image is a deep red-burlap fabric texture from the Sarpetorp
    // dashboard's clock widget; at 20% alpha over FOREST_BG it adds warmth
    // throughout the scene without dominating any zone.
    let clock_image = Image::from_buffer(
        BG_IMAGE,
        ImageType::Extension("jpg"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Default,
        RenderAssetUsages::default(),
    )
    .expect("Sarpetorp clock-bg.jpg decodes");
    let clock_handle = images.add(clock_image);
    commands.spawn((
        Sprite {
            image: clock_handle,
            custom_size: Some(Vec2::new(BG_FIT_WIDTH, BG_FIT_HEIGHT)),
            color: Color::srgba(1.0, 1.0, 1.0, BG_OPACITY),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, -1.0),
    ));

    // Load the sidebar wood-panel image; the actual rendering is done in
    // setup_ui as a UI ImageNode (top-left-origin coords are unambiguous
    // for the sidebar zone, and a clipping parent gives proper cover-fit).
    let wood_image = Image::from_buffer(
        SIDEBAR_WOOD_IMAGE,
        ImageType::Extension("jpg"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Default,
        RenderAssetUsages::default(),
    )
    .expect("wood-panel-bg.jpg decodes");
    let wood_handle = images.add(wood_image);
    commands.insert_resource(SidebarBgHandle(wood_handle));
}

fn load_fonts(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let semibold = fonts.add(
        Font::try_from_bytes(SHARP_SANS_SEMIBOLD.to_vec()).expect("Sharp Sans Semibold loads"),
    );
    let bold =
        fonts.add(Font::try_from_bytes(SHARP_SANS_BOLD.to_vec()).expect("Sharp Sans Bold loads"));
    let lucide = fonts.add(Font::try_from_bytes(LUCIDE.to_vec()).expect("Lucide loads"));
    commands.insert_resource(FontHandles {
        semibold,
        bold,
        lucide,
    });
}

fn setup_ui(mut commands: Commands, fonts: Res<FontHandles>, sidebar_bg: Res<SidebarBgHandle>) {
    commands.spawn(Camera2d);

    // ─── Sidebar wood-panel background ────────────────────────────────
    // Image asset is pre-resized to EXACTLY 540×1080 (sips), matching the
    // sidebar Node bounds 1:1. Use NodeImageMode::Auto (default) so the
    // image renders at its natural dimensions — no Stretch quirks, no
    // overshoot, no short-fall. UI coords top-left-origin. Negative
    // GlobalZIndex pins behind menu text / chrome / preview UI.
    commands.spawn((
        ImageNode {
            image: sidebar_bg.0.clone(),
            color: Color::srgba(1.0, 1.0, 1.0, SIDEBAR_OPACITY),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(0.0),
            left: Val::Px(0.0),
            width: Val::Px(SIDEBAR_WIDTH),
            height: Val::Px(SIDEBAR_HEIGHT),
            ..default()
        },
        GlobalZIndex(-10),
    ));

    // Six-tile vertical menu — left side, generous vertical spacing.
    // Per Fredrik 2026-05-21: no cursor marker — the OAT_MILK vs OAT_DIM
    // text-color contrast is sufficient to indicate selection. The
    // MenuCursorMarker type stays around (menu_render_system still
    // queries it; the empty iter just no-ops).
    for (i, tile) in MENU.iter().enumerate() {
        let y = 240.0 + (i as f32 * 88.0);
        let label_color = if i == 0 { OAT_MILK } else { OAT_DIM };

        // Lucide icon
        commands.spawn((
            Text::new(tile.icon.to_string()),
            TextFont {
                font: fonts.lucide.clone(),
                font_size: 40.0,
                ..default()
            },
            TextColor(label_color),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(y - 4.0),
                left: Val::Px(130.0),
                ..default()
            },
            MenuIcon { index: i },
        ));

        // Label — ALL CAPS Sharp Sans Bold
        commands.spawn((
            Text::new(tile.label),
            TextFont {
                font: fonts.bold.clone(),
                font_size: 34.0,
                ..default()
            },
            TextColor(label_color),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(y),
                left: Val::Px(200.0),
                ..default()
            },
            MenuLabel { index: i },
        ));
    }

    // ─── Lights submenu (3 group tiles at the first 3 menu y positions).
    // Hidden by default; menu_render_system flips visibility based on
    // engine_res.menu_level. Pressing A on cursor=Lights at Root enters
    // this submenu; B returns to Root. Per Fredrik 2026-05-21.
    for (i, sub) in LIGHTS_SUBMENU.iter().enumerate() {
        let y = 240.0 + (i as f32 * 88.0);
        commands.spawn((
            Text::new(sub.icon.to_string()),
            TextFont {
                font: fonts.lucide.clone(),
                font_size: 40.0,
                ..default()
            },
            TextColor(OAT_DIM),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(y - 4.0),
                left: Val::Px(130.0),
                ..default()
            },
            Visibility::Hidden,
            LightsSubmenuIcon { index: i },
        ));
        commands.spawn((
            Text::new(sub.label),
            TextFont {
                font: fonts.bold.clone(),
                font_size: 34.0,
                ..default()
            },
            TextColor(OAT_DIM),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(y),
                left: Val::Px(200.0),
                ..default()
            },
            Visibility::Hidden,
            LightsSubmenuLabel { index: i },
        ));
    }

    // ─── Cursor-driven preview pane (right 2/3) ────────────────────
    let (default_icon, default_label, default_subtitle) = preview_for(MENU[0].item);

    // Subtle inner-card background to define the pane's region. Bevy
    // 0.18 has UI `BorderRadius` now (added 0.15); rounded corners can
    // be added here in a follow-up if the design wants them.
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(180.0),
            left: Val::Px(540.0),
            width: Val::Px(1320.0),
            height: Val::Px(720.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.043, 0.071, 0.047, 0.40)),
        BorderColor::all(Color::srgba(0.13, 0.18, 0.13, 0.30)),
    ));

    // Preview icon — huge Lucide glyph
    commands.spawn((
        Text::new(default_icon.to_string()),
        TextFont {
            font: fonts.lucide.clone(),
            font_size: 220.0,
            ..default()
        },
        TextColor(OAT_MILK),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(260.0),
            left: Val::Px(620.0),
            ..default()
        },
        PreviewElement::Icon,
    ));

    // Preview label — big Sharp Sans Bold ALL-CAPS
    commands.spawn((
        Text::new(default_label),
        TextFont {
            font: fonts.bold.clone(),
            font_size: 92.0,
            ..default()
        },
        TextColor(OAT_MILK),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(290.0),
            left: Val::Px(890.0),
            ..default()
        },
        PreviewElement::Label,
    ));

    // Preview subtitle — Sharp Sans Semibold, dimmed
    commands.spawn((
        Text::new(default_subtitle),
        TextFont {
            font: fonts.semibold.clone(),
            font_size: 32.0,
            ..default()
        },
        TextColor(OAT_DIM),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(410.0),
            left: Val::Px(890.0),
            ..default()
        },
        PreviewElement::Subtitle,
    ));

    // Controller chrome — vertical stack near the bottom of the sidebar,
    // shifted leftward to align with the menu's icon/label column structure
    // (per Fredrik 2026-05-21: "adjust the btn legend like 20% to the left
    // within the menu sidebar"). Glyph column matches menu icon column
    // (x = 130), label column matches roughly halfway between menu icon
    // and label (x = 175). Visually the chrome rows now read as
    // mini-menu-rows: glyph then label, sharing left-margin discipline
    // with the menu above.
    let chrome_specs = [
        ("A", "SELECT", AMBER_ACCENT),
        ("B", "BACK", OAT_DIM),
        ("X", "LIGHTS", OAT_DIM),
        ("Y", "ALL OFF", OAT_DIM),
    ];
    let chrome_y_start = 905.0;
    let chrome_row_gap = 42.0;
    let chrome_glyph_x = 130.0;
    let chrome_label_x = 175.0;
    for (i, (button, label, color)) in chrome_specs.iter().enumerate() {
        let y = chrome_y_start + (i as f32 * chrome_row_gap);
        // Button glyph — bold, glyph column
        commands.spawn((
            Text::new(button.to_string()),
            TextFont {
                font: fonts.bold.clone(),
                font_size: 26.0,
                ..default()
            },
            TextColor(*color),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(y),
                left: Val::Px(chrome_glyph_x),
                ..default()
            },
        ));
        // Action label — semibold, label column
        commands.spawn((
            Text::new(label.to_string()),
            TextFont {
                font: fonts.semibold.clone(),
                font_size: 18.0,
                ..default()
            },
            TextColor(OAT_FAINT),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(y + 4.0), // slight downshift to align baselines
                left: Val::Px(chrome_label_x),
                ..default()
            },
        ));
    }

    // Resume-offer ribbon line — sits above the controller chrome.
    // Starts hidden; engine's KioskHint.ribbon turns it on when there's
    // a confident resume offer (Slice 3e). Bevy 0.18 uses tuple-spawn
    // with Text + TextFont + TextColor + Node + Visibility instead of
    // the 0.14 TextBundle + Text::from_sections pattern. Single color
    // (amber) — the [A] chip styling lives in the rendered string
    // itself ("[A] Resume X") rather than a dual-section text.
    commands.spawn((
        Text::new(""),
        TextFont {
            font: fonts.semibold.clone(),
            font_size: 22.0,
            ..default()
        },
        TextColor(AMBER_ACCENT),
        TextLayout::new_with_justify(Justify::Center),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(965.0), // just above chrome (chrome_y=1010)
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            ..default()
        },
        Visibility::Hidden, // engine turns on when confident
        RibbonLabel,
    ));

    // Full-screen Ambient canvas (Slice 3g). Below the blackout
    // overlay (z=300) so Off state still blacks-out over Ambient.
    // Color is set per-tick by ambient_render_system to amber × engine
    // brightness; spawned at full amber so first paint isn't a flash.
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(0.0),
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            bottom: Val::Px(0.0),
            ..default()
        },
        BackgroundColor(AMBER_ACCENT),
        Visibility::Hidden, // engine flips on Ambient state
        GlobalZIndex(300),
        AmbientCanvas,
    ));

    // Full-screen blackout overlay (Slice 3f). Spawned BEFORE the
    // state badge so the badge stays visible on top (dev observability
    // — even when blackout is on you can see the engine state in the
    // corner). On Shannon production this badge will be hidden.
    // GlobalZIndex pins it just below the badge regardless of future
    // spawn-order changes.
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(0.0),
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            bottom: Val::Px(0.0),
            ..default()
        },
        BackgroundColor(Color::BLACK),
        Visibility::Hidden, // starts off; engine flips on TV-off
        GlobalZIndex(500),
        BlackoutOverlay,
    ));

    // Engine state badge (top-right) — useful for dev iteration to see
    // the engine in action. May be removed once the preview pane's
    // content fully signals the engine state contextually.
    commands.spawn((
        Text::new("—"),
        TextFont {
            font: fonts.semibold.clone(),
            font_size: 18.0,
            ..default()
        },
        TextColor(OAT_FAINT),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(30.0),
            right: Val::Px(40.0),
            ..default()
        },
        StateBadge,
    ));
}

// ─── Systems ─────────────────────────────────────────────────────────

fn gamepad_event_system(
    mut button_events: MessageReader<GamepadButtonChangedEvent>,
    mut axis_events: MessageReader<GamepadAxisChangedEvent>,
    mut engine_res: ResMut<EngineRes>,
    daemon_url: Res<DaemonUrl>,
) {
    // Snapshot the current cursor depending on which menu level is
    // active. We work in usize-space inside this system and write back
    // to engine_res.cursor / submenu_cursor at the end.
    let (mut cursor_idx, n) = match engine_res.menu_level {
        MenuLevel::Root => (menu_index_of(engine_res.cursor), MENU.len()),
        MenuLevel::LightsSubmenu => (engine_res.submenu_cursor, LIGHTS_SUBMENU.len()),
    };

    for ev in button_events.read() {
        if ev.value <= 0.5 {
            continue;
        }
        engine_res.fresh_controller_input = true;
        engine_res.since_input = Duration::ZERO;
        match ev.button {
            GamepadButton::DPadUp => {
                cursor_idx = if cursor_idx == 0 {
                    n - 1
                } else {
                    cursor_idx - 1
                };
                // Latency diagnostic 2026-05-21: pair vs evtest /dev/input/event1
                // kernel-arrival timestamp to measure kernel→Bevy delivery.
                info!(
                    "DPadUp: level={:?} cursor_idx={}",
                    engine_res.menu_level, cursor_idx
                );
            }
            GamepadButton::DPadDown => {
                cursor_idx = (cursor_idx + 1) % n;
                info!(
                    "DPadDown: level={:?} cursor_idx={}",
                    engine_res.menu_level, cursor_idx
                );
            }
            GamepadButton::South => match engine_res.menu_level {
                MenuLevel::Root => {
                    let item = MENU[cursor_idx].item;
                    info!("Selected: {:?}", item);
                    match item {
                        MenuItem::Lights => {
                            // A on LIGHTS opens the group submenu
                            // (Fredrik 2026-05-21).
                            engine_res.menu_level = MenuLevel::LightsSubmenu;
                            engine_res.submenu_cursor = 0;
                            cursor_idx = 0;
                            info!("Enter LightsSubmenu (cursor=0=bedroom)");
                        }
                        MenuItem::Sleep => {
                            // A on SLEEP = bedroom wind-down (Fredrik
                            // 2026-05-21 afternoon, Lights-before-Games
                            // sequencing): engine ForceOff (TV blackout
                            // via BlackoutTvPower) AND all-lights-off
                            // for bedroom + office. Different from Y
                            // (North) which is engine-state-only via
                            // Manual::ForceOff — Sleep adds the lights
                            // kill so "bedroom going to sleep" is a
                            // single deliberate affordance. Hallway
                            // stays automation-driven (presence-service
                            // handles it). The same X_ALL_TOGGLE_GROUPS
                            // const drives both the X-toggle and the
                            // Sleep-off path so the "what's a light to
                            // the kiosk" SSoT is one place.
                            info!(
                                "Sleep: ForceOff engine + all lights off ({:?})",
                                X_ALL_TOGGLE_GROUPS
                            );
                            engine_res.manual_press = Some(Manual::ForceOff);
                            try_dispatch_lights_multi(
                                &mut engine_res,
                                &daemon_url,
                                X_ALL_TOGGLE_GROUPS,
                                "off",
                            );
                        }
                        MenuItem::Watch => {
                            // A on WATCH fires the Phase-7 spela-thin-
                            // client (Session B 2026-05-21): POST /watch
                            // → daemon spawns `spela-local <title>` →
                            // Darwin spela NVENC-transcodes H.264 1080p
                            // HLS → Shannon mpv decodes (currently SW;
                            // patched mpv via apt.undo.it for
                            // --hwdec=drm is the Layer-2 follow-up).
                            // Debounced (`try_dispatch_watch`) at 2 s
                            // so two rapid presses don't fire two
                            // parallel spela-locals (Darwin spela
                            // serves one stream at a time).
                            //
                            // V1 uses a hardcoded smoke title
                            // (`WATCH_SMOKE_TITLE`); the title-picker
                            // UX (resume-last vs context-engine vs
                            // in-Bevy search per kiosk research § 7
                            // Ranks 1+7) is a separate arc.
                            //
                            // Scanout-handoff caveat: stock spela-local
                            // launches mpv with --vo=drm, which
                            // conflicts with cage's hold on
                            // /dev/dri/card0. First-light through this
                            // button currently requires either (a) the
                            // kiosk service stopped manually, or (b)
                            // the patched-mpv + dmabuf-wayland +
                            // shannon-mode handoff work that lands in
                            // the cluster of Phase-7 follow-ups
                            // (Layer-1 cage + Layer-2 patched mpv).
                            // The daemon /watch route + this button
                            // wiring are the ARCHITECTURAL touchpoint;
                            // the handoff is the COMPOSITION question.
                            info!("Watch: dispatching {:?}", WATCH_SMOKE_TITLE);
                            try_dispatch_watch(&mut engine_res, &daemon_url, WATCH_SMOKE_TITLE);
                        }
                        MenuItem::Music => {
                            // A on MUSIC = toggle play/pause on the
                            // default media_player entity (today:
                            // `media_player.fredrik` = spotifyd
                            // Spotify-Connect on Shannon, advertised
                            // via mDNS). Daemon's /media route does
                            // `homeassistant.media_player.media_play_pause`
                            // on whatever entity `MUSIC_DEFAULT_ENTITY`
                            // resolves to in `HaConfig::media_entities`.
                            //
                            // Toggle semantics are the right default:
                            // it works whether music is currently
                            // playing or not, and the user mental model
                            // is "press to start/stop" — no need for
                            // separate play/pause tiles.
                            //
                            // Future per-zone routing (vardagsrum /
                            // atelier / etc.) extends
                            // `HaConfig::media_entities` on the daemon
                            // + adds a sub-menu here (similar to the
                            // Lights submenu pattern). For first wire,
                            // single-entity default is enough.
                            info!("Music: toggle play/pause ({})", MUSIC_DEFAULT_ENTITY);
                            try_dispatch_media(
                                &mut engine_res,
                                &daemon_url,
                                MUSIC_DEFAULT_ENTITY,
                                "play_pause",
                            );
                        }
                        // Other tiles (Games, Sensors): South-arm
                        // wiring pending per the kiosk plan's tile-
                        // action roadmap. Games = RetroArch launch
                        // (needs cage process-model research, mode-
                        // script + daemon /mode/{m} route; design hub
                        // §13.29); Sensors = preview-pane redesign
                        // (render-system, not an action).
                        _ => {}
                    }
                }
                MenuLevel::LightsSubmenu => {
                    // A on a group toggles its lights via the daemon.
                    // Debounced — see try_dispatch_lights for the why.
                    let group = LIGHTS_SUBMENU[cursor_idx].group;
                    info!("Toggle lights group: {}", group);
                    try_dispatch_lights(&mut engine_res, &daemon_url, group, "toggle");
                }
            },
            GamepadButton::East => match engine_res.menu_level {
                MenuLevel::Root => {
                    info!("Back (no-op at Root)");
                }
                MenuLevel::LightsSubmenu => {
                    // B exits submenu, restores root cursor on Lights.
                    info!("Exit LightsSubmenu → Root (cursor restored to Lights)");
                    engine_res.menu_level = MenuLevel::Root;
                    engine_res.cursor = MenuItem::Lights;
                    cursor_idx = menu_index_of(MenuItem::Lights);
                }
            },
            GamepadButton::North => {
                engine_res.manual_press = Some(Manual::ForceOff);
                info!("ALL OFF (engine ForceOff)");
            }
            GamepadButton::West => {
                // X = global "toggle ALL lights" (Fredrik 2026-05-21
                // afternoon: *"make it ALL the lights"*). Fires
                // regardless of which menu item is selected.
                // X_ALL_TOGGLE_GROUPS = bedroom + office (hallway
                // excluded — presence-driven, would race the
                // automation). Atomic 300 ms debounce across all
                // dispatched POSTs via try_dispatch_lights_multi.
                // Crash-proof since 2026-05-21 (see ha.rs +
                // spawn_daemon_lights_post for the barrage-crash
                // history and child-lock principle).
                info!("X: toggle ALL lights ({:?})", X_ALL_TOGGLE_GROUPS);
                try_dispatch_lights_multi(
                    &mut engine_res,
                    &daemon_url,
                    X_ALL_TOGGLE_GROUPS,
                    "toggle",
                );
            }
            _ => {}
        }
    }

    for ev in axis_events.read() {
        if !matches!(ev.axis, GamepadAxis::LeftStickY) {
            continue;
        }
        if ev.value.abs() > 0.7 {
            engine_res.fresh_controller_input = true;
            engine_res.since_input = Duration::ZERO;
        }
        if ev.value > 0.7 {
            cursor_idx = if cursor_idx == 0 {
                n - 1
            } else {
                cursor_idx - 1
            };
        } else if ev.value < -0.7 {
            cursor_idx = (cursor_idx + 1) % n;
        }
    }

    // Write back the final cursor to the level-appropriate field.
    match engine_res.menu_level {
        MenuLevel::Root => engine_res.cursor = MENU[cursor_idx].item,
        MenuLevel::LightsSubmenu => engine_res.submenu_cursor = cursor_idx,
    }
}

fn keyboard_event_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut engine_res: ResMut<EngineRes>,
    daemon_url: Res<DaemonUrl>,
) {
    let n = MENU.len();
    let mut cursor_idx = menu_index_of(engine_res.cursor);
    let mut any_press = false;

    if keys.just_pressed(KeyCode::ArrowUp) {
        cursor_idx = if cursor_idx == 0 {
            n - 1
        } else {
            cursor_idx - 1
        };
        any_press = true;
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        cursor_idx = (cursor_idx + 1) % n;
        any_press = true;
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        info!("Selected (keyboard): {:?}", MENU[cursor_idx].item);
        any_press = true;
    }
    if keys.just_pressed(KeyCode::Escape) {
        info!("Back (keyboard)");
        any_press = true;
    }
    if keys.just_pressed(KeyCode::KeyQ) {
        engine_res.manual_press = Some(Manual::ForceOff);
        info!("ALL OFF (keyboard Q)");
        any_press = true;
    }
    if keys.just_pressed(KeyCode::KeyX) {
        // Mirrors Xbox X button — global "toggle ALL lights" shortcut.
        // Same debounce + multi-dispatch path as the gamepad West arm.
        info!(
            "X (keyboard): toggle ALL lights ({:?})",
            X_ALL_TOGGLE_GROUPS
        );
        try_dispatch_lights_multi(&mut engine_res, &daemon_url, X_ALL_TOGGLE_GROUPS, "toggle");
        any_press = true;
    }

    if any_press {
        engine_res.fresh_controller_input = true;
        engine_res.since_input = Duration::ZERO;
        engine_res.dev_keyboard_active = true;
        engine_res.cursor = MENU[cursor_idx].item;
    }
}

fn engine_tick_system(
    time: Res<Time<Real>>,
    gamepads: Query<&Gamepad>,
    mut engine_res: ResMut<EngineRes>,
) {
    let fresh = engine_res.fresh_controller_input;
    let manual_press = engine_res.manual_press.take();
    engine_res.fresh_controller_input = false;

    // Latency diagnostic 2026-05-21: fires ONLY when there was fresh input
    // this tick. Pair with the DPad info! lines + menu_render_system info!
    // to compute the full input→render chain timing.
    if fresh {
        info!("engine_tick: fresh=true");
    }

    engine_res.since_input += time.delta();

    // Slice 3e — refresh HA-state from the shared snapshot. try_lock
    // never blocks Bevy; if the poller is mid-write we just skip this
    // frame and try next tick (snapshot is ~50 bytes, lock contention
    // is negligible at 30 fps).
    if let Some(snap_handle) = engine_res.ha_snapshot.clone() {
        if let Ok(snap) = snap_handle.try_lock() {
            // Treat stale snapshots (poller stuck > 30s) as "no signal".
            let fresh_enough = snap
                .refreshed_at
                .map(|t| t.elapsed() < Duration::from_secs(30))
                .unwrap_or(false);
            if fresh_enough {
                engine_res.media = snap.media;
                engine_res.ha_ribbon_title = snap.resumable_title.clone();
                engine_res.ha_occupancy = snap.occupancy_present;
            } else {
                // No fresh data — clear to safe defaults so a stale
                // playing-media state doesn't pin Content state forever.
                engine_res.media = Media::None;
                engine_res.ha_ribbon_title = None;
            }
        }
    }

    // Presence oracle: an actual Xbox controller (Bevy 0.18: queried
    // entities, not a Res<Gamepads>), OR the dev-host keyboard
    // fallback (sticky, set on first keypress). Production Shannon
    // never sees the keyboard path; Mac dev iteration relies on it
    // to demo without an Xbox controller paired to the Mac.
    // Slice 3e: HA occupancy is informational only — the controller-BT
    // oracle remains the canonical presence signal (per design hub §3).
    // ha_occupancy may eventually OR in for the disconnect-grace path.
    let controller_connected = !gamepads.is_empty() || engine_res.dev_keyboard_active;

    let now = current_local_minutes();

    let inputs = Inputs {
        now,
        controller_connected,
        since_controller_input: engine_res.since_input,
        fresh_controller_input: fresh,
        media: engine_res.media,
        outdoor_brightness: engine_res.outdoor_brightness,
        manual_press,
    };

    let outcome = engine_res.engine.step(&inputs);
    engine_res.state = outcome.state;

    // Slice 3f: route engine TV-power actions through the local
    // BlackoutTvPower port. The daemon (Slice 2) is the SMART-PLUG
    // actuator; Blackout is the Bevy-side RENDER toggle (paints black
    // instead of cutting HDMI, preserving Argon DA2 keepalive).
    for action in &outcome.actions {
        if let Action::SetTvPower(on) = action {
            engine_res.tv_power.set_power(*on);
        }
    }

    // Apply the predicted cursor ONLY on engine-state transition INTO
    // Kiosk (from Off/Ambient/Content). The Slice 3a hint design
    // (design hub § 13.7) is "stable frame, context-filled":
    // pre-position the cursor when the user RETURNS, not on every
    // non-fresh frame nor at arbitrary idle timeouts (both of which
    // make the cursor snap-back while user is just viewing the menu).
    //
    // The prior implementation (commits 10d8da8/2e00198) overwrote
    // the user's D-pad nav within ~16 ms (next frame after press),
    // creating a visible "snap-back to Lights/Watch" bug noticed
    // 2026-05-21 first-bedroom-test ("It resets to 'the start'
    // every few seconds?!").
    //
    // Tracking previous_state via a static is heavy; using a single
    // flag on EngineRes is the clean fix.
    let title_for_hint = engine_res.ha_ribbon_title.clone();
    let hint = engine_res
        .engine
        .hint_with_offer(&inputs, title_for_hint.as_deref());
    let just_entered_kiosk =
        matches!(outcome.state, DisplayState::Kiosk) && !engine_res.was_in_kiosk;
    if just_entered_kiosk {
        if let Some(predicted) = hint.cursor {
            engine_res.cursor = predicted;
        }
        // Reset submenu state on Kiosk re-entry — don't strand the user
        // in a stale Lights submenu after the screen had been off /
        // ambient. Pairing with the snap-back fix's discipline of
        // "context-fill on RETURN" (design § 13.7).
        engine_res.menu_level = MenuLevel::Root;
        engine_res.submenu_cursor = 0;
    }
    engine_res.was_in_kiosk = matches!(outcome.state, DisplayState::Kiosk);
    // Cache the computed ribbon for the render system. We store the
    // String rather than the `RibbonOffer` to keep the EngineRes free
    // of `crate::context` re-exports beyond what's already used.
    engine_res.ribbon_text = hint.ribbon.map(|r| r.text);
}

/// Spawn a std::thread that polls the shannon-kiosk-actions daemon's
/// /ha-state endpoint and writes the latest snapshot into the shared
/// `Arc<Mutex<HaSnapshot>>`. The Bevy engine_tick_system reads from
/// the same Mutex each frame (try_lock — never blocks Bevy).
///
/// Slice 3e (Bevy-side HA consumption). Pairs with Slice 3d's
/// /ha-state endpoint on the daemon.
///
/// Failure modes (all silently retried on next interval):
///   - daemon down (Mac dev iteration without daemon running): each
///     poll fails with connection-refused; snapshot stays at default.
///   - daemon transient error (HA outage): same — silent retry.
///   - reqwest::blocking creates its own tokio runtime in a background
///     thread; this is fine because Bevy doesn't share that runtime.
///
/// Returns the shared snapshot handle so EngineRes can hold a clone
/// for fast frame-rate reads.
fn spawn_ha_state_poller(daemon_url: String, interval: Duration) -> Arc<Mutex<HaSnapshot>> {
    let snapshot = Arc::new(Mutex::new(HaSnapshot::default()));
    let writer = snapshot.clone();
    std::thread::Builder::new()
        .name("ha-state-poller".into())
        .spawn(move || ha_state_poll_loop(daemon_url, interval, writer))
        .expect("spawn ha-state-poller thread");
    snapshot
}

fn ha_state_poll_loop(daemon_url: String, interval: Duration, writer: Arc<Mutex<HaSnapshot>>) {
    // 3s timeout — daemon is localhost so any slowness > 3s means it's
    // wedged. We don't want to block the poller for tens of seconds.
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let url = format!("{}/ha-state", daemon_url.trim_end_matches('/'));
    loop {
        std::thread::sleep(interval);
        let Ok(resp) = client.get(&url).send() else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(json) = resp.json::<serde_json::Value>() else {
            continue;
        };
        let snap = parse_ha_state(&json);
        if let Ok(mut guard) = writer.lock() {
            *guard = snap;
        }
    }
}

/// Pure JSON → HaSnapshot parser. The daemon's /ha-state endpoint
/// emits engine_media as a string ("none" | "music" | "video" | "game")
/// — easier to parse than reconstructing the MediaPlayerState struct
/// because Bevy doesn't need the full attribute set, just the engine
/// inputs.
fn parse_ha_state(v: &serde_json::Value) -> HaSnapshot {
    let media = match v.get("engine_media").and_then(|x| x.as_str()) {
        Some("music") => Media::Music,
        Some("video") => Media::Video,
        Some("game") => Media::Game,
        _ => Media::None,
    };
    let resumable_title = v
        .get("resumable_title")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    let occupancy_present = v
        .get("occupancy_present")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    HaSnapshot {
        media,
        resumable_title,
        occupancy_present,
        refreshed_at: Some(std::time::Instant::now()),
    }
}

fn current_local_minutes() -> ClockMinutes {
    use std::time::{SystemTime, UNIX_EPOCH};
    let utc_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    const SWEDEN_OFFSET_SECS: u64 = 2 * 60 * 60;
    let local_secs = utc_secs.saturating_add(SWEDEN_OFFSET_SECS);
    let minutes_of_day = ((local_secs / 60) % (24 * 60)) as u16;
    ClockMinutes::at(minutes_of_day / 60, minutes_of_day % 60)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn menu_render_system(
    engine_res: Res<EngineRes>,
    mut prev_selected: Local<Option<usize>>,
    mut root_label_q: Query<
        (&mut TextColor, &mut Visibility, &MenuLabel),
        (
            Without<MenuIcon>,
            Without<LightsSubmenuLabel>,
            Without<LightsSubmenuIcon>,
        ),
    >,
    mut root_icon_q: Query<
        (&mut TextColor, &mut Visibility, &MenuIcon),
        (
            Without<MenuLabel>,
            Without<LightsSubmenuLabel>,
            Without<LightsSubmenuIcon>,
        ),
    >,
    mut cursor_q: Query<
        (&mut Visibility, &MenuCursorMarker),
        (
            Without<MenuLabel>,
            Without<MenuIcon>,
            Without<LightsSubmenuLabel>,
            Without<LightsSubmenuIcon>,
        ),
    >,
    mut sub_label_q: Query<
        (&mut TextColor, &mut Visibility, &LightsSubmenuLabel),
        (
            Without<MenuLabel>,
            Without<MenuIcon>,
            Without<LightsSubmenuIcon>,
        ),
    >,
    mut sub_icon_q: Query<
        (&mut TextColor, &mut Visibility, &LightsSubmenuIcon),
        (
            Without<MenuLabel>,
            Without<MenuIcon>,
            Without<LightsSubmenuLabel>,
        ),
    >,
) {
    if !engine_res.is_changed() {
        return;
    }
    let in_submenu = matches!(engine_res.menu_level, MenuLevel::LightsSubmenu);
    let selected = if in_submenu {
        engine_res.submenu_cursor
    } else {
        menu_index_of(engine_res.cursor)
    };

    // Latency diagnostic 2026-05-21: log only when the rendered selection
    // index actually changes (engine_res is_changed fires every tick because
    // engine_tick_system takes ResMut — without this guard we'd flood at 30Hz).
    if Some(selected) != *prev_selected {
        info!(
            "menu_render: level={:?} cursor={:?} selected={} (was {:?})",
            engine_res.menu_level, engine_res.cursor, selected, *prev_selected
        );
        *prev_selected = Some(selected);
    }

    // Root menu: visible iff at Root level. Selection highlights cursor.
    for (mut color, mut vis, label) in root_label_q.iter_mut() {
        *vis = if in_submenu {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        color.0 = if label.index == selected && !in_submenu {
            OAT_MILK
        } else {
            OAT_DIM
        };
    }
    for (mut color, mut vis, icon) in root_icon_q.iter_mut() {
        *vis = if in_submenu {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        color.0 = if icon.index == selected && !in_submenu {
            OAT_MILK
        } else {
            OAT_DIM
        };
    }
    for (mut vis, _marker) in cursor_q.iter_mut() {
        // Cursor markers removed by design (Fredrik 2026-05-21) — no
        // entities exist with MenuCursorMarker, but the query stays for
        // backward compatibility / potential future re-introduction.
        *vis = Visibility::Hidden;
    }

    // Lights submenu: inverse — visible iff at LightsSubmenu level.
    for (mut color, mut vis, sub) in sub_label_q.iter_mut() {
        *vis = if in_submenu {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        color.0 = if sub.index == selected && in_submenu {
            OAT_MILK
        } else {
            OAT_DIM
        };
    }
    for (mut color, mut vis, sub) in sub_icon_q.iter_mut() {
        *vis = if in_submenu {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        color.0 = if sub.index == selected && in_submenu {
            OAT_MILK
        } else {
            OAT_DIM
        };
    }
}

fn preview_render_system(engine_res: Res<EngineRes>, mut q: Query<(&mut Text, &PreviewElement)>) {
    if !engine_res.is_changed() {
        return;
    }
    let (icon, label, subtitle) = preview_for(engine_res.cursor);
    for (mut text, element) in q.iter_mut() {
        let new_value = match element {
            PreviewElement::Icon => icon.to_string(),
            PreviewElement::Label => label.to_string(),
            PreviewElement::Subtitle => subtitle.to_string(),
        };
        if text.0 != new_value {
            text.0 = new_value;
        }
    }
}

/// Slice 3e: render the resume-offer ribbon. Visibility flips with the
/// engine's computed ribbon (cached on EngineRes.ribbon_text). Hidden
/// keeps the bottom strip empty — silent ribbon (design § 1).
fn ribbon_render_system(
    engine_res: Res<EngineRes>,
    mut q: Query<(&mut Text, &mut Visibility), With<RibbonLabel>>,
) {
    if !engine_res.is_changed() {
        return;
    }
    let Ok((mut text, mut vis)) = q.single_mut() else {
        return;
    };
    match &engine_res.ribbon_text {
        Some(t) => {
            // Bevy 0.18: Text is now a single-String wrapper. Format
            // the whole "[A]  {title}" string in one go (the merge
            // collapsed the old two-section colored chip to single
            // amber color — design tradeoff for the upgrade; can be
            // restored via TextSpan children in a polish slice).
            let new_value = format!("[A]  {t}");
            if text.0 != new_value {
                text.0 = new_value;
            }
            *vis = Visibility::Inherited;
        }
        None => {
            *vis = Visibility::Hidden;
        }
    }
}

/// Slice 3g: render the Ambient scene — a full-screen amber canvas
/// modulated by the engine's adaptive-dim brightness. Visible only
/// when DisplayState::Ambient(b); color = AMBER_ACCENT × b.
fn ambient_render_system(
    engine_res: Res<EngineRes>,
    mut q: Query<(&mut Visibility, &mut BackgroundColor), With<AmbientCanvas>>,
) {
    if !engine_res.is_changed() {
        return;
    }
    let Ok((mut vis, mut bg)) = q.single_mut() else {
        return;
    };
    match engine_res.state {
        DisplayState::Ambient(brightness) => {
            *vis = Visibility::Visible;
            let b = brightness.get();
            // Amber AMBER_ACCENT = (0.94, 0.71, 0.18). Scaling RGB by
            // brightness keeps the hue and dims toward black. Alpha
            // stays 1.0 (full occlusion — Ambient is its own scene,
            // not an overlay on the kiosk menu).
            *bg = Color::srgb(0.94 * b, 0.71 * b, 0.18 * b).into();
        }
        _ => {
            *vis = Visibility::Hidden;
        }
    }
}

/// Slice 3f: flip the full-screen blackout overlay per `tv_power`
/// state. The engine emits SetTvPower(_) on state transitions
/// (Off↔Kiosk/Content/Ambient); engine_tick_system applies it to
/// EngineRes.tv_power; this system flips the overlay visibility.
fn blackout_render_system(
    engine_res: Res<EngineRes>,
    mut q: Query<&mut Visibility, With<BlackoutOverlay>>,
) {
    if !engine_res.is_changed() {
        return;
    }
    let Ok(mut vis) = q.single_mut() else {
        return;
    };
    *vis = if engine_res.tv_power.is_on() {
        Visibility::Hidden
    } else {
        Visibility::Visible
    };
}

fn state_badge_system(engine_res: Res<EngineRes>, mut q: Query<&mut Text, With<StateBadge>>) {
    if !engine_res.is_changed() {
        return;
    }
    let label = match engine_res.state {
        DisplayState::Off => "OFF",
        DisplayState::Kiosk => "KIOSK",
        DisplayState::Content(_) => "CONTENT",
        DisplayState::Ambient(_) => "AMBIENT",
    };
    if let Ok(mut text) = q.single_mut() {
        if text.0 != label {
            text.0 = label.to_string();
        }
    }
}
