//! Shannon bedroom kiosk — Phase 3 production UI (Slice 3c steps 1 + 2).
//!
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
//! - **No pixelation** — modern execution of the retro-game-menu UX
//!   patterns; the amber-on-black + Press Start 2P Phase-2 aesthetic was
//!   the GPU-stability stepping stone, now superseded
//!
//! What this slice DOES ship (steps 1 + 2):
//! - Vertical 6-tile menu (left ~1/3) with Lucide icons + Sharp Sans
//!   labels + cursor marker (▸)
//! - Engine integration (Slice 1 + 3a): per-tick `Inputs` assembled
//!   from bevy_gilrs + clock; `Engine::step` + `Engine::hint`
//! - Cursor prediction: user input wins; the hint only nudges when
//!   there's no recent deliberate D-pad nav
//! - Y button mapping to `Manual::ForceOff`
//! - Background image (sarpetorp-clock-bg.jpg, cover-fit + 20% opacity
//!   over the forest-radial base — mirrors the dashboard's CSS layering)
//! - Cursor-driven preview pane (right ~2/3) — stable frame, context-
//!   filled: huge Lucide icon + big label + dim subtitle, all swap with
//!   the cursor. Per-tile content is placeholder text for Slice 3c; real
//!   data lands in 3d/3e
//! - Basic controller chrome bar at the bottom (text-only A/B/Y labels —
//!   formal colored Xbox-styled chips come in a future step)
//!
//! What this slice DEFERS:
//! - Formal Xbox-styled colored circle chips (step 2c follow-up; Bevy
//!   0.14 lacks UI border_radius)
//! - Ribbon line styling (3e wires the actual offer)
//! - 3d: daemon HA polling for media + presence
//! - 3e: ribbon-offer wiring (resume-last-watched)
//! - 3f: `BlackoutTvPower` + `HdmiSignalTvPower` actuators
//! - 3g: Ambient + Off scene roots
//!
//! Render path: HW-GLES via Mesa Panfrost on Mali T860 — unchanged from
//! Phase 2 (the vendored wgpu-hal-0.21.1-mali-fix is still load-bearing).

use bevy::input::gamepad::{GamepadAxisChangedEvent, GamepadButtonChangedEvent};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
#[cfg(target_os = "linux")]
use bevy::render::settings::Backends;
use bevy::render::settings::{WgpuLimits, WgpuSettings, WgpuSettingsPriority};
use bevy::render::texture::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::render::RenderPlugin;
use bevy::winit::WinitSettings;
use shannon_kiosk::context::{
    Action, BlackoutTvPower, ClockMinutes, Config, DisplayState, Engine, Inputs, Manual, Media,
    MenuItem, TvPower,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ─── Sarpetorp forest palette (mirrors the dashboard) ────────────────
// Background = the forest-radial-gradient base (Sarpetorp index.html
// `background: radial-gradient(circle at center, rgb(18,30,20) 0%,
// rgb(15,26,18) 100%)`). The inner color is painted as a flat ClearColor
// behind the bg image; the radial-outer ring is a single-color
// approximation for now (the full radial gradient is observation-tunable).
const FOREST_BG: Color = Color::srgb(0.071, 0.118, 0.078); // rgb(18,30,20)
const OAT_MILK: Color = Color::srgb(0.957, 0.937, 0.898); // primary text
const OAT_DIM: Color = Color::srgb(0.55, 0.56, 0.49); // secondary text
const OAT_FAINT: Color = Color::srgb(0.38, 0.39, 0.36); // tertiary text
const AMBER_ACCENT: Color = Color::srgb(0.94, 0.71, 0.18); // selected + [A] (Sarpetorp "Sol nu" register)

// ─── Embedded font assets ────────────────────────────────────────────
// include_bytes! bundles fonts into the binary — no `assets/` dir
// needed at runtime on Shannon, deploys as a single executable. Sharp
// Sans .otf files are commercial (user-owned personal license),
// gitignored, copied from ~/Library/Fonts/ at dev time per
// design hub § 13.3. Lucide is MIT-licensed + committed.
const SHARP_SANS_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/SharpSans-Semibold.otf");
const SHARP_SANS_BOLD: &[u8] = include_bytes!("../assets/fonts/SharpSans-Bold.otf");
const LUCIDE: &[u8] = include_bytes!("../assets/fonts/Lucide.ttf");

// ─── Embedded background image ──────────────────────────────────────
// Mirrored from the Sarpetorp dashboard's top widget (`clock-bg.jpg`).
// Native 3320×5299 portrait (kiosk-station was rotated 90°); we cover-
// fit it across the 1920×1080 landscape Shannon TV at 20% opacity over
// the forest base — the calm-nature-old-house feel per user 2026-05-20:
// "calm and nature and old-house-y, just like the dashboard."
const BG_IMAGE: &[u8] = include_bytes!("../assets/backgrounds/sarpetorp-clock-bg.jpg");

// Cover-fit scale: max(1920/3320, 1080/5299) = 0.578. Final sprite is
// 1920 × 3063 (width matches; height overflows; top + bottom get
// clipped to window) — center-anchored so the photo's middle band shows.
const BG_FIT_WIDTH: f32 = 1920.0;
const BG_FIT_HEIGHT: f32 = 3063.0;
const BG_OPACITY: f32 = 0.20;

// ─── Lucide codepoints for the six menu tiles ────────────────────────
// Verified against lucide-codepoints.json v1.16.0 (Slice 3b commit).
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

// ─── Bevy resources ──────────────────────────────────────────────────

/// The engine + the per-tick accumulator state Bevy needs to feed it.
#[derive(Resource)]
struct EngineRes {
    engine: Engine,
    state: DisplayState,
    cursor: MenuItem,
    // Idle accumulator — reset on any controller input
    since_input: Duration,
    // Pending inputs collected by `gamepad_event_system`, drained by
    // `engine_tick_system`:
    fresh_controller_input: bool,
    manual_press: Option<Manual>,
    // Sticky flag: any keyboard input on the dev host enables a
    // presence-oracle override so `Engine::decide` enters Kiosk even
    // without a connected Xbox controller. Production Shannon ignores
    // this (no keyboard is wired). Set true permanently on the first
    // keyboard event; reset never (a dev session implies presence).
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
            // Mid-curve midday default until Slice 3d feeds the real
            // Sarpetorp solar curve via the daemon.
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

#[derive(Component)]
struct MenuCursorMarker {
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

/// Per-tile preview content. Slice 3c step 2 ships these as placeholders
/// (icon + label + subtitle); Slice 3d swaps in live data per the
/// stable-frame-context-filled keystone (design § 1).
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
        // Reactive 30 fps cap — preserved from Phase 2; load-bearing
        // for Mali T860 stability under freq caps (see project
        // CLAUDE.md "Hardware-stability + freq-cap mitigation").
        .insert_resource(WinitSettings {
            focused_mode: bevy::winit::UpdateMode::Reactive {
                wait: Duration::from_millis(33),
                react_to_device_events: true,
                react_to_user_events: true,
                react_to_window_events: true,
            },
            unfocused_mode: bevy::winit::UpdateMode::Reactive {
                wait: Duration::from_millis(33),
                react_to_device_events: true,
                react_to_user_events: true,
                react_to_window_events: true,
            },
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
            let snap = spawn_ha_state_poller(daemon_url, Duration::from_secs(interval_secs));
            EngineRes {
                ha_snapshot: Some(snap),
                ..Default::default()
            }
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
                        // Bedroom production target is a 1080p TV
                        // (design § 13 / GPU-research § 1080p note);
                        // ultrawide just centers more dead space.
                        resolution: (1920., 1080.).into(),
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

fn setup_background(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    // Load the bg JPEG from embedded bytes — no runtime assets/ dir
    // dependency. JFIF/JPG is widely supported by `image` crate which
    // Bevy uses under the hood.
    let image = Image::from_buffer(
        BG_IMAGE,
        ImageType::Extension("jpg"),
        CompressedImageFormats::NONE,
        true, // is_srgb: JPG is color, not data
        ImageSampler::Default,
        RenderAssetUsages::default(),
    )
    .expect("Sarpetorp clock-bg.jpg decodes");
    let handle = images.add(image);
    commands.spawn(SpriteBundle {
        texture: handle,
        sprite: Sprite {
            custom_size: Some(Vec2::new(BG_FIT_WIDTH, BG_FIT_HEIGHT)),
            color: Color::srgba(1.0, 1.0, 1.0, BG_OPACITY),
            ..default()
        },
        // z=-1 to render behind the bevy_ui layer (which defaults to
        // z=0 on its own UI camera anyway, but explicit > implicit).
        transform: Transform::from_xyz(0.0, 0.0, -1.0),
        ..default()
    });
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

fn setup_ui(mut commands: Commands, fonts: Res<FontHandles>) {
    commands.spawn(Camera2dBundle::default());

    // Six-tile vertical menu — left side, generous vertical spacing.
    for (i, tile) in MENU.iter().enumerate() {
        let y = 240.0 + (i as f32 * 88.0);

        // Cursor marker (▸) — visible only on the selected tile
        commands.spawn((
            TextBundle {
                text: Text::from_section(
                    "▸",
                    TextStyle {
                        font: fonts.bold.clone(),
                        font_size: 32.0,
                        color: AMBER_ACCENT,
                    },
                ),
                style: Style {
                    position_type: PositionType::Absolute,
                    top: Val::Px(y),
                    left: Val::Px(86.0),
                    ..default()
                },
                visibility: if i == 0 {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                },
                ..default()
            },
            MenuCursorMarker { index: i },
        ));

        // Lucide icon
        commands.spawn((
            TextBundle {
                text: Text::from_section(
                    tile.icon.to_string(),
                    TextStyle {
                        font: fonts.lucide.clone(),
                        font_size: 40.0,
                        color: if i == 0 { OAT_MILK } else { OAT_DIM },
                    },
                ),
                style: Style {
                    position_type: PositionType::Absolute,
                    top: Val::Px(y - 4.0),
                    left: Val::Px(130.0),
                    ..default()
                },
                ..default()
            },
            MenuIcon { index: i },
        ));

        // Label — ALL CAPS Sharp Sans Bold with slight letter-spacing
        commands.spawn((
            TextBundle {
                text: Text::from_section(
                    tile.label,
                    TextStyle {
                        font: fonts.bold.clone(),
                        font_size: 34.0,
                        color: if i == 0 { OAT_MILK } else { OAT_DIM },
                    },
                ),
                style: Style {
                    position_type: PositionType::Absolute,
                    top: Val::Px(y),
                    left: Val::Px(200.0),
                    ..default()
                },
                ..default()
            },
            MenuLabel { index: i },
        ));
    }

    // ─── Cursor-driven preview pane (right 2/3) ────────────────────
    // Stable frame, context-filled (design § 1): the panel is always
    // there, its contents update with the cursor. Slice 3c step 2 ships
    // placeholder per-tile content (icon + label + subtitle); Slice 3d
    // swaps in live data from the daemon's HA polling.
    let (default_icon, default_label, default_subtitle) = preview_for(MENU[0].item);

    // Subtle inner-card background to define the pane's region. Bevy
    // 0.14 has no built-in border_radius for UI nodes (introduced in
    // 0.15) — square corners for now; rounding can ride on a later
    // Bevy upgrade per design § 13.13 observation-tuned parameters.
    commands.spawn(NodeBundle {
        style: Style {
            position_type: PositionType::Absolute,
            top: Val::Px(180.0),
            left: Val::Px(540.0),
            width: Val::Px(1320.0),
            height: Val::Px(720.0),
            ..default()
        },
        background_color: Color::srgba(0.043, 0.071, 0.047, 0.40).into(),
        border_color: Color::srgba(0.13, 0.18, 0.13, 0.30).into(),
        ..default()
    });

    // Preview icon — huge Lucide glyph, single-accent oat-milk
    commands.spawn((
        TextBundle {
            text: Text::from_section(
                default_icon.to_string(),
                TextStyle {
                    font: fonts.lucide.clone(),
                    font_size: 220.0,
                    color: OAT_MILK,
                },
            ),
            style: Style {
                position_type: PositionType::Absolute,
                top: Val::Px(260.0),
                left: Val::Px(620.0),
                ..default()
            },
            ..default()
        },
        PreviewElement::Icon,
    ));

    // Preview label — big Sharp Sans Bold ALL-CAPS, oat-milk
    commands.spawn((
        TextBundle {
            text: Text::from_section(
                default_label,
                TextStyle {
                    font: fonts.bold.clone(),
                    font_size: 92.0,
                    color: OAT_MILK,
                },
            ),
            style: Style {
                position_type: PositionType::Absolute,
                top: Val::Px(290.0),
                left: Val::Px(890.0),
                ..default()
            },
            ..default()
        },
        PreviewElement::Label,
    ));

    // Preview subtitle — Sharp Sans Semibold, dimmed
    commands.spawn((
        TextBundle {
            text: Text::from_section(
                default_subtitle,
                TextStyle {
                    font: fonts.semibold.clone(),
                    font_size: 32.0,
                    color: OAT_DIM,
                },
            ),
            style: Style {
                position_type: PositionType::Absolute,
                top: Val::Px(410.0),
                left: Val::Px(890.0),
                ..default()
            },
            ..default()
        },
        PreviewElement::Subtitle,
    ));

    // Controller chrome bar — bottom strip, text-only for now.
    // Formal Xbox-styled colored circle chips deferred (Bevy 0.14 has
    // no native UI border_radius; rounded chips need either a sprite-
    // texture approach or a Bevy 0.15 upgrade).
    let chrome_specs = [
        ("A", "SELECT", AMBER_ACCENT),
        ("B", "BACK", OAT_DIM),
        ("Y", "ALL OFF", OAT_DIM),
    ];
    let chrome_y = 1010.0;
    for (i, (button, label, color)) in chrome_specs.iter().enumerate() {
        let x = 120.0 + (i as f32 * 260.0);
        commands.spawn(TextBundle {
            text: Text::from_section(
                button.to_string(),
                TextStyle {
                    font: fonts.bold.clone(),
                    font_size: 26.0,
                    color: *color,
                },
            ),
            style: Style {
                position_type: PositionType::Absolute,
                top: Val::Px(chrome_y),
                left: Val::Px(x),
                ..default()
            },
            ..default()
        });
        commands.spawn(TextBundle {
            text: Text::from_section(
                label.to_string(),
                TextStyle {
                    font: fonts.semibold.clone(),
                    font_size: 18.0,
                    color: OAT_FAINT,
                },
            ),
            style: Style {
                position_type: PositionType::Absolute,
                top: Val::Px(chrome_y + 6.0),
                left: Val::Px(x + 36.0),
                ..default()
            },
            ..default()
        });
    }

    // Resume-offer ribbon line — sits above the controller chrome.
    // Starts hidden; engine's KioskHint.ribbon turns it on when there's
    // a confident resume offer (Slice 3e). Centered, single line, with
    // a leading `[A]` chip in amber to match the chrome.
    commands.spawn((
        TextBundle {
            text: Text::from_sections([
                // amber [A] chip
                TextSection::new(
                    "[A]  ",
                    TextStyle {
                        font: fonts.bold.clone(),
                        font_size: 22.0,
                        color: AMBER_ACCENT,
                    },
                ),
                // ribbon text (engine-supplied)
                TextSection::new(
                    "",
                    TextStyle {
                        font: fonts.semibold.clone(),
                        font_size: 22.0,
                        color: OAT_MILK,
                    },
                ),
            ])
            .with_justify(JustifyText::Center),
            style: Style {
                position_type: PositionType::Absolute,
                top: Val::Px(965.0), // just above chrome (chrome_y=1010)
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                ..default()
            },
            visibility: Visibility::Hidden, // engine turns on when confident
            ..default()
        },
        RibbonLabel,
    ));

    // Full-screen Ambient canvas (Slice 3g). Below the blackout
    // overlay (z=300) so Off state still blacks-out over Ambient.
    // Color is set per-tick by ambient_render_system to amber × engine
    // brightness; spawned at full amber so first paint isn't a flash.
    commands.spawn((
        NodeBundle {
            style: Style {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(0.0),
                ..default()
            },
            background_color: AMBER_ACCENT.into(),
            visibility: Visibility::Hidden, // engine flips on Ambient state
            z_index: ZIndex::Global(300),
            ..default()
        },
        AmbientCanvas,
    ));

    // Full-screen blackout overlay (Slice 3f). Spawned BEFORE the
    // state badge so the badge stays visible on top (dev observability
    // — even when blackout is on you can see the engine state in the
    // corner). On Shannon production this badge will be hidden.
    // ZIndex::Global pins it just below the badge regardless of
    // future spawn-order changes.
    commands.spawn((
        NodeBundle {
            style: Style {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(0.0),
                ..default()
            },
            background_color: Color::BLACK.into(),
            visibility: Visibility::Hidden, // starts off; engine flips on TV-off
            z_index: ZIndex::Global(500),
            ..default()
        },
        BlackoutOverlay,
    ));

    // Engine state badge (top-right) — useful for dev iteration to see
    // the engine in action. May be removed once the preview pane's
    // content fully signals the engine state contextually.
    commands.spawn((
        TextBundle {
            text: Text::from_section(
                "—",
                TextStyle {
                    font: fonts.semibold.clone(),
                    font_size: 18.0,
                    color: OAT_FAINT,
                },
            ),
            style: Style {
                position_type: PositionType::Absolute,
                top: Val::Px(30.0),
                right: Val::Px(40.0),
                ..default()
            },
            ..default()
        },
        StateBadge,
    ));
}

// ─── Systems ─────────────────────────────────────────────────────────

fn gamepad_event_system(
    mut button_events: EventReader<GamepadButtonChangedEvent>,
    mut axis_events: EventReader<GamepadAxisChangedEvent>,
    mut engine_res: ResMut<EngineRes>,
) {
    let n = MENU.len();
    let mut cursor_idx = menu_index_of(engine_res.cursor);

    for ev in button_events.read() {
        if ev.value <= 0.5 {
            continue; // edge-trigger on press only
        }
        engine_res.fresh_controller_input = true;
        engine_res.since_input = Duration::ZERO;
        match ev.button_type {
            GamepadButtonType::DPadUp => {
                cursor_idx = if cursor_idx == 0 {
                    n - 1
                } else {
                    cursor_idx - 1
                };
            }
            GamepadButtonType::DPadDown => {
                cursor_idx = (cursor_idx + 1) % n;
            }
            GamepadButtonType::South => {
                // A — select. Slice 3c step 1 logs only; submenu
                // launches wire in via the Slice 2 daemon (Slice 3d/3e).
                info!("Selected: {:?}", MENU[cursor_idx].item);
            }
            GamepadButtonType::East => {
                // B — back. No-op for now; submenu nav stack arrives
                // when tiles have content beyond the launcher row.
                info!("Back");
            }
            GamepadButtonType::North => {
                // Y — ALL OFF (engine `Manual::ForceOff`). The engine
                // makes it sticky; a fresh controller input later
                // clears it (precedence rule 2).
                engine_res.manual_press = Some(Manual::ForceOff);
                info!("ALL OFF (engine ForceOff)");
            }
            _ => {}
        }
    }

    for ev in axis_events.read() {
        if !matches!(ev.axis_type, GamepadAxisType::LeftStickY) {
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

    engine_res.cursor = MENU[cursor_idx].item;
}

/// Keyboard fallback for dev iteration on the Mac (where the Xbox
/// controller may not be paired). Mirrors gamepad mappings:
/// Arrow Up/Down → cursor; Enter/Space → A; Escape → B; Q → Y (ALL OFF).
/// Any keypress flips the sticky `dev_keyboard_active` flag so the
/// engine's presence-oracle treats the user as present (otherwise the
/// engine sits in Off forever, with no Xbox controller wired).
/// Production Shannon ignores this (no keyboard in the bedroom).
fn keyboard_event_system(keys: Res<ButtonInput<KeyCode>>, mut engine_res: ResMut<EngineRes>) {
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

    if any_press {
        engine_res.fresh_controller_input = true;
        engine_res.since_input = Duration::ZERO;
        engine_res.dev_keyboard_active = true;
        engine_res.cursor = MENU[cursor_idx].item;
    }
}

fn engine_tick_system(
    time: Res<Time<Real>>,
    gamepads: Res<Gamepads>,
    mut engine_res: ResMut<EngineRes>,
) {
    // Drain pending inputs (mut takes are scoped so the borrow ends
    // before the engine call).
    let fresh = engine_res.fresh_controller_input;
    let manual_press = engine_res.manual_press.take();
    engine_res.fresh_controller_input = false;

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

    // Presence oracle: an actual Xbox controller, OR the dev-host
    // keyboard fallback (sticky, set on first keypress). Production
    // Shannon never sees the keyboard path; Mac dev iteration relies on
    // it to demo without an Xbox controller paired to the Mac.
    // Slice 3e: HA occupancy is informational only — the controller-BT
    // oracle remains the canonical presence signal (per design hub §3).
    // ha_occupancy may eventually OR in for the disconnect-grace path.
    let controller_connected = gamepads.iter().count() > 0 || engine_res.dev_keyboard_active;

    // Wall-clock as minutes-since-midnight (local time). Slice 3d may
    // route this through the daemon for fleet-coherent time; for now,
    // the dev host's clock is enough.
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

    // Apply the predicted cursor ONLY when the user hasn't just acted —
    // deliberate D-pad nav must dominate the auto-prediction. Also
    // compute the ribbon offer now (Slice 3e) — the engine gates
    // confidence per the host-supplied resume title.
    let title_for_hint = engine_res.ha_ribbon_title.clone();
    let hint = engine_res
        .engine
        .hint_with_offer(&inputs, title_for_hint.as_deref());
    if !fresh {
        if let Some(predicted) = hint.cursor {
            engine_res.cursor = predicted;
        }
    }
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

/// Local wall-clock as `ClockMinutes`. Uses chrono-free std::time +
/// Sweden's standard UTC offset (UTC+1 winter / UTC+2 summer DST handled
/// by querying the OS for the local time zone in Slice 3d). For dev
/// iteration today, a coarse hard-coded CEST offset is acceptable — the
/// engine's hard-off / wind-down windows are observation-tuned anyway.
fn current_local_minutes() -> ClockMinutes {
    use std::time::{SystemTime, UNIX_EPOCH};
    let utc_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Sweden CEST = UTC+2 (May = summer). Slice 3d wires real
    // tz-aware conversion via chrono or the daemon's clock.
    const SWEDEN_OFFSET_SECS: u64 = 2 * 60 * 60;
    let local_secs = utc_secs.saturating_add(SWEDEN_OFFSET_SECS);
    let minutes_of_day = ((local_secs / 60) % (24 * 60)) as u16;
    ClockMinutes::at(minutes_of_day / 60, minutes_of_day % 60)
}

fn menu_render_system(
    engine_res: Res<EngineRes>,
    mut label_q: Query<(&mut Text, &MenuLabel), Without<MenuIcon>>,
    mut icon_q: Query<(&mut Text, &MenuIcon), Without<MenuLabel>>,
    mut cursor_q: Query<(&mut Visibility, &MenuCursorMarker)>,
) {
    if !engine_res.is_changed() {
        return;
    }
    let selected = menu_index_of(engine_res.cursor);

    for (mut text, label) in label_q.iter_mut() {
        text.sections[0].style.color = if label.index == selected {
            OAT_MILK
        } else {
            OAT_DIM
        };
    }
    for (mut text, icon) in icon_q.iter_mut() {
        text.sections[0].style.color = if icon.index == selected {
            OAT_MILK
        } else {
            OAT_DIM
        };
    }
    for (mut vis, marker) in cursor_q.iter_mut() {
        *vis = if marker.index == selected {
            Visibility::Visible
        } else {
            Visibility::Hidden
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
        if text.sections[0].value != new_value {
            text.sections[0].value = new_value;
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
    let Ok((mut text, mut vis)) = q.get_single_mut() else {
        return;
    };
    match &engine_res.ribbon_text {
        Some(t) => {
            // Section[0] is the "[A]  " amber chip; section[1] is the
            // engine-computed text — we only update section[1].
            if text.sections.len() >= 2 && text.sections[1].value != *t {
                text.sections[1].value = t.clone();
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
    let Ok((mut vis, mut bg)) = q.get_single_mut() else {
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
    let Ok(mut vis) = q.get_single_mut() else {
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
    if let Ok(mut text) = q.get_single_mut() {
        if text.sections[0].value != label {
            text.sections[0].value = label.to_string();
        }
    }
}
