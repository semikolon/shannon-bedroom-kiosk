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

## Status of major pre-Phase-2 unlocks (May 13, 2026)

### ✅ wgpu-hal Mali T860 HW-GLES patch — BUILT (not deployed)

Vendored at `vendored/wgpu-hal-0.21.1-mali-fix/` with the retry-loop backport of upstream PRs #7952 + #9153 applied to `src/gles/egl.rs:548-613` (search `BEGIN mali-fix patch` in the file). Root `Cargo.toml` has `[patch.crates-io] wgpu-hal = { path = ... }`. Cross-build verified on Darwin: `2m 23s` clean, 0 errors, 147 routine wgpu-hal warnings (typical for the upstream code on this backend combo). Output binary at `darwin:~/shannon-kiosk-build/shannon-bedroom-kiosk/target/aarch64-unknown-linux-gnu/release/shannon-kiosk`.

**Patch port specifics**: v0.21.1's `egl.rs` calls `egl.bind_api(...)` once at function entry rather than per-attempt, so we didn't need #9153's closure pattern. The patch just wraps the existing `create_context` call in a retry loop with Core→Ext→None robustness degradation on `BadAttribute | BadMatch | BadConfig` errors. ~100 lines net.

**Deploy preconditions** (still gating — DO NOT push the new binary to Shannon yet):
1. Shannon instrumentation items 4 + 5 fully landed (see § E in research doc)
2. HA-only baseline observation period 24-48h (kiosk MODE=blank, eliminates kiosk-induced freezes from the picture)
3. Mode script change: `~/dotfiles/system/shannon/etc/shannon-modes/shannon-kiosk.sh` needs `VK_ICD_FILENAMES` removed AND `WGPU_BACKEND=gl` added (NOT done yet — keeps lavapipe still active as a revert path)

When all three are met, deploy via the standard pattern:
```bash
ssh darwin 'cat ~/shannon-kiosk-build/shannon-bedroom-kiosk/target/aarch64-unknown-linux-gnu/release/shannon-kiosk' \
  | ssh shannon 'cat > /usr/local/bin/shannon-kiosk && chmod +x /usr/local/bin/shannon-kiosk && sync'
ssh shannon 'shannon-mode now shannon-kiosk; sync'
```
Then verify HW acceleration is actually used (Mesa env: `MESA_DEBUG=1 EGL_LOG_LEVEL=debug` should show `panfrost_dri.so` loaded; GPU frame timing in Bevy diagnostics should drop ~5-10× vs lavapipe).

### 🟨 Shannon instrumentation items 4 + 5 — STAGED (item 5 ready, item 4 awaiting address pick)

Item 5 (userspace canary watchdog) ready to deploy via `hemma system-apply shannon`. Will trigger interactive prompt; user runs manually.

Item 4 (pstore + ramoops) script + systemd unit staged + harmless (inert without ramoops backing). Kernel-cmdline edit deferred — research doc's proposed `0xff000000` is RK3399 SoC MMIO, NOT DRAM. See research doc § E item 4 "Address conflict finding" for the corrected analysis and three options (`memmap=1M$0x70000000` is the most likely candidate; needs user nod before editing `/boot/armbianEnv.txt`).

### 🟨 HA-only baseline 24-48h — pending

Clock has been reset twice today by planned Vattenfall power outages (06:30-08:30 + 14:30-16:30). Restart the baseline window after item 5 lands.

## Next-session focus (Phase 2)

Retro UI shell: pixel-font + main-menu navigation IA + Xbox-controller input mapping + CRT shader (post-processing pass over Bevy's default 2D pipeline). Outstanding decisions:
- Palette: amber-on-black (CRT-monitor era) vs green-on-black (terminal era) vs full-color retro-game palette
- Pixel font: Press Start 2P (the default candidate per research doc) vs an actual NES/SNES bitmap font ripped from a public-domain font pack
- Menu structure: flat (games / movies / lights / quit) vs nested (Games > NES > ...) — first cut is flat for simplicity

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
