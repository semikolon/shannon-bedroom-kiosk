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
use bevy::log::info;
use bevy::prelude::*;
#[cfg(target_os = "linux")]
use bevy::render::settings::Backends;
use bevy::render::settings::{WgpuLimits, WgpuSettings, WgpuSettingsPriority};
use bevy::render::RenderPlugin;
use bevy::window::WindowResolution;
use bevy::winit::WinitSettings;
use shannon_kiosk::context::{
    ClockMinutes, Config, DisplayState, Engine, Inputs, Manual, Media, MenuItem,
};
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
            dev_keyboard_active: false,
            media: Media::None,
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
                state_badge_system,
            ),
        )
        .run();
}

fn setup_background(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
    let image = Image::from_buffer(
        BG_IMAGE,
        ImageType::Extension("jpg"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Default,
        RenderAssetUsages::default(),
    )
    .expect("Sarpetorp clock-bg.jpg decodes");
    let handle = images.add(image);
    commands.spawn((
        Sprite {
            image: handle,
            custom_size: Some(Vec2::new(BG_FIT_WIDTH, BG_FIT_HEIGHT)),
            color: Color::srgba(1.0, 1.0, 1.0, BG_OPACITY),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, -1.0),
    ));
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
    commands.spawn(Camera2d);

    // Six-tile vertical menu — left side, generous vertical spacing.
    for (i, tile) in MENU.iter().enumerate() {
        let y = 240.0 + (i as f32 * 88.0);
        let label_color = if i == 0 { OAT_MILK } else { OAT_DIM };

        // Cursor marker (▸) — visible only on the selected tile
        commands.spawn((
            Text::new("▸"),
            TextFont {
                font: fonts.bold.clone(),
                font_size: 32.0,
                ..default()
            },
            TextColor(AMBER_ACCENT),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(y),
                left: Val::Px(86.0),
                ..default()
            },
            if i == 0 {
                Visibility::Visible
            } else {
                Visibility::Hidden
            },
            MenuCursorMarker { index: i },
        ));

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

    // Controller chrome bar — bottom strip, text-only for now.
    let chrome_specs = [
        ("A", "SELECT", AMBER_ACCENT),
        ("B", "BACK", OAT_DIM),
        ("Y", "ALL OFF", OAT_DIM),
    ];
    let chrome_y = 1010.0;
    for (i, (button, label, color)) in chrome_specs.iter().enumerate() {
        let x = 120.0 + (i as f32 * 260.0);
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
                top: Val::Px(chrome_y),
                left: Val::Px(x),
                ..default()
            },
        ));
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
                top: Val::Px(chrome_y + 6.0),
                left: Val::Px(x + 36.0),
                ..default()
            },
        ));
    }

    // Engine state badge (top-right)
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
) {
    let n = MENU.len();
    let mut cursor_idx = menu_index_of(engine_res.cursor);

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
            }
            GamepadButton::DPadDown => {
                cursor_idx = (cursor_idx + 1) % n;
            }
            GamepadButton::South => {
                info!("Selected: {:?}", MENU[cursor_idx].item);
            }
            GamepadButton::East => {
                info!("Back");
            }
            GamepadButton::North => {
                engine_res.manual_press = Some(Manual::ForceOff);
                info!("ALL OFF (engine ForceOff)");
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

    engine_res.cursor = MENU[cursor_idx].item;
}

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
    gamepads: Query<&Gamepad>,
    mut engine_res: ResMut<EngineRes>,
) {
    let fresh = engine_res.fresh_controller_input;
    let manual_press = engine_res.manual_press.take();
    engine_res.fresh_controller_input = false;

    engine_res.since_input += time.delta();
    // Presence oracle: an actual Xbox controller, OR the dev-host
    // keyboard fallback (sticky, set on first keypress).
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

    if !fresh {
        let hint = engine_res.engine.hint(&inputs);
        if let Some(predicted) = hint.cursor {
            engine_res.cursor = predicted;
        }
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

fn menu_render_system(
    engine_res: Res<EngineRes>,
    mut label_q: Query<(&mut TextColor, &MenuLabel), Without<MenuIcon>>,
    mut icon_q: Query<(&mut TextColor, &MenuIcon), Without<MenuLabel>>,
    mut cursor_q: Query<(&mut Visibility, &MenuCursorMarker)>,
) {
    if !engine_res.is_changed() {
        return;
    }
    let selected = menu_index_of(engine_res.cursor);

    for (mut color, label) in label_q.iter_mut() {
        color.0 = if label.index == selected {
            OAT_MILK
        } else {
            OAT_DIM
        };
    }
    for (mut color, icon) in icon_q.iter_mut() {
        color.0 = if icon.index == selected {
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
        if text.0 != new_value {
            text.0 = new_value;
        }
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
    if let Ok(mut text) = q.single_mut() {
        if text.0 != label {
            text.0 = label.to_string();
        }
    }
}
