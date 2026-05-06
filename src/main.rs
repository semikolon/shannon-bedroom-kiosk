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
use bevy::winit::WinitSettings;

fn main() {
    App::new()
        // Reactive update mode: Bevy only renders frames in response to input
        // events (gamepad, keyboard, window resize), not continuously at 60fps.
        // CRITICAL on Shannon — lavapipe (CPU software Vulkan) renders take
        // ~hundreds of ms each on RK3399 hexa-core; uncapped continuous render
        // pegs all 6 cores → kernel softlockup. May 6, 2026 forced this:
        // 12s detached probe survived (timeout SIGTERM'd it), but service-mode
        // continuous render froze the host within ~10s. With desktop_app()
        // settings, idle CPU drops near-zero; only re-renders on real input.
        .insert_resource(WinitSettings::desktop_app())
        .add_plugins(DefaultPlugins.set(WindowPlugin {
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
