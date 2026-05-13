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

1. **`WinitSettings::desktop_app()` is mandatory** under lavapipe (CPU software Vulkan), not optional. Default Bevy continuous render loop on lavapipe pegs all 6 cores → kernel softlockup. Reactive update mode (event-driven) is the canonical Shannon path. **Once HW-GLES path is working (see § "Next move" below), reactive mode becomes optional but still recommended for power efficiency.**
2. **Don't enable HW Vulkan via panvk** on Mali T860. The panfrost ICD is renamed to `.disabled-mali-t860-panvk-stall` permanently. **Midgard panvk was deliberately removed from Mesa upstream in 2022** and will not return. Do NOT unrename, do NOT retest. See `~/dotfiles/system/shannon/README.md` § "GPU stack on Shannon".
3. **Always `sync`** after any `scp` / `ssh shannon "cat > ..."` / `shannon-mode set` — see `~/dotfiles/system/shannon/README.md` § "Workload policy".
4. **Watchdog won't auto-recover most freezes** — pid1 keeps petting `/dev/watchdog0` even when userspace is wedged. Always pull-and-replug Shannon's USB-C power.
5. **NEVER add the kiosk to systemd auto-start** until stable for many days. Currently `shannon-display.service` is `disabled` deliberately — boot to console, kiosk only via explicit `shannon-mode now`. This makes power-cycle recovery from a bad kiosk change graceful.

## Status (May 13-14, 2026 — evening)

**User direction on GPU stability tradeoffs** (May 13, 2026, verbatim — load-bearing for any future "should we accept software rendering?" temptation): *"I want you to keep working to get GPU support working. I do not want you to revert to software rendering. It's too slow. I want GPU acceleration working."* Lavapipe (CPU software Vulkan) is the documented Phase 1 fallback but is NOT acceptable as a long-term answer. The mitigation strategies in research doc § G2 are ordered around finding a stable HW-GPU path, not accepting a software-render answer.


### ✅ HW-GLES path UNLOCKED end-to-end through wgpu/Bevy stack

Vendored wgpu-hal 0.21.1-mali-fix at `vendored/wgpu-hal-0.21.1-mali-fix/` with 4-patch evolution (`dc09bfb`, `715917a`, `cd615e0`, `2128543`):
1. **Robustness retry loop** Core→Ext→None on `BadAttribute|BadMatch|BadConfig` (backports upstream PRs #7952 + #9153)
2. **`WGPU_GL_PREFER_GLES=1` env override** forces `bind_api(OPENGL_ES_API)` instead of probing CLIENT_APIS for "OpenGL" — Panfrost desktop-GL caps at 3.1 below wgpu's 3.3 floor; its GLES 3.1 satisfies wgpu's GLES-backend min of 3.0+
3. **`EGL_PLATFORM=x11` veto** on Wayland EGL platform selection — matches the X11 window Bevy emits via Xwayland under cage; ALSO drop the `wayland` Bevy feature so winit can only emit X11 windows (bypasses the wgpu-hal Wayland re-init bug that terminates the AdapterContext's display)
4. **`max_texture_dimension_2d=4096`** to fit ultrawide 3440×1440 monitor (default 2048 is the WebGL2 floor)

Bevy `WgpuSettings::priority=WebGL2` + custom limits keep us in the GLES 3.0/WebGL2 feature subset (Panfrost-on-Midgard doesn't implement VERTEX_STORAGE and caps compute_workgroup at 128). `WinitSettings::Reactive { wait: 33 ms }` = ~30 fps cap.

**Phase 2 retro UI rendered visually** on the bedroom monitor — amber-on-black "SHANNON" title + 5-item menu (GAMES / MEDIA / LIGHTS / SENSORS / SLEEP) with Press Start 2P pixel font. User confirmed visible briefly before each freeze.

### 🟥 BLOCKER — sustained kiosk runtime wedges Shannon

Within seconds-to-minutes of `shannon-display.service` start, Shannon deep-wedges: ICMP unresponsive, SSH dies, hardware watchdog never fires (pid1 stays alive enough to pet it), no kernel panic, no pstore capture. Heartbeat shows perfectly normal state right before freeze (load 0.13, temp 55C, no PSI, no dirty pages) — **cliff-edge wedge with zero warning**. Only recovery: power-cycle.

**Mitigations attempted May 13 (all FAILED to stabilize)**:
- `PAN_MESA_DEBUG=no_afbc` (Mesa Arm Frame Buffer Compression off)
- `WGPU_GLES_MINOR_VERSION=0` (force GLES 3.0 code paths)
- `WinitSettings::game()` (continuous render) — wedges in ~15s
- `WinitSettings::Reactive { wait: 33 ms }` (30 Hz cap) — wedges in ~30s
- `echo $max > /sys/class/devfreq/ff9a0000.gpu/min_freq` (clamp GPU min freq high; eliminate OPP transitions) — write succeeded cleanly, kiosk still wedges after launch

**Telemetry observation**: at moment of wedge, simple_ondemand governor downclocked GPU to 200MHz reading Bevy's bursty 30Hz workload as idle. Devfreq is in the picture but not the sole trigger (pinning min_freq=max didn't prevent the wedge — there's a deeper Mali/Panfrost/wgpu interaction).

Full retrospective + mitigation table + next-session priorities: [`~/dotfiles/docs/bedroom_kiosk_gpu_research_2026_05_06.md`](../../dotfiles/docs/bedroom_kiosk_gpu_research_2026_05_06.md) § G2.

### 🟢 Infrastructure that's now in place (May 13)

- **Smart-plug autonomous recovery**: `~/.local/bin/shannon-power-cycle` (Tuya Cloud OpenAPI). Device ID `bf6687a9d79a30c121ytru`. Empirically 5-sec off-cycle reliably reboots Shannon, comes back in ~30s. HA-independent (HA runs ON Shannon).
- **High-frequency GPU+kernel telemetry**: `system/shannon/usr/local/sbin/shannon-gpu-telemetry` runs every 10s via cron, logs to `/var/log.hdd/shannon-gpu-telemetry.log` (SD-shadow, survives wedges). Captures Panfrost devfreq state, governor, cur/min/max freq, PSI cpu/io/mem, kiosk-procs, thermal.
- **`dmesg-stream` background capture** from mode script — `dmesg -wT` → `/var/log.hdd/dmesg-stream.log`. Status: tonight's freezes happened too fast for ext4 commit=120 to flush; need netconsole UDP for next attempt.
- **pstore + ramoops** configured (item 4 from research doc § E). Empty after every freeze because no kernel panic happens; this class doesn't generate pstore captures.

## Workload policy — DO NOT BUILD ON SHANNON

Per [`~/dotfiles/system/shannon/README.md`](../../dotfiles/system/shannon/README.md) § "Workload policy — heavy I/O OFF Shannon" — Rust crate compiles, large apt installs, anything that writes >50 MB sustained MUST be cross-compiled elsewhere. The canonical Rust cross-compile pattern: source on Mac Mini → rsync to Darwin → `cross build --target aarch64-unknown-linux-gnu --release` on Darwin → scp single binary (~25 MB) to Shannon. **This is followed throughout the kiosk dev cycle.**

## Next-session focus (priority order)

1. **Kernel 6.12 LTS pin** — Helios64 + Pinebook Pro communities both report 6.12 as last stable RK3399 baseline. Armbian has `linux-image-current-rockchip64` LTS branch.
2. **Netconsole UDP setup** (research doc § E item 6) — captures kernel ring buffer over UDP to Mac Mini, survives wedge as long as brcmfmac WiFi stays alive. Vulnerable to WiFi-firmware-crash class but worth setting up — current `dmesg-stream` to SD is too slow.
3. **Mesa 25.3.2 upgrade** — current 25.0.7-2 is ~6 months old, Panfrost has had ongoing fixes.
4. **Drop cage + run direct DRM/KMS** — eliminate Xwayland-under-cage layer entirely. winit supports KMS/DRM-direct.
5. **Drop wgpu + raw GLES via SDL2** — biggest rewrite but Mali Panfrost has been verified stable for retro emulators (Retroarch) and mpv on this exact hardware. The freeze may be wgpu-specific.

Outstanding Phase 2 design choices (NOT blocking — easy to tweak):
- Palette: amber-on-black chosen ✓
- Pixel font: Press Start 2P chosen ✓
- Menu structure: flat 5-item — `GAMES / MEDIA / LIGHTS / SENSORS / SLEEP` ✓

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
