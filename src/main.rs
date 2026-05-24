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
//! - **Tiles**: Games / Music / Lights / Watch / Sensors / Buses / Sleep
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
use bevy::render::settings::{WgpuSettings, WgpuSettingsPriority};
use bevy::render::RenderPlugin;
use bevy::ui::IsDefaultUiCamera;
use bevy::window::{CursorOptions, WindowResolution};
use bevy::winit::WinitSettings;
use serde_json::Value;
use shannon_kiosk::context::{
    Action, BlackoutTvPower, ClockMinutes, Config, DisplayState, Engine, Inputs, Manual, Media,
    MenuItem, TvPower,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
// SIDEBAR_HEIGHT used to be 1080.0 (a fixed pixel value); the sidebar
// Node now uses Val::Vh(100.0) instead so it fills the viewport on any
// display. Const retained as a documentation anchor for the asset's
// natural height — useful if/when a future arc swaps the wood image.
#[allow(dead_code)]
const SIDEBAR_HEIGHT: f32 = 1080.0;
const SIDEBAR_OPACITY: f32 = 0.40;

// ─── Lucide codepoints for the menu tiles ────────────────────────────
const ICON_GAMES: char = '\u{e0df}'; // gamepad-2
const ICON_MUSIC: char = '\u{e122}'; // music
const ICON_LIGHTS: char = '\u{e1c2}'; // lightbulb
const ICON_WATCH: char = '\u{e481}'; // play-square
const ICON_SENSORS: char = '\u{e038}'; // activity
const ICON_BUSES: char = '\u{e334}'; // bus-front
const ICON_SLEEP: char = '\u{e11e}'; // moon
const ICON_PLAY_PAUSE: char = '\u{e12e}'; // pause
const ICON_PREVIOUS: char = '\u{e15f}'; // skip-back
const ICON_NEXT: char = '\u{e160}'; // skip-forward

// ─── Menu definition ─────────────────────────────────────────────────
struct TileSpec {
    item: MenuItem,
    label: &'static str,
    icon: char,
    enabled: bool,
}

const MENU: &[TileSpec] = &[
    TileSpec {
        item: MenuItem::Games,
        label: "GAMES",
        icon: ICON_GAMES,
        enabled: false,
    },
    TileSpec {
        item: MenuItem::Music,
        label: "MUSIC",
        icon: ICON_MUSIC,
        enabled: true,
    },
    TileSpec {
        item: MenuItem::Lights,
        label: "LIGHTS",
        icon: ICON_LIGHTS,
        enabled: true,
    },
    TileSpec {
        item: MenuItem::Watch,
        label: "WATCH",
        icon: ICON_WATCH,
        enabled: true,
    },
    TileSpec {
        item: MenuItem::Sensors,
        label: "SENSORS",
        icon: ICON_SENSORS,
        enabled: true,
    },
    TileSpec {
        item: MenuItem::Buses,
        label: "BUSES",
        icon: ICON_BUSES,
        enabled: true,
    },
    TileSpec {
        item: MenuItem::Sleep,
        label: "SLEEP",
        icon: ICON_SLEEP,
        enabled: true,
    },
];

fn menu_index_of(item: MenuItem) -> usize {
    MENU.iter()
        .position(|t| t.item == item)
        .expect("MENU must contain every MenuItem variant")
}

fn menu_color(index: usize, selected: bool) -> Color {
    let enabled = MENU.get(index).map(|tile| tile.enabled).unwrap_or_default();
    match (enabled, selected) {
        (true, true) => OAT_MILK,
        (true, false) => OAT_DIM,
        (false, true) => OAT_DIM,
        (false, false) => OAT_FAINT,
    }
}

// ─── Lights submenu (Fredrik 2026-05-21: A on LIGHTS opens a group
// picker; A on a group toggles it; B returns to root). ──────────────
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
enum MenuLevel {
    #[default]
    Root,
    LightsSubmenu,
    MusicSubmenu,
}

struct SubmenuTile {
    /// Kiosk-side action key. For the lights submenu this is a daemon
    /// `/lights/:group/:action` group; for Music this is a daemon
    /// `/media/:entity/:action` action.
    key: &'static str,
    label: &'static str,
    icon: char,
}

const LIGHTS_SUBMENU: &[SubmenuTile] = &[
    SubmenuTile {
        key: "bedroom",
        label: "BEDROOM",
        icon: ICON_LIGHTS,
    },
    SubmenuTile {
        key: "office",
        label: "OFFICE",
        icon: ICON_LIGHTS,
    },
    SubmenuTile {
        key: "hallway",
        label: "HALLWAY",
        icon: ICON_LIGHTS,
    },
];

const MUSIC_SUBMENU: &[SubmenuTile] = &[
    SubmenuTile {
        key: "play_pause",
        label: "PLAY / PAUSE",
        icon: ICON_PLAY_PAUSE,
    },
    SubmenuTile {
        key: "prev",
        label: "PREVIOUS",
        icon: ICON_PREVIOUS,
    },
    SubmenuTile {
        key: "next",
        label: "NEXT",
        icon: ICON_NEXT,
    },
];

fn submenu_for(level: MenuLevel) -> &'static [SubmenuTile] {
    match level {
        MenuLevel::Root => &[],
        MenuLevel::LightsSubmenu => LIGHTS_SUBMENU,
        MenuLevel::MusicSubmenu => MUSIC_SUBMENU,
    }
}

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
    /// Current menu level — Root shows the top-level menu; tile
    /// submenus expose focused controls. B at a submenu returns to Root
    /// with the owning tile restored. Exit-Kiosk resets to Root.
    menu_level: MenuLevel,
    /// Cursor index within the current submenu; unused at Root.
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

#[derive(Clone, Debug, Default)]
struct SarpetorpSnapshot {
    sensors: SensorSnapshot,
    buses: BusSnapshot,
    error: Option<String>,
    refreshed_at: Option<Instant>,
}

#[derive(Clone, Debug, Default)]
struct SensorSnapshot {
    indoor_temp: Option<f64>,
    indoor_humidity: Option<f64>,
    indoor_stale: bool,
    outdoor_temp: Option<f64>,
    tank_top: Option<f64>,
    tank_bottom: Option<f64>,
    pipe_outflow: Option<f64>,
    advisory: Option<String>,
    evening_temp: Option<f64>,
    tonight_low: Option<f64>,
    sun_hours: Option<String>,
    spark_indoor: String,
    spark_outdoor: String,
    spark_tank: String,
    spark_solar: String,
}

#[derive(Clone, Debug, Default)]
struct BusSnapshot {
    northbound: Vec<BusDeparture>,
    southbound: Vec<BusDeparture>,
    error: Option<String>,
}

#[derive(Clone, Debug)]
struct BusDeparture {
    departure_time: String,
    departure_timestamp: i64,
    minutes_until: i64,
    line_number: String,
    destination: String,
    direction_flag: String,
    delayed_minutes: i64,
    stop_short: String,
    stop_distance: String,
}

#[derive(Resource, Clone)]
struct SarpetorpSnapshotRes(Arc<Mutex<SarpetorpSnapshot>>);

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

/// Marker for submenu icon entities (spawned alongside the root menu
/// icons at the same y positions; text/visibility are filled from the
/// active submenu definition).
#[derive(Component)]
struct LightsSubmenuIcon {
    index: usize,
}

/// Marker for submenu label entities. Paired with LightsSubmenuIcon.
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

#[derive(Component)]
struct DashboardPreviewLine {
    index: usize,
}

fn preview_for(item: MenuItem) -> (char, &'static str, &'static str) {
    match item {
        MenuItem::Games => (ICON_GAMES, "GAMES", "Recently played"),
        MenuItem::Music => (ICON_MUSIC, "MUSIC", "Now playing"),
        MenuItem::Lights => (ICON_LIGHTS, "LIGHTS", "Bedroom · Office · Hallway"),
        MenuItem::Watch => (ICON_WATCH, "WATCH", "Continue watching"),
        MenuItem::Sensors => (ICON_SENSORS, "SENSORS", "Inomhus · Väder · Tank"),
        MenuItem::Buses => (ICON_BUSES, "BUSES", "Ottekil · Björkvik"),
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
        .insert_resource({
            let base_url = std::env::var("SARPETORP_DASHBOARD_URL")
                .unwrap_or_else(|_| "http://sarpetorp.home".to_string());
            let interval_secs = std::env::var("SARPETORP_POLL_INTERVAL_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(60);
            SarpetorpSnapshotRes(spawn_sarpetorp_poller(
                base_url,
                Duration::from_secs(interval_secs),
            ))
        })
        .add_plugins(
            DefaultPlugins
                .set(RenderPlugin {
                    render_creation: WgpuSettings {
                        priority: WgpuSettingsPriority::Functionality,
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
                        // Cage + rootless Xwayland on Shannon can create
                        // Bevy borderless fullscreen windows as 0x0/black.
                        // The bedroom TV path is fixed 1080p, so keep the
                        // known-good explicit size and make the window chrome-free.
                        resolution: WindowResolution::new(1920, 1080),
                        resizable: true,
                        decorations: false,
                        ..default()
                    }),
                    primary_cursor_options: Some(CursorOptions {
                        visible: false,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_systems(
            Startup,
            (setup_camera, load_fonts, setup_background, setup_ui).chain(),
        )
        .add_systems(
            Update,
            (
                gamepad_event_system,
                keyboard_event_system,
                engine_tick_system,
                menu_render_system,
                preview_render_system,
                dashboard_preview_render_system,
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

fn setup_camera(mut commands: Commands) {
    commands.spawn((Camera2d, IsDefaultUiCamera));
}

fn setup_ui(mut commands: Commands, fonts: Res<FontHandles>, sidebar_bg: Res<SidebarBgHandle>) {
    // ─── Sidebar wood-panel background ────────────────────────────────
    // Image asset is 540×1080 natural pixels. The sidebar Node height
    // uses Val::Vh(100.0) (= 100% viewport height) so it fills the
    // window regardless of display: 1080 on the bedroom TV, 1440 on
    // the dev ultrawide. NodeImageMode::Stretch scales the asset to
    // the Node bounds so the wood texture covers the whole sidebar
    // height. Minor Mali Panfrost stretch overshoot (~10 px noted in
    // an earlier 2026-05-21 session) is acceptable — the alternative
    // (Auto + natural 540×1080 image) left a black band below y=1080
    // on the ultrawide that Fredrik flagged as "wood panel doesn't
    // cover the whole sidebar" (2026-05-21 evening screenshot pass).
    // Width stays Val::Px(540) — sidebar is a designed UI element with
    // a fixed pixel chrome width regardless of viewport. Negative
    // GlobalZIndex pins behind menu text / chrome / preview UI.
    commands.spawn((
        ImageNode {
            image: sidebar_bg.0.clone(),
            color: Color::srgba(1.0, 1.0, 1.0, SIDEBAR_OPACITY),
            image_mode: bevy::ui::widget::NodeImageMode::Stretch,
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(0.0),
            left: Val::Px(0.0),
            width: Val::Px(SIDEBAR_WIDTH),
            height: Val::Vh(100.0),
            ..default()
        },
        GlobalZIndex(-10),
    ));

    // Top-level vertical menu — left side, generous vertical spacing.
    // Per Fredrik 2026-05-21: no cursor marker — the OAT_MILK vs OAT_DIM
    // text-color contrast is sufficient to indicate selection. The
    // MenuCursorMarker type stays around (menu_render_system still
    // queries it; the empty iter just no-ops).
    for (i, tile) in MENU.iter().enumerate() {
        let y = 240.0 + (i as f32 * 88.0);
        let label_color = menu_color(i, i == 0);

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

    // ─── Tile submenus at the first menu y positions. Hidden by default;
    // menu_render_system swaps labels/icons from the active submenu.
    let submenu_slots = LIGHTS_SUBMENU.len().max(MUSIC_SUBMENU.len());
    for i in 0..submenu_slots {
        let sub = LIGHTS_SUBMENU.get(i).unwrap_or(&LIGHTS_SUBMENU[0]);
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

    // Native Sarpetorp mirrors for Sensors and Buses. These stay hidden
    // unless one of those tiles is selected; the poller below reads the
    // same `/data/...` JSON endpoints as the React dashboard, but Bevy
    // renders compact text rows locally so Shannon never pays a browser
    // UI tax for these pages.
    for i in 0..10 {
        commands.spawn((
            Text::new(""),
            TextFont {
                font: fonts.semibold.clone(),
                font_size: if i <= 1 { 32.0 } else { 24.0 },
                ..default()
            },
            TextColor(if i <= 1 { OAT_MILK } else { OAT_DIM }),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(520.0 + (i as f32 * 42.0)),
                left: Val::Px(620.0),
                ..default()
            },
            Visibility::Hidden,
            DashboardPreviewLine { index: i },
        ));
    }

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
        level => (engine_res.submenu_cursor, submenu_for(level).len()),
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
                        MenuItem::Music => {
                            engine_res.menu_level = MenuLevel::MusicSubmenu;
                            engine_res.submenu_cursor = 0;
                            cursor_idx = 0;
                            info!("Enter MusicSubmenu (cursor=0=play_pause)");
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
                        // Other tiles (Games, Buses, Sensors): South-arm
                        // wiring pending per the kiosk plan's tile-
                        // action roadmap. Games = RetroArch launch
                        // (needs cage process-model research, mode-
                        // script + daemon /mode/{m} route; design hub
                        // §13.29); Sensors/Buses = native Sarpetorp
                        // dashboard mirrors (render-system work).
                        _ => {}
                    }
                }
                MenuLevel::LightsSubmenu => {
                    // A on a group toggles its lights via the daemon.
                    // Debounced — see try_dispatch_lights for the why.
                    let group = LIGHTS_SUBMENU[cursor_idx].key;
                    info!("Toggle lights group: {}", group);
                    try_dispatch_lights(&mut engine_res, &daemon_url, group, "toggle");
                }
                MenuLevel::MusicSubmenu => {
                    let action = MUSIC_SUBMENU[cursor_idx].key;
                    info!("Music: {} ({})", action, MUSIC_DEFAULT_ENTITY);
                    try_dispatch_media(&mut engine_res, &daemon_url, MUSIC_DEFAULT_ENTITY, action);
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
                MenuLevel::MusicSubmenu => {
                    info!("Exit MusicSubmenu → Root (cursor restored to Music)");
                    engine_res.menu_level = MenuLevel::Root;
                    engine_res.cursor = MenuItem::Music;
                    cursor_idx = menu_index_of(MenuItem::Music);
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
        MenuLevel::LightsSubmenu | MenuLevel::MusicSubmenu => {
            engine_res.submenu_cursor = cursor_idx
        }
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
    // Kiosk (from Off/Ambient/Content), and only when that transition
    // was not caused by direct input this same tick. The Slice 3a hint
    // design (design hub § 13.7) is "stable frame, context-filled":
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
        // A controller press from Ambient/Off both wakes the Kiosk state
        // and mutates UI state before this system runs. Re-applying the
        // prediction here makes D-pad navigation visibly jump back to
        // Watch/Lights; resetting the submenu here can also undo an
        // A-on-Lights open. Preserve direct input; keep prediction/reset
        // for passive/context returns.
        if !fresh {
            if let Some(predicted) = hint.cursor {
                engine_res.cursor = predicted;
            }
            // Reset submenu state on Kiosk re-entry — don't strand the user
            // in a stale Lights submenu after the screen had been off /
            // ambient. Pairing with the snap-back fix's discipline of
            // "context-fill on RETURN" (design § 13.7).
            engine_res.menu_level = MenuLevel::Root;
            engine_res.submenu_cursor = 0;
        } else {
            info!("Kiosk entry prediction/reset skipped: fresh input already changed UI");
        }
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

fn spawn_sarpetorp_poller(base_url: String, interval: Duration) -> Arc<Mutex<SarpetorpSnapshot>> {
    let snapshot = Arc::new(Mutex::new(SarpetorpSnapshot::default()));
    let writer = snapshot.clone();
    std::thread::Builder::new()
        .name("sarpetorp-poller".into())
        .spawn(move || sarpetorp_poll_loop(base_url, interval, writer))
        .expect("spawn sarpetorp-poller thread");
    snapshot
}

fn sarpetorp_poll_loop(
    base_url: String,
    interval: Duration,
    writer: Arc<Mutex<SarpetorpSnapshot>>,
) {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(6))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    loop {
        let snapshot = fetch_sarpetorp_snapshot(&client, &base_url);
        if let Ok(mut guard) = writer.lock() {
            *guard = snapshot;
        }
        std::thread::sleep(interval);
    }
}

fn fetch_sarpetorp_snapshot(
    client: &reqwest::blocking::Client,
    base_url: &str,
) -> SarpetorpSnapshot {
    let mut snapshot = SarpetorpSnapshot {
        refreshed_at: Some(Instant::now()),
        ..Default::default()
    };

    let fetch = |path: &str| -> Result<Value, String> {
        let url = format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let resp = client.get(&url).send().map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("{} {}", path, resp.status()));
        }
        resp.json::<Value>().map_err(|e| e.to_string())
    };

    let house = fetch("/data/house_temperature");
    let weather = fetch("/data/weather");
    let sun = fetch("/data/sun_windows");
    let wood = fetch("/data/wood_stove");
    let indoor_history = fetch("/data/house_temperature_history");
    let tank_history = fetch("/data/wood_stove_history");
    let outdoor_history = fetch("/data/outdoor_temperature_history");
    let solar_history = fetch("/data/solar_history");
    let buses = fetch("/data/bus_departures");

    if let Ok(bus_json) = buses {
        snapshot.buses = parse_bus_snapshot(&bus_json);
    } else if let Err(e) = buses {
        snapshot.buses.error = Some(e);
    }

    match (
        house,
        weather,
        sun,
        wood,
        indoor_history,
        tank_history,
        outdoor_history,
        solar_history,
    ) {
        (
            Ok(house),
            Ok(weather),
            Ok(sun),
            Ok(wood),
            Ok(indoor_history),
            Ok(tank_history),
            Ok(outdoor_history),
            Ok(solar_history),
        ) => {
            snapshot.sensors = parse_sensor_snapshot(
                &house,
                &weather,
                &sun,
                &wood,
                &indoor_history,
                &tank_history,
                &outdoor_history,
                &solar_history,
            );
        }
        _ => {
            snapshot.error = Some("Sarpetorp data unavailable".to_string());
        }
    }

    snapshot
}

#[allow(clippy::too_many_arguments)]
fn parse_sensor_snapshot(
    house: &Value,
    weather: &Value,
    sun: &Value,
    wood: &Value,
    indoor_history: &Value,
    tank_history: &Value,
    outdoor_history: &Value,
    solar_history: &Value,
) -> SensorSnapshot {
    let indoor_values = value_series(indoor_history, "temp_c");
    let smoothed_indoor = median_recent(&indoor_values, 12);
    let indoor_temp = smoothed_indoor.or_else(|| number_at(house, &["temp_c"]));
    let outdoor_temp = number_at(weather, &["current", "temp_c"]);
    let tank_top = number_at(wood, &["tank_top_temp"]);
    let tank_bottom = number_at(wood, &["tank_bottom_temp"]);
    let pipe_outflow = number_at(wood, &["pipe_outflow_temp"]);
    let tonight_low = number_at(
        weather,
        &["forecast", "forecastday", "0", "day", "mintemp_c"],
    );

    let (advisory, evening_temp, sun_hours) = compute_evening_advisory(
        indoor_temp,
        outdoor_temp,
        tonight_low,
        weather,
        sun,
        tank_top,
        tank_bottom,
    );

    let tank_values = tank_history
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| {
                    let top = number_at(v, &["tt"]);
                    let bottom = number_at(v, &["tb"]);
                    match (top, bottom) {
                        (Some(t), Some(b)) => Some((t + b) / 2.0),
                        (Some(t), None) => Some(t),
                        (None, Some(b)) => Some(b),
                        _ => None,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let outdoor_values = value_series(outdoor_history, "temp_c");
    let solar_values = value_series(solar_history, "ghi");

    SensorSnapshot {
        indoor_temp,
        indoor_humidity: number_at(house, &["humidity_pct"]),
        indoor_stale: house
            .get("is_stale")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        outdoor_temp,
        tank_top,
        tank_bottom,
        pipe_outflow,
        advisory,
        evening_temp,
        tonight_low,
        sun_hours,
        spark_indoor: sparkline(&indoor_values),
        spark_outdoor: sparkline(&outdoor_values),
        spark_tank: sparkline(&tank_values),
        spark_solar: sparkline(&solar_values),
    }
}

fn compute_evening_advisory(
    indoor_temp: Option<f64>,
    outdoor_temp: Option<f64>,
    tonight_low: Option<f64>,
    weather: &Value,
    sun: &Value,
    tank_top: Option<f64>,
    tank_bottom: Option<f64>,
) -> (Option<String>, Option<f64>, Option<String>) {
    let sun_hours = sun
        .get("daily_sun_hours")
        .and_then(Value::as_array)
        .and_then(|arr| {
            let today = local_date_string();
            arr.iter()
                .find(|v| v.get("date").and_then(Value::as_str) == Some(today.as_str()))
                .or_else(|| arr.first())
        })
        .and_then(|v| v.get("sun_hours_text"))
        .and_then(Value::as_str)
        .map(String::from);

    let Some(indoor) = indoor_temp else {
        return (None, None, sun_hours);
    };
    let Some(outdoor_now) = outdoor_temp else {
        return (None, None, sun_hours);
    };
    let Some(low) = tonight_low else {
        return (None, None, sun_hours);
    };

    let hourly_outdoor = hourly_outdoor(weather);
    let hourly_brightness = hourly_brightness(sun);
    let current_hour = current_local_minutes().raw() as f64 / 60.0;
    let target_hour = 21.0;

    let predicted = if hourly_outdoor.is_empty() {
        let day_avg = (outdoor_now + low) / 2.0;
        let hours_until_evening = (target_hour - current_hour).max(0.0);
        let day_drift = (day_avg - indoor) * 0.04 * hours_until_evening;
        let solar_gain = hourly_brightness
            .iter()
            .filter(|(h, b)| (*h as f64) >= current_hour.floor() && *b >= 40.0)
            .count() as f64
            * 0.20;
        indoor + day_drift + solar_gain
    } else {
        predict_evening_indoor(
            indoor,
            &hourly_outdoor,
            &hourly_brightness,
            current_hour,
            target_hour,
            tank_top,
            tank_bottom,
        )
    };

    let label = if predicted >= 18.0 {
        "Eld behövs ej"
    } else if predicted >= 16.0 {
        "Elda kanske?"
    } else {
        "Elda gärna"
    };
    (Some(label.to_string()), Some(predicted), sun_hours)
}

fn predict_evening_indoor(
    current_indoor: f64,
    hourly_outdoor: &[(u8, f64)],
    hourly_brightness: &[(u8, f64)],
    current_hour: f64,
    target_hour: f64,
    tank_top: Option<f64>,
    tank_bottom: Option<f64>,
) -> f64 {
    const HEAT_LOSS_COEF: f64 = 0.02;
    const SOLAR_GAIN_COEF: f64 = 0.003;
    const INTERNAL_GAIN_COEF: f64 = 0.02;
    const TANK_HEAT_COEF: f64 = 0.010;

    let mut temp = current_indoor;
    let mut hour = current_hour;
    while hour < target_hour - 1e-9 {
        let next_hour = target_hour.min(hour.floor() + 1.0);
        let dt = next_hour - hour;
        let outdoor = hourly_outdoor
            .iter()
            .find(|(h, _)| *h == hour.floor() as u8)
            .map(|(_, t)| *t)
            .unwrap_or(temp);
        let brightness = hourly_brightness
            .iter()
            .find(|(h, _)| *h == hour.floor() as u8)
            .map(|(_, b)| *b)
            .unwrap_or(0.0);
        let tank_heat = tank_heat_rate(tank_top, tank_bottom, temp, TANK_HEAT_COEF);
        temp += -(HEAT_LOSS_COEF * (temp - outdoor) * dt)
            + (SOLAR_GAIN_COEF * brightness * dt)
            + (INTERNAL_GAIN_COEF * dt)
            + (tank_heat * dt);
        hour = next_hour;
    }
    temp
}

fn tank_heat_rate(tank_top: Option<f64>, tank_bottom: Option<f64>, room: f64, coef: f64) -> f64 {
    let values = [tank_top, tank_bottom]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if values.is_empty() {
        return 0.0;
    }
    let avg = values.iter().sum::<f64>() / values.len() as f64;
    if avg <= room {
        0.0
    } else {
        coef * (avg - room)
    }
}

fn hourly_outdoor(weather: &Value) -> Vec<(u8, f64)> {
    weather
        .pointer("/forecast/forecastday/0/hour")
        .and_then(Value::as_array)
        .map(|hours| {
            hours
                .iter()
                .filter_map(|h| {
                    let temp = h.get("temp_c").and_then(Value::as_f64)?;
                    let hour = h
                        .get("time")
                        .and_then(Value::as_str)
                        .and_then(|s| s.split(' ').nth(1))
                        .and_then(|s| s.split(':').next())
                        .and_then(|s| s.parse::<u8>().ok())?;
                    Some((hour, temp))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn hourly_brightness(sun: &Value) -> Vec<(u8, f64)> {
    let curve = sun
        .get("todays_brightness_curve")
        .or_else(|| sun.pointer("/locations/0/todays_brightness_curve"));
    curve
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|p| {
                    let hour = p
                        .get("hour")
                        .and_then(Value::as_str)
                        .and_then(|s| s.split(':').next())
                        .and_then(|s| s.parse::<u8>().ok())?;
                    let brightness = p
                        .get("brightness_percent")
                        .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse::<f64>().ok()))?;
                    Some((hour, brightness.clamp(0.0, 100.0)))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_bus_snapshot(v: &Value) -> BusSnapshot {
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return BusSnapshot {
            error: Some(err.to_string()),
            ..Default::default()
        };
    }
    let now_sec = unix_now_secs();
    let mut departures = v
        .get("departures")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(parse_bus_departure)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for bus in &mut departures {
        bus.minutes_until = ((bus.departure_timestamp - now_sec) / 60).max(-1);
    }
    let mut feasible = merge_connections(deduplicate_buses(
        departures
            .into_iter()
            .filter(|b| b.minutes_until >= 0 && is_useful_destination(b))
            .collect(),
    ));
    feasible.sort_by_key(|b| b.departure_timestamp);
    BusSnapshot {
        northbound: limit_next_day(
            feasible
                .iter()
                .filter(|b| is_northbound(b))
                .cloned()
                .collect(),
            now_sec,
        ),
        southbound: limit_next_day(
            feasible.into_iter().filter(|b| !is_northbound(b)).collect(),
            now_sec,
        ),
        error: None,
    }
}

fn parse_bus_departure(v: &Value) -> Option<BusDeparture> {
    Some(BusDeparture {
        departure_time: v.get("departure_time")?.as_str()?.to_string(),
        departure_timestamp: v.get("departure_timestamp")?.as_i64()?,
        minutes_until: v.get("minutes_until").and_then(Value::as_i64).unwrap_or(0),
        line_number: v.get("line_number")?.as_str()?.to_string(),
        destination: v.get("destination")?.as_str()?.to_string(),
        direction_flag: v
            .get("direction_flag")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        delayed_minutes: v
            .get("delayed_minutes")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        stop_short: v.get("stop_short")?.as_str()?.to_string(),
        stop_distance: v.get("stop_distance")?.as_str()?.to_string(),
    })
}

fn is_northbound(bus: &BusDeparture) -> bool {
    if !bus.direction_flag.is_empty() {
        return bus.direction_flag == "1";
    }
    let lower = bus.destination.to_lowercase();
    lower.contains("katrineholm") || (lower.contains("björkvik") && !lower.contains("gustavsborg"))
}

fn is_useful_destination(bus: &BusDeparture) -> bool {
    is_northbound(bus) || bus.destination.to_lowercase().contains("nyköping")
}

fn deduplicate_buses(buses: Vec<BusDeparture>) -> Vec<BusDeparture> {
    let mut kept = Vec::new();
    let mut used = vec![false; buses.len()];
    for i in 0..buses.len() {
        if used[i] {
            continue;
        }
        let mut best = buses[i].clone();
        for j in (i + 1)..buses.len() {
            if used[j] {
                continue;
            }
            let other = &buses[j];
            if best.line_number == other.line_number
                && is_northbound(&best) == is_northbound(other)
                && (best.departure_timestamp - other.departure_timestamp).abs() <= 15 * 60
                && best.stop_short != other.stop_short
            {
                if other.stop_short == "O" {
                    best = other.clone();
                }
                used[j] = true;
            }
        }
        kept.push(best);
    }
    kept
}

fn merge_connections(buses: Vec<BusDeparture>) -> Vec<BusDeparture> {
    let feeders = buses
        .iter()
        .filter(|b| b.line_number == "590" && is_northbound(b) && b.stop_short == "O")
        .cloned()
        .collect::<Vec<_>>();
    let katrineholm = buses
        .iter()
        .filter(|b| b.line_number == "490" && is_northbound(b) && b.stop_short == "B")
        .cloned()
        .collect::<Vec<_>>();
    let mut used_feed = vec![false; feeders.len()];
    let mut used_kat = vec![false; katrineholm.len()];
    let mut merged = Vec::new();

    for (k_idx, k_bus) in katrineholm.iter().enumerate() {
        for (f_idx, feeder) in feeders.iter().enumerate() {
            if used_feed[f_idx] {
                continue;
            }
            let gap = k_bus.departure_timestamp - feeder.departure_timestamp;
            if (5 * 60..=25 * 60).contains(&gap) {
                let mut bus = k_bus.clone();
                bus.departure_time = feeder.departure_time.clone();
                bus.departure_timestamp = feeder.departure_timestamp;
                bus.minutes_until = feeder.minutes_until;
                bus.stop_short = "O".to_string();
                bus.stop_distance = "600m".to_string();
                merged.push(bus);
                used_feed[f_idx] = true;
                used_kat[k_idx] = true;
                break;
            }
        }
    }

    let mut result = Vec::new();
    for bus in buses {
        let is_used_feeder = feeders
            .iter()
            .enumerate()
            .any(|(i, f)| used_feed[i] && same_bus_identity(f, &bus));
        let is_used_kat = katrineholm
            .iter()
            .enumerate()
            .any(|(i, k)| used_kat[i] && same_bus_identity(k, &bus));
        if !is_used_feeder && !is_used_kat {
            result.push(bus);
        }
    }
    result.extend(merged);
    result.sort_by_key(|b| b.departure_timestamp);
    result
}

fn same_bus_identity(a: &BusDeparture, b: &BusDeparture) -> bool {
    a.departure_timestamp == b.departure_timestamp
        && a.line_number == b.line_number
        && a.destination == b.destination
        && a.stop_short == b.stop_short
}

fn limit_next_day(buses: Vec<BusDeparture>, now_sec: i64) -> Vec<BusDeparture> {
    const MAX_NEXT_DAY: usize = 3;
    let today_day = now_sec / 86_400;
    let mut today = Vec::new();
    let mut tomorrow = Vec::new();
    for bus in buses {
        if bus.departure_timestamp / 86_400 == today_day {
            today.push(bus);
        } else {
            tomorrow.push(bus);
        }
    }
    tomorrow.truncate(MAX_NEXT_DAY);
    today.extend(tomorrow);
    today
}

fn number_at(v: &Value, path: &[&str]) -> Option<f64> {
    let mut cur = v;
    for part in path {
        cur = if let Ok(idx) = part.parse::<usize>() {
            cur.as_array()?.get(idx)?
        } else {
            cur.get(*part)?
        };
    }
    cur.as_f64().or_else(|| cur.as_i64().map(|n| n as f64))
}

fn value_series(v: &Value, key: &str) -> Vec<f64> {
    v.as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| number_at(item, &[key]))
                .collect()
        })
        .unwrap_or_default()
}

fn median_recent(values: &[f64], count: usize) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut recent = values.iter().rev().take(count).copied().collect::<Vec<_>>();
    if recent.is_empty() {
        return None;
    }
    recent.sort_by(|a, b| a.total_cmp(b));
    Some(recent[recent.len() / 2])
}

fn sparkline(values: &[f64]) -> String {
    // ASCII fallback (was Unicode Block Elements U+2581-2588 — those rendered
    // as TOFU/empty rectangles on Shannon's bedroom TV because SharpSans
    // lacks Block Elements glyphs and Bevy 0.18 doesn't font-fallback for
    // missing codepoints; live evidence in Fredrik's 2026-05-23 Sensors-page
    // photo). The ASCII ramp gives ~the same visual density progression and
    // renders in every font. Tradeoff is slightly less elegant glyphs;
    // proper fix would be either bundling a Unicode-complete fallback font
    // and threading per-codepoint fallback through TextSpan, or rendering
    // sparklines as actual Bevy mesh bars — both deferred.
    const STEPS: [char; 8] = ['_', '.', ',', '-', '=', '+', '*', '#'];
    let points = values.iter().rev().take(36).copied().collect::<Vec<_>>();
    if points.len() < 2 {
        return "no trace".to_string();
    }
    let mut points = points.into_iter().rev().collect::<Vec<_>>();
    let min = points.iter().copied().fold(f64::INFINITY, f64::min);
    let max = points.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = (max - min).max(0.001);
    points
        .drain(..)
        .map(|v| {
            let idx = (((v - min) / range) * (STEPS.len() as f64 - 1.0)).round() as usize;
            STEPS[idx.min(STEPS.len() - 1)]
        })
        .collect()
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn local_date_string() -> String {
    let days = (unix_now_secs() + 2 * 60 * 60) / 86_400;
    civil_from_days(days)
}

fn civil_from_days(days_since_epoch: i64) -> String {
    // Howard Hinnant's civil-from-days conversion, adapted for UTC+2
    // local-day labels. Good for dashboard date matching without pulling
    // chrono into the kiosk binary.
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    format!("{year:04}-{m:02}-{d:02}")
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
        (
            &mut Text,
            &mut TextColor,
            &mut Visibility,
            &LightsSubmenuLabel,
        ),
        (
            Without<MenuLabel>,
            Without<MenuIcon>,
            Without<LightsSubmenuIcon>,
        ),
    >,
    mut sub_icon_q: Query<
        (
            &mut Text,
            &mut TextColor,
            &mut Visibility,
            &LightsSubmenuIcon,
        ),
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
    let in_submenu = !matches!(engine_res.menu_level, MenuLevel::Root);
    let submenu = submenu_for(engine_res.menu_level);
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
        color.0 = menu_color(label.index, label.index == selected && !in_submenu);
    }
    for (mut color, mut vis, icon) in root_icon_q.iter_mut() {
        *vis = if in_submenu {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        color.0 = menu_color(icon.index, icon.index == selected && !in_submenu);
    }
    for (mut vis, _marker) in cursor_q.iter_mut() {
        // Cursor markers removed by design (Fredrik 2026-05-21) — no
        // entities exist with MenuCursorMarker, but the query stays for
        // backward compatibility / potential future re-introduction.
        *vis = Visibility::Hidden;
    }

    // Tile submenus: inverse of the root menu.
    for (mut text, mut color, mut vis, sub) in sub_label_q.iter_mut() {
        let tile = submenu.get(sub.index);
        if let Some(tile) = tile {
            if text.0 != tile.label {
                text.0 = tile.label.to_string();
            }
        }
        *vis = if in_submenu && tile.is_some() {
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
    for (mut text, mut color, mut vis, sub) in sub_icon_q.iter_mut() {
        let tile = submenu.get(sub.index);
        if let Some(tile) = tile {
            let icon = tile.icon.to_string();
            if text.0 != icon {
                text.0 = icon;
            }
        }
        *vis = if in_submenu && tile.is_some() {
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

fn dashboard_preview_render_system(
    engine_res: Res<EngineRes>,
    sarpetorp: Res<SarpetorpSnapshotRes>,
    mut q: Query<(
        &mut Text,
        &mut TextColor,
        &mut Visibility,
        &DashboardPreviewLine,
    )>,
) {
    let show_dashboard = matches!(engine_res.cursor, MenuItem::Sensors | MenuItem::Buses)
        && matches!(engine_res.menu_level, MenuLevel::Root);
    if !show_dashboard {
        for (_, _, mut vis, _) in q.iter_mut() {
            *vis = Visibility::Hidden;
        }
        return;
    }

    let snapshot = sarpetorp
        .0
        .try_lock()
        .ok()
        .map(|guard| guard.clone())
        .unwrap_or_default();

    let lines = match engine_res.cursor {
        MenuItem::Sensors => sensor_preview_lines(&snapshot),
        MenuItem::Buses => bus_preview_lines(&snapshot),
        _ => Vec::new(),
    };

    for (mut text, mut color, mut vis, line) in q.iter_mut() {
        if let Some((value, line_color)) = lines.get(line.index) {
            if text.0 != *value {
                text.0 = value.clone();
            }
            color.0 = *line_color;
            *vis = Visibility::Inherited;
        } else {
            *vis = Visibility::Hidden;
        }
    }
}

fn sensor_preview_lines(snapshot: &SarpetorpSnapshot) -> Vec<(String, Color)> {
    if let Some(error) = &snapshot.error {
        return vec![(error.clone(), OAT_DIM)];
    }
    if snapshot.refreshed_at.is_none() {
        return vec![("Laddar Sarpetorp...".to_string(), OAT_DIM)];
    }
    let s = &snapshot.sensors;
    let indoor = temp_label(s.indoor_temp);
    let outdoor = temp_label(s.outdoor_temp);
    let humidity = s
        .indoor_humidity
        .map(|h| format!(" · {:.0}% RH", h))
        .unwrap_or_default();
    let stale = if s.indoor_stale { " · stale" } else { "" };
    let advisory = s.advisory.as_deref().unwrap_or("—");
    let evening = s
        .evening_temp
        .map(|t| format!("{:.1}° ikväll", t))
        .unwrap_or_else(|| "ikväll —".to_string());
    let low = s
        .tonight_low
        .map(|t| format!("Lägsta ute {:.0}°", t))
        .unwrap_or_else(|| "Lägsta ute —".to_string());
    let sun = s.sun_hours.as_deref().unwrap_or("Sol —");
    let tank = match (s.tank_top, s.tank_bottom, s.pipe_outflow) {
        (Some(top), Some(bottom), Some(pipe)) => {
            format!("Tank {:.0}/{:.0}° · panna {:.0}°", top, bottom, pipe)
        }
        (Some(top), Some(bottom), None) => format!("Tank {:.0}/{:.0}°", top, bottom),
        (Some(top), None, _) => format!("Tank topp {:.0}°", top),
        (None, Some(bottom), _) => format!("Tank botten {:.0}°", bottom),
        _ => "Tank —".to_string(),
    };
    vec![
        (format!("Inne {indoor}{humidity}{stale}"), OAT_MILK),
        (format!("{advisory} · {evening}"), AMBER_ACCENT),
        (format!("Ute {outdoor} · {low} · {sun}"), OAT_DIM),
        (tank, OAT_DIM),
        (format!("Inne   {}", s.spark_indoor), OAT_MILK),
        (
            format!("Ute    {}", s.spark_outdoor),
            Color::srgb(0.49, 0.83, 0.99),
        ),
        (
            format!("Tank   {}", s.spark_tank),
            Color::srgb(1.0, 0.67, 0.39),
        ),
        (
            format!("Sol    {}", s.spark_solar),
            Color::srgb(0.98, 0.80, 0.08),
        ),
    ]
}

fn bus_preview_lines(snapshot: &SarpetorpSnapshot) -> Vec<(String, Color)> {
    if let Some(error) = &snapshot.buses.error {
        return vec![(error.clone(), OAT_DIM)];
    }
    if snapshot.refreshed_at.is_none() {
        return vec![("Laddar busstider...".to_string(), OAT_DIM)];
    }
    let mut lines = Vec::new();
    lines.push(("Mot Björkvik & Katrineholm".to_string(), OAT_MILK));
    lines.extend(bus_column_lines(&snapshot.buses.northbound));
    lines.push(("Mot Nyköping".to_string(), OAT_MILK));
    lines.extend(bus_column_lines(&snapshot.buses.southbound));
    if lines.len() == 2 {
        vec![("Inga bussar idag".to_string(), OAT_DIM)]
    } else {
        lines
    }
}

fn bus_column_lines(buses: &[BusDeparture]) -> Vec<(String, Color)> {
    const MAX_DEPARTURES: usize = 6;
    if buses.is_empty() {
        return vec![("Inga avgångar".to_string(), OAT_FAINT)];
    }
    buses
        .iter()
        .take(MAX_DEPARTURES)
        .enumerate()
        .map(|(i, bus)| {
            let time = if i == 0 {
                format_time_display(bus)
            } else {
                bus.departure_time.clone()
            };
            let delay = if bus.delayed_minutes > 0 {
                format!(" +{}m", bus.delayed_minutes)
            } else {
                String::new()
            };
            let stop = if bus.stop_short == "O" {
                "fr Ottekils vsk"
            } else {
                "fr Björkvik"
            };
            let color = if bus.minutes_until <= 10 {
                AMBER_ACCENT
            } else if i == 0 {
                OAT_MILK
            } else {
                OAT_DIM
            };
            (
                format!(
                    "{}  {} {} · {}{}",
                    bus.line_number, time, stop, bus.destination, delay
                ),
                color,
            )
        })
        .collect()
}

fn format_time_display(bus: &BusDeparture) -> String {
    if bus.minutes_until == 0 {
        return format!("{} - nu!", bus.departure_time);
    }
    if bus.minutes_until > 120 {
        return bus.departure_time.clone();
    }
    if bus.minutes_until > 59 {
        let hours = bus.minutes_until / 60;
        let mins = bus.minutes_until % 60;
        if mins > 0 {
            return format!("{} - om {}h {}m", bus.departure_time, hours, mins);
        }
        return format!("{} - om {}h", bus.departure_time, hours);
    }
    if bus.delayed_minutes > 0 {
        return format!(
            "{} - om {}m ({}m sen)",
            bus.departure_time, bus.minutes_until, bus.delayed_minutes
        );
    }
    format!("{} - om {}m", bus.departure_time, bus.minutes_until)
}

fn temp_label(temp: Option<f64>) -> String {
    temp.map(|t| format!("{:.1}°", t))
        .unwrap_or_else(|| "—".to_string())
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
