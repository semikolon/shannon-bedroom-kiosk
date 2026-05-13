//! Shannon bedroom kiosk — main menu UI.
//!
//! Phase 1 (current): hello-world window, gamepad detection, placeholder
//! "SHANNON" text. Validates that Bevy compiles + runs on macOS dev (Mac
//! Mini) and aarch64 Linux deploy (Shannon RK3399). Once the bedroom
//! display is connected we'll push this to Shannon and verify cage +
//! Bevy + Xbox-controller end-to-end.
//!
//! Architectural notes (full plan: `~/dotfiles/docs/shannon_bedroom_kiosk_plan_2026_05_06.md`):
//! - Default state: kiosk runs always-on, retro CRT/scanline aesthetic
//! - Input: Xbox controller via bevy_gilrs. D-pad/stick navigates,
//!   A=select, B=back, Start=pause/menu
//! - Actions: spawn external apps (retroarch, chromium streaming) via
//!   a sibling `shannon-kiosk-actions` daemon (Phase 3)
//! - Streaming services: launch Chromium kiosk on-demand for
//!   YouTube/Netflix/HBO; return to this UI when Chromium exits

use bevy::prelude::*;
use bevy::input::gamepad::{GamepadAxisChangedEvent, GamepadButtonChangedEvent};
use bevy::render::RenderPlugin;
use bevy::render::settings::{Backends, WgpuLimits, WgpuSettings, WgpuSettingsPriority};
use bevy::winit::WinitSettings;

fn main() {
    App::new()
        // Reactive update mode: Bevy only renders frames in response to input
        // events (gamepad, keyboard, window resize), not continuously at 60fps.
        // CRITICAL on Shannon under lavapipe (CPU software Vulkan) — renders
        // take ~hundreds of ms each on RK3399 hexa-core; uncapped continuous
        // render pegs all 6 cores → kernel softlockup. Still recommended on
        // HW-GLES (Mali T860 + Panfrost) for power efficiency.
        .insert_resource(WinitSettings::desktop_app())
        .add_plugins(DefaultPlugins.set(RenderPlugin {
            // HW-GLES via Mesa Panfrost on Mali T860 (Midgard 4th gen).
            // The hardware exposes OpenGL ES 3.1 (Mesa 25.0.7 driver_info
            // reports "OpenGL ES 3.1 Mesa 25.0.7-2"). wgpu's default
            // `Functionality` priority would require feature flags like
            // VERTEX_STORAGE (SSBO in vertex shaders) which Panfrost on
            // Midgard doesn't support — Bevy's `mesh2d_layout` for
            // example needs it. `WebGL2` priority caps wgpu's required
            // feature set to the WebGL2 / GLES 3.0 downlevel subset which
            // Panfrost-on-T860 satisfies fully. Backend pinned to GL so
            // we don't accidentally try Vulkan (Mali panvk is permanently
            // dead — see shannon-kiosk.sh comments).
            //
            // On Mac Mini Metal, Backends::GL would force GL on macOS too
            // which is unsupported. So we let backends=All on macOS via
            // the cfg below, only constraining on Linux (the Shannon
            // path).
            render_creation: WgpuSettings {
                priority: WgpuSettingsPriority::WebGL2,
                // Set limits EXPLICITLY rather than relying on priority alone
                // — Bevy 0.14's RenderPlugin merges its own additions on top
                // of the priority-derived limits, which bumps things like
                // max_compute_workgroup_size_y back up to 256, exceeding
                // Panfrost-on-Midgard's 128 cap. Pinning limits to
                // `downlevel_webgl2_defaults()` (compute=0 across the board)
                // matches WebGL2's no-compute-shader profile and stays
                // within Panfrost capabilities.
                limits: WgpuLimits::downlevel_webgl2_defaults(),
                #[cfg(target_os = "linux")]
                backends: Some(Backends::GL),
                ..default()
            }.into(),
            ..default()
        }).set(WindowPlugin {
            primary_window: Some(Window {
                title: "Shannon".to_string(),
                resolution: (1280., 720.).into(),
                resizable: true,
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.04)))
        .add_systems(Startup, setup)
        .add_systems(Update, (gamepad_status_system, gamepad_input_system))
        .run();
}

#[derive(Component)]
struct StatusText;

#[derive(Component)]
struct GamepadInputText;

fn setup(mut commands: Commands) {
    commands.spawn(Camera2dBundle::default());

    commands.spawn(TextBundle {
        text: Text::from_section(
            "SHANNON",
            TextStyle {
                font_size: 120.0,
                color: Color::srgb(0.95, 0.65, 0.05),
                ..default()
            },
        )
        .with_justify(JustifyText::Center),
        style: Style {
            position_type: PositionType::Absolute,
            top: Val::Px(80.0),
            justify_self: JustifySelf::Center,
            ..default()
        },
        ..default()
    });

    commands.spawn(TextBundle {
        text: Text::from_section(
            "bedroom kiosk · v0.1",
            TextStyle {
                font_size: 24.0,
                color: Color::srgb(0.6, 0.4, 0.0),
                ..default()
            },
        )
        .with_justify(JustifyText::Center),
        style: Style {
            position_type: PositionType::Absolute,
            top: Val::Px(220.0),
            justify_self: JustifySelf::Center,
            ..default()
        },
        ..default()
    });

    commands.spawn((
        TextBundle {
            text: Text::from_section(
                "[gamepad: searching...]",
                TextStyle {
                    font_size: 18.0,
                    color: Color::srgb(0.4, 0.6, 0.4),
                    ..default()
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

    commands.spawn((
        TextBundle {
            text: Text::from_section(
                "press any controller button...",
                TextStyle {
                    font_size: 22.0,
                    color: Color::srgb(0.5, 0.5, 0.6),
                    ..default()
                },
            ),
            style: Style {
                position_type: PositionType::Absolute,
                bottom: Val::Px(80.0),
                justify_self: JustifySelf::Center,
                ..default()
            },
            ..default()
        },
        GamepadInputText,
    ));
}

fn gamepad_status_system(
    gamepads: Res<Gamepads>,
    mut status_q: Query<&mut Text, With<StatusText>>,
) {
    let count = gamepads.iter().count();
    let summary = if count == 0 {
        "[gamepad: not connected]".to_string()
    } else {
        let names: Vec<_> = gamepads
            .iter()
            .map(|g| gamepads.name(g).unwrap_or("?").to_string())
            .collect();
        format!("[gamepad: {}]", names.join(" + "))
    };
    if let Ok(mut text) = status_q.get_single_mut() {
        text.sections[0].value = summary;
    }
}

fn gamepad_input_system(
    mut button_events: EventReader<GamepadButtonChangedEvent>,
    mut axis_events: EventReader<GamepadAxisChangedEvent>,
    mut input_q: Query<&mut Text, With<GamepadInputText>>,
) {
    let mut latest: Option<String> = None;
    for ev in button_events.read() {
        if ev.value > 0.5 {
            latest = Some(format!("button {:?} pressed", ev.button_type));
        }
    }
    for ev in axis_events.read() {
        if ev.value.abs() > 0.5 {
            latest = Some(format!("axis {:?} = {:.2}", ev.axis_type, ev.value));
        }
    }
    if let Some(msg) = latest {
        if let Ok(mut text) = input_q.get_single_mut() {
            text.sections[0].value = msg;
        }
    }
}
