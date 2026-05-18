//! shannon-kiosk-minimal — minimal Bevy app for isolating the wedge cause.
//!
//! Just a Bevy DefaultPlugins app with a Camera2dBundle + dark-blue clear
//! color. NO menu items, NO font asset load, NO gamepad handling, NO
//! Update systems beyond what DefaultPlugins enables. Same wgpu/wgpu-hal
//! stack as the main kiosk (Mali T860 HW-GLES via the vendored patches).
//!
//! Goal: if THIS wedges Shannon within minutes, the wedge is in the
//! Bevy/wgpu/Mali init or render-loop core — independent of our Phase 2
//! menu code. If it runs stably, our menu code (font load, text spawns,
//! gamepad polling) triggers something specific.

use bevy::prelude::*;
use bevy::render::settings::{Backends, WgpuLimits, WgpuSettings, WgpuSettingsPriority};
use bevy::render::RenderPlugin;
use bevy::winit::WinitSettings;

fn main() {
    App::new()
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
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.04)))
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
                        title: "shannon-kiosk-minimal".to_string(),
                        resolution: (1280., 720.).into(),
                        resizable: true,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera2dBundle::default());
        })
        .run();
}
