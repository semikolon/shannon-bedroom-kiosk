//! Shannon bedroom kiosk — Phase 2 main menu UI.
//!
//! Amber-on-black retro aesthetic, Press Start 2P pixel font, flat
//! 5-item menu navigated with the Xbox controller D-pad (or left
//! stick). A = select (logs for now — will wire to handlers in Phase
//! 3), B = back.
//!
//! Architectural notes (full plan: `~/dotfiles/docs/shannon_bedroom_kiosk_plan_2026_05_06.md`):
//! - Always-on retro CRT/scanline aesthetic
//! - Input: Xbox controller via bevy_gilrs. D-pad/stick navigates,
//!   A=select, B=back, Start=pause/menu (future)
//! - Actions: spawn external apps (retroarch, chromium streaming) via
//!   a sibling `shannon-kiosk-actions` daemon (Phase 3)
//! - Streaming services: launch Chromium kiosk on-demand for
//!   YouTube/Netflix/HBO; return to this UI when Chromium exits
//!
//! Render path: HW-GLES via Mesa Panfrost on Mali T860 (Midgard 4th
//! gen). Unlocked May 13, 2026 by the vendored wgpu-hal patches in
//! `vendored/wgpu-hal-0.21.1-mali-fix/`. See shannon-kiosk.sh for the
//! env stack and the research doc § A for the full diagnosis.

use bevy::input::gamepad::{GamepadAxisChangedEvent, GamepadButtonChangedEvent};
use bevy::prelude::*;
use bevy::render::settings::{Backends, WgpuLimits, WgpuSettings, WgpuSettingsPriority};
use bevy::render::RenderPlugin;
use bevy::winit::WinitSettings;

// ----- Palette (CRT-amber-on-black, like an old arcade cabinet) -----
const AMBER_BRIGHT: Color = Color::srgb(0.95, 0.65, 0.05);
const AMBER_DIM: Color = Color::srgb(0.55, 0.35, 0.02);
const AMBER_FAINT: Color = Color::srgb(0.32, 0.21, 0.01);
const BG: Color = Color::srgb(0.02, 0.02, 0.04);

// ----- Menu definition (Phase 2: flat. Phase 3 may go nested.) -----
const MENU_ITEMS: &[&str] = &["GAMES", "MEDIA", "LIGHTS", "SENSORS", "SLEEP"];

#[derive(Resource, Default)]
struct MenuState {
    selected: usize,
}

#[derive(Component)]
struct MenuItemLabel {
    index: usize,
}

#[derive(Component)]
struct StatusText;

fn main() {
    App::new()
        // FPS-capped reactive mode (~30 Hz max), chosen to mitigate
        // Mali T860 Panfrost stability issues. Uncapped continuous render
        // (WinitSettings::game()) wedged Shannon within 15 s on May 13.
        // The hypothesis (per research — Maíra Canal's GPU-sched-leak
        // patch series + RK3399 voltage-coupling devfreq instability):
        // sustained high-rate GPU job submission stresses the Panfrost
        // job scheduler + devfreq OPP transitions to a kernel-wedge
        // state. Capping submission to ~30/sec drastically reduces job
        // queue pressure without sacrificing input responsiveness.
        //
        // 33 ms = ~30 fps. Lower latency than desktop_app() (which uses
        // a 24-hour wait) by orders of magnitude — gamepad input visible
        // within 33 ms.
        .insert_resource(WinitSettings {
            focused_mode: bevy::winit::UpdateMode::Reactive {
                wait: std::time::Duration::from_millis(33),
                react_to_device_events: true,
                react_to_user_events: true,
                react_to_window_events: true,
            },
            unfocused_mode: bevy::winit::UpdateMode::Reactive {
                wait: std::time::Duration::from_millis(33),
                react_to_device_events: true,
                react_to_user_events: true,
                react_to_window_events: true,
            },
        })
        .insert_resource(ClearColor(BG))
        .init_resource::<MenuState>()
        .add_plugins(
            DefaultPlugins
                .set(RenderPlugin {
                    // HW-GLES via Mesa Panfrost on Mali T860 (Midgard 4th
                    // gen). Custom Limits: WebGL2-baseline (compute=0,
                    // matches Panfrost-on-Midgard which has 0 SSBO support
                    // and caps max_compute_workgroup_size_y at 128 vs
                    // wgpu's default 256), but with max_texture_dimension_2d
                    // bumped to 4096 to fit Shannon's 3440×1440 ultrawide.
                    // See `bedroom_kiosk_gpu_research_2026_05_06.md` § A.
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
                        resolution: (1280., 720.).into(),
                        resizable: true,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                gamepad_status_system,
                menu_navigation_system,
                menu_render_system,
            ),
        )
        .run();
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    let font: Handle<Font> = asset_server.load("fonts/PressStart2P-Regular.ttf");

    commands.spawn(Camera2dBundle::default());

    // Title — large amber, the only chrome that ever "shouts"
    commands.spawn(TextBundle {
        text: Text::from_section(
            "SHANNON",
            TextStyle {
                font: font.clone(),
                font_size: 96.0,
                color: AMBER_BRIGHT,
            },
        ),
        style: Style {
            position_type: PositionType::Absolute,
            top: Val::Px(60.0),
            justify_self: JustifySelf::Center,
            ..default()
        },
        ..default()
    });

    // Subtitle
    commands.spawn(TextBundle {
        text: Text::from_section(
            "BEDROOM KIOSK",
            TextStyle {
                font: font.clone(),
                font_size: 18.0,
                color: AMBER_FAINT,
            },
        ),
        style: Style {
            position_type: PositionType::Absolute,
            top: Val::Px(180.0),
            justify_self: JustifySelf::Center,
            ..default()
        },
        ..default()
    });

    // Menu items — vertically stacked, first item starts highlighted.
    // Layout numbers chosen for 1280×720 fallback; ultrawide just
    // centers more dead space horizontally — fine for v1.
    for (i, label) in MENU_ITEMS.iter().enumerate() {
        let y = 320.0 + (i as f32 * 60.0);
        let initial_color = if i == 0 { AMBER_BRIGHT } else { AMBER_DIM };
        let initial_value = if i == 0 {
            format!(">  {}  <", label)
        } else {
            format!("   {}   ", label)
        };
        commands.spawn((
            TextBundle {
                text: Text::from_section(
                    initial_value,
                    TextStyle {
                        font: font.clone(),
                        font_size: 28.0,
                        color: initial_color,
                    },
                ),
                style: Style {
                    position_type: PositionType::Absolute,
                    top: Val::Px(y),
                    justify_self: JustifySelf::Center,
                    ..default()
                },
                ..default()
            },
            MenuItemLabel { index: i },
        ));
    }

    // Gamepad status badge (top-right)
    commands.spawn((
        TextBundle {
            text: Text::from_section(
                "[ no controller ]",
                TextStyle {
                    font: font.clone(),
                    font_size: 12.0,
                    color: AMBER_FAINT,
                },
            ),
            style: Style {
                position_type: PositionType::Absolute,
                top: Val::Px(20.0),
                right: Val::Px(20.0),
                ..default()
            },
            ..default()
        },
        StatusText,
    ));

    // Footer hint (bottom-center)
    commands.spawn(TextBundle {
        text: Text::from_section(
            "D-PAD: NAVIGATE   A: SELECT   B: BACK",
            TextStyle {
                font: font.clone(),
                font_size: 14.0,
                color: AMBER_FAINT,
            },
        ),
        style: Style {
            position_type: PositionType::Absolute,
            bottom: Val::Px(40.0),
            justify_self: JustifySelf::Center,
            ..default()
        },
        ..default()
    });
}

fn gamepad_status_system(gamepads: Res<Gamepads>, mut q: Query<&mut Text, With<StatusText>>) {
    let txt = if gamepads.iter().count() == 0 {
        "[ no controller ]".to_string()
    } else {
        let names: Vec<_> = gamepads
            .iter()
            .map(|g| gamepads.name(g).unwrap_or("?").to_string())
            .collect();
        format!("[ {} ]", names.join(" + "))
    };
    if let Ok(mut text) = q.get_single_mut() {
        if text.sections[0].value != txt {
            text.sections[0].value = txt;
        }
    }
}

fn menu_navigation_system(
    mut button_events: EventReader<GamepadButtonChangedEvent>,
    mut axis_events: EventReader<GamepadAxisChangedEvent>,
    mut state: ResMut<MenuState>,
) {
    let n = MENU_ITEMS.len();

    for ev in button_events.read() {
        // Debug: log every button event so we can verify input flow
        // in the journal. Drop this once stable Phase 2 is verified.
        info!("gamepad button {:?} value={}", ev.button_type, ev.value);
        // Only react on press (value > 0.5), not release
        if ev.value <= 0.5 {
            continue;
        }
        match ev.button_type {
            GamepadButtonType::DPadUp => {
                state.selected = if state.selected == 0 {
                    n - 1
                } else {
                    state.selected - 1
                };
            }
            GamepadButtonType::DPadDown => {
                state.selected = (state.selected + 1) % n;
            }
            GamepadButtonType::South => {
                // A button on Xbox controller — "select"
                info!("Selected: {}", MENU_ITEMS[state.selected]);
            }
            GamepadButtonType::East => {
                // B button — "back" (no-op for now; will pop nav stack
                // when nested menus arrive)
                info!("Back");
            }
            _ => {}
        }
    }

    // Left stick Y-axis as secondary nav. Pushing up = +1.0 on Bevy/
    // gilrs convention; down = -1.0. Edge-trigger on |y|>0.7 to avoid
    // multiple steps per push.
    for ev in axis_events.read() {
        if !matches!(ev.axis_type, GamepadAxisType::LeftStickY) {
            continue;
        }
        if ev.value > 0.7 {
            state.selected = if state.selected == 0 {
                n - 1
            } else {
                state.selected - 1
            };
        } else if ev.value < -0.7 {
            state.selected = (state.selected + 1) % n;
        }
    }
}

fn menu_render_system(state: Res<MenuState>, mut q: Query<(&mut Text, &MenuItemLabel)>) {
    if !state.is_changed() {
        return;
    }
    for (mut text, item) in q.iter_mut() {
        let is_selected = item.index == state.selected;
        text.sections[0].style.color = if is_selected { AMBER_BRIGHT } else { AMBER_DIM };
        let label = MENU_ITEMS[item.index];
        text.sections[0].value = if is_selected {
            format!(">  {}  <", label)
        } else {
            format!("   {}   ", label)
        };
    }
}
