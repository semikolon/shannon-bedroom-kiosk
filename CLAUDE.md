# shannon-bedroom-kiosk

Bevy 0.14 retro UI for Shannon's bedroom HDMI display. Xbox-controller-driven menu launching games / lights / streaming services / sensors.

**Canonical plan**: `~/dotfiles/docs/shannon_bedroom_kiosk_plan_2026_05_06.md` — vision, phased roadmap, risk register, implementation log. Read this first.

**Phase status (2026-05-06 evening)**: Phase 1 done end-to-end (cross-compile + display + Bevy + Xbox controller). Phase 2 (retro UI shell — pixel font, CRT shader, menu navigation) is the active workstream.

## Architecture

`main.rs` runs Bevy with reactive `WinitSettings::desktop_app()` mode + bevy_gilrs gamepad input. Cross-compiled to aarch64 via cross-rs `:edge` (Cross.toml lists arm64 dev libs). Deployed as `/usr/local/bin/shannon-kiosk` on Shannon, launched under `cage` (Wayland compositor, no Xwayland needed) via `shannon-display.service` MODE=`shannon-kiosk` (mode script at `dotfiles/system/shannon/etc/shannon-modes/shannon-kiosk.sh`).

Action handlers (Phase 3+) will be a separate Rust+axum daemon at `127.0.0.1:8080` — NOT in this binary. Bevy posts to it for game launches / light toggles / Chromium spawns.

## Build & Deploy

**Build (cross-compile on Darwin)**:
```bash
# 1. Sync source to Darwin (excludes target/ + .git/)
rsync -av --exclude='target/' --exclude='.git/' \
  ~/Projects/shannon-bedroom-kiosk/ \
  darwin:~/shannon-kiosk-build/shannon-bedroom-kiosk/

# 2. Cross-compile (~3m41s first build, ~1m23s incremental)
ssh darwin "cd ~/shannon-kiosk-build/shannon-bedroom-kiosk && \
  PKG_CONFIG_ALLOW_CROSS=1 ~/.cargo/bin/cross build \
    --target aarch64-unknown-linux-gnu --release \
    --config build.rustc-wrapper='\"\"'"
```

**Deploy to Shannon** (always sync after — `commit=120` ext4 mount loses /etc edits across freezes):
```bash
ssh darwin 'cat ~/shannon-kiosk-build/shannon-bedroom-kiosk/target/aarch64-unknown-linux-gnu/release/shannon-kiosk' \
  | ssh shannon 'cat > /usr/local/bin/shannon-kiosk && chmod +x /usr/local/bin/shannon-kiosk && sync'
```

**Activate / restart on Shannon**:
```bash
ssh shannon 'shannon-mode now shannon-kiosk; sync'  # also restarts shannon-display.service
```

**Stop kiosk safely** (e.g., before deploys that may crash-loop, or to recover from a bad change):
```bash
ssh shannon 'shannon-mode set blank; systemctl stop shannon-display.service; sync'
```

`shannon-mode set <mode>` updates `/etc/default/shannon-display` durably — change persists across reboot. Use `set` (no service restart) when you want to stop crash-loops without flapping the service.

## Critical constraints (derived from Phase 1 power-cycles)

1. **`WinitSettings::desktop_app()` is mandatory**, not optional. Default Bevy continuous render loop on lavapipe (CPU software Vulkan) pegs all 6 cores → kernel softlockup. Reactive update mode (event-driven) is the canonical Shannon path.
2. **Don't enable HW Vulkan via panvk** on Mali T860. The panfrost ICD is renamed to `.disabled-mali-t860-panvk-stall` deliberately. Mode script forces lavapipe via `VK_ICD_FILENAMES`. Re-enable when Mesa fixes T860 panvk (track Mesa changelogs for Mali Midgard panvk improvements; see `dotfiles/system/shannon/README.md` § "GPU stack on Shannon" for the table).
3. **Always `sync`** after any `scp` / `ssh shannon "cat > ..."` / `shannon-mode set` — see `dotfiles/system/shannon/README.md` § "Workload policy".
4. **Watchdog won't auto-recover most freezes** — pid1 keeps petting `/dev/watchdog0` even when userspace is wedged. Always pull-and-replug Shannon's USB-C power.
5. **NEVER add the kiosk to systemd auto-start** until stable for many days. Currently `shannon-display.service` is `disabled` deliberately — boot to console, kiosk only via explicit `shannon-mode now`. This makes power-cycle recovery from a bad kiosk change graceful.

## Cross-references

- Vision + phased plan + risk register + Phase-1 retrospective: `~/dotfiles/docs/shannon_bedroom_kiosk_plan_2026_05_06.md`
- Shannon hardware + workload policy + GPU stack: `~/dotfiles/system/shannon/README.md`
- Mode infrastructure: `~/dotfiles/system/shannon/etc/shannon-modes/` + `~/dotfiles/system/shannon/etc/systemd/system/shannon-display.service`
- Cross-compile reference (Rust+arm64 dev libs pattern): `~/dotfiles/scripts/shannon-setup/35-spotifyd-cross-install.sh`
- Phase 9 IoT hub context (Shannon's broader role): `~/dotfiles/CLAUDE.md` § "Shannon Phase 9 status"
- Personal IoT umbrella (bedroom audio, retro gaming, Tuya lamps, Argon amp keepalive): `~/dotfiles/docs/personal_iot.md`

## Xbox Wireless Controller (currently paired)

- MAC: `40:8E:2C:60:C8:7C`
- xpadneo DKMS module claims it on connect → `/dev/input/js0` + `event4`
- Re-pair flow if needed: stop kiosk → `bt-agent -c NoInputNoOutput &` (NOT `bluetoothctl agent` — fails with "Failed to register agent object") → `bluetoothctl pair / trust / connect $MAC` while controller in pair mode (Pair button on top edge near USB-C, ~3s hold for rapid blink). Bonded state survives reboot — auto-reconnects.

## Bevy version

Currently Bevy 0.14 (wgpu 0.20). If wgpu's Mali GLES `BadMatch` issue gets fixed in a future Bevy/wgpu release, that may unblock HW-accelerated path on Shannon (would let us drop the `WinitSettings::desktop_app()` reactive constraint and run smoother). Not blocking for Phase 2-3.

If we hit Bevy resource-use ceilings on Shannon, fallback per kiosk plan § "Why Bevy specifically" is `egui + winit` — lighter, but more glue work for retro shaders.
