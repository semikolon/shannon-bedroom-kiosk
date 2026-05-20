//! Shannon bedroom kiosk — Phase 3 production UI (Slice 3c step 1).
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
//! What this slice DOES ship:
//! - Vertical 6-tile menu (left ~1/3) with Lucide icons + Sharp Sans
//!   labels + cursor marker (▸)
//! - Engine integration (Slice 1 + 3a): per-tick `Inputs` assembled
//!   from bevy_gilrs + clock; `Engine::step` + `Engine::hint`
//! - Cursor prediction: user input wins; the hint only nudges when
//!   there's no recent deliberate D-pad nav
//! - Y button mapping to `Manual::ForceOff`
//! - Basic controller chrome bar at the bottom (text-only A/B/Y +
//!   labels — formal colored circle chips come in step 2)
//!
//! What this slice DEFERS (Slice 3c step 2):
//! - Background image (`assets/backgrounds/sarpetorp-clock-bg.jpg`)
//! - Cursor-driven preview pane (right ~2/3 of the screen)
//! - Formal Xbox-styled button chips (colored circles with letters)
//! - Ribbon line styling
//!
//! What this slice DEFERS (later slices, see § 13.11):
//! - 3d: daemon HA polling for media + presence
//! - 3e: ribbon-offer wiring (resume-last-watched)
//! - 3f: `BlackoutTvPower` + `HdmiSignalTvPower` actuators
//! - 3g: Ambient + Off scene roots
//!
//! Render path: HW-GLES via Mesa Panfrost on Mali T860 — unchanged from
//! Phase 2 (the vendored wgpu-hal-0.21.1-mali-fix is still load-bearing).

use bevy::input::gamepad::{GamepadAxisChangedEvent, GamepadButtonChangedEvent};
use bevy::prelude::*;
#[cfg(target_os = "linux")]
use bevy::render::settings::Backends;
use bevy::render::settings::{WgpuLimits, WgpuSettings, WgpuSettingsPriority};
use bevy::render::RenderPlugin;
use bevy::winit::WinitSettings;
use shannon_kiosk::context::{
    ClockMinutes, Config, DisplayState, Engine, Inputs, Manual, Media, MenuItem,
};
use std::time::Duration;

// ─── Sarpetorp forest palette (mirrors the dashboard) ────────────────
// Background = the forest-radial-gradient base (Sarpetorp index.html
// `background: radial-gradient(circle at center, rgb(18,30,20) 0%,
// rgb(15,26,18) 100%)`). Slice 3c step 1 paints the inner color as a
// flat ClearColor — the bg image + radial overlay land in step 2.
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
    // Placeholders for Slice 3d (daemon HA polling):
    media: Media,
    outdoor_brightness: f32,
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
            media: Media::None,
            // Mid-curve midday default until Slice 3d feeds the real
            // Sarpetorp solar curve via the daemon.
            outdoor_brightness: 0.6,
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
        .init_resource::<EngineRes>()
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
        .add_systems(Startup, (load_fonts, setup_ui).chain())
        .add_systems(
            Update,
            (
                gamepad_event_system,
                engine_tick_system,
                menu_render_system,
                state_badge_system,
            ),
        )
        .run();
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

    // Controller chrome bar — bottom strip, text-only for Slice 3c
    // step 1 (formal Xbox-styled colored circle chips come in step 2).
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

    // Engine state badge (top-right) — useful for dev iteration to see
    // the engine in action. Will likely disappear in step 2 once the
    // preview pane carries that signal contextually.
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
    let controller_connected = gamepads.iter().count() > 0;

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

    // Apply the predicted cursor ONLY when the user hasn't just acted —
    // deliberate D-pad nav must dominate the auto-prediction.
    if !fresh {
        let hint = engine_res.engine.hint(&inputs);
        if let Some(predicted) = hint.cursor {
            engine_res.cursor = predicted;
        }
    }
}

/// Local wall-clock as `ClockMinutes`. Uses chrono-free std::time +
/// Sweden's standard UTC offset (UTC+1 winter / UTC+2 summer DST handled
/// by querying the OS for the local time zone in step 2). For Slice 3c
/// step 1 dev iteration, a coarse UTC fallback is acceptable — the
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
