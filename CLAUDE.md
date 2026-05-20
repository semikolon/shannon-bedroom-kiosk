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

### 🟥→🟢 May 20, 2026 — HARDWARE-STABILITY ROOT-CAUSE + freq-cap MITIGATION (supersedes the May-17 "PHASE 2 VALIDATED" below)

**Kiosk-with-controller live test ran 2026-05-20** (on the dev ultrawide monitor at desk; bedroom TV move deferred). **Six kiosk launches across full + minimal Bevy, Mesa 25.0.7 + 25.2.6, HA-stopped + HA-running, controller-pressed + controller-untouched** — ALL wedged in 7-45 s. Repeated the May-13 "BLOCKER" pattern that May-17 was thought to have refuted. **Refutation of "GPU-driver" framing**: `stress-ng --cpu 6 --vm 2` with **no GPU activity** ALSO wedged Shannon in <60 s. Mesa 25.2.6 upgrade tested → wedged identically (no menu visible before wedge — possibly faster wedge with newer Mesa, possibly stochastic).

**Root cause** (working hypothesis from this session's data; physical confirmation pending): **SoC misbehaves at peak frequencies under sustained heavy load**. Likely contributors:
- **PMIC droop**: RK808 voltage regulation under peak current (A72 @ 2.016 GHz draws 1.075 V; sustained 6-core load may exceed regulation envelope)
- **Passive cooling marginal**: at A72 = 1.608 GHz cap temp hit 88 °C in 2 min (past CPU passive throttle 85 °C) — die temp likely higher than sensor reads
- **Possibly undersized PSU** (Tuya plug → USB-C wall wart of unknown rating)
- Signal: **PSI irq full ≈ 0.94 even at idle** suggests marginal baseline that any added load pushes over

**Test matrix that converged on the fix**:

| Config (max freqs) | Stress-ng survival | Kiosk-minimal survival |
|---|---|---|
| A53=1.512 / A72=2.016 / GPU=800 (rated max) | wedge in <60 s | wedge in 7-45 s |
| A53=1.512 / A72=**1.608** / GPU=800 | wedge at ~2 min @ 88 °C | n/a |
| A53=**1.008** / A72=**1.008** / GPU=**400** | **3 min clean @ 69 °C peak** | **5+ min stable @ 65-67 °C steady** |

**Mitigation DEPLOYED + ENABLED at boot**: `shannon-freq-caps.service` (oneshot, `multi-user.target`, before docker + shannon-display). Sourced from SSoT at `~/dotfiles/system/shannon/usr/local/sbin/shannon-apply-freq-caps` + `~/dotfiles/system/shannon/etc/systemd/system/shannon-freq-caps.service`. Caps survive reboot via systemd. Net cost: ~50% peak performance for IoT-hub + kiosk stability.

**Hardware-side improvements still open** (physical access needed, none blocking):
- Verify wall-wart PSU rating vs RK3399 peak demand (~3 A @ 5 V)
- Clip-on 30 mm fan over the heatsink (~$5 USB-powered)
- Inspect thermal compound under existing heatsink (may be dried)
- Larger heatsink

**Implications**:
- The **`Workload policy — DO NOT BUILD ON SHANNON`** rule below was forged for the **SD-rootfs era**; Demeter USB rootfs is fine for apt installs and Rust compiles (heavy-write policy lifted by user 2026-05-20). The constraint that NOW matters is **peak frequency under sustained load** — cross-compilation is still a workload-throughput win but no longer a stability necessity. Update accordingly when next iterating that policy.
- **Phase 2** is no longer "VALIDATED at full freq" — it's "stable under freq caps". Use the freq-cap config as the baseline for Phase 3 work.
- **`systemctl enable shannon-display.service`** is now a smaller risk under caps. Still want a 1+ hour stability soak before flipping it on, but the wedge mode is mitigated.
- **Slice 4 (patched mpv `--hwdec=drm` 1080p H.264)** needs explicit validation at GPU=400 MHz cap. If decode requires more GPU headroom, the higher-sustainable-freq search (1.2 / 1.416 GHz candidates) becomes a Slice-4 prerequisite.

**Forensic tooling forged this session** (reusable for future wedge investigations):
- `fb0-capture` (Shannon-side `/usr/local/sbin/fb0-capture`) — atomic `dd /dev/fb0` to SD-shadow + sidecar `.meta`; survives wedge via `/var/log.hdd` + sync
- `fb0-convert` (Mac-side `~/.local/bin/fb0-convert`) — BGRA8888 → PNG via Pillow; reads sidecar or filename-encoded dims
- `shannon-gpu-telemetry` extended to dump `/proc/interrupts` per-snapshot (10s cadence) — survives wedge via SD-shadow + sync; captures pre-wedge IRQ rates for post-hoc delta analysis

### ✅ May 17, 2026 — PHASE 2 VALIDATED (watchdog-confound proven by experiment; BLOCKER below is SUPERSEDED) — *2026-05-20 caveat: this "VALIDATED" verdict was UPHELD for short runs but OVERTURNED at length. The watchdog confound was real; the kiosk wedge has additional causes that May-17's 5-6 min sample size couldn't detect. See the May-20 section above.*

**Confound-free bifurcation re-test EXECUTED May 17 ~23:00 CEST → the BLOCKER writeup below is RESOLVED.** With both confounds removed (watchdog observability-only + Demeter USB rootfs), the *exact* May-13 binaries ran rock-stable: minimal Bevy core **6+ min**, full Phase-2 retro menu **5+ min** — uptime monotonic (no reboot), PSI cpu/io ~0.00, `SYSRQ FIRED`=0, HA=200 throughout, Mali-T860 Panfrost adapter clean. The "deep-wedges in 19-27 s, 4+ times" blocker was the `shannon-watchdog` SYSRQ footgun (triple-checked → proven twice). The genuine-Mali residual is **refuted by experiment**. **The escalation ladder (SDL2 / kernel-6.12 / Mesa-25.3.2 / drop-cage / vendor-blob) is MOOTED — do NOT execute it.** Bevy 0.14 + vendored wgpu-hal-0.21.1-mali-fix + cage + Mesa-Panfrost-25.0.7 is viable as-is. Only residue: a ≥1 h soak before `systemctl enable shannon-display.service`. Canonical record: `~/dotfiles/docs/shannon_bedroom_kiosk_plan_2026_05_06.md` Implementation log "May 17 ~23:00 — bifurcation test EXECUTED".

### 🔄 May 17, 2026 — watchdog-confound reconciliation (the pre-test analysis that predicted the above — kept for the reasoning trail)

The "🟥 BLOCKER" writeup below is **preserved but now known to be watchdog-SYSRQ-confounded** (triple-checked May 17). `shannon-watchdog` (the SYSRQ-force-panic footgun, root-caused for the boot-loop saga in `~/dotfiles/docs/demeter_excavation_2026_05_14.md` § ROOT-CAUSE CORRECTION) was **also firing on the May-13 kiosk night itself** — `shannon-watchdog.log.2.gz`: `SYSRQ FIRED` at 2026-05-13T23:51:14 and 23:55:01 (box healthy, load <0.7, ~3.6 GB free), plus continuous `FAIL n/3 — HA_unreachable` all evening. The "no kernel panic / cliff-edge / pstore empty" symptom below was a survivor-logging illusion (zram `/var/log` dies with the panic; only the sync-after-write watchdog log survived). **Both historical confounds are eliminated as of May 17**: watchdog is observability-only (SYSRQ-panic deleted) + rootfs is on Demeter USB (SD-I/O class gone). **Honest caveat: this does NOT prove there's no genuine sub-90 s Mali/wgpu deadlock underneath** (minimal-Bevy "19-27 s" < the ~90 s watchdog window) — the residual is *unknown, not refuted*; the confound destroyed the clean signal. Full reconciliation: `~/dotfiles/docs/bedroom_kiosk_gpu_research_2026_05_06.md` § G2 → "May 17, 2026 — watchdog-confound reconciliation". **The escalation ladder in "Next-session focus" was premised on confounded data — run the confound-free bifurcation re-test FIRST.**

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

**0. ✅ DONE May 17 — confound-free bifurcation re-test EXECUTED → Phase 2 GO** (minimal 6+ min + full menu 5+ min, rock-stable, zero SYSRQ). **Items 1-5 below are MOOTED — do NOT execute** (they were premised on confounded data). Kept only as the superseded analysis trail.

**NOW THE FOCUS — Phase 3** (action-handler daemon, the real next deliverable):
- Rust + axum localhost daemon at `127.0.0.1:8080`, separate binary (NOT in the Bevy process). Endpoints: `/launch/retroarch?core=&rom=`, `/lights/<group>/<action>` (proxy HA REST), `/launch/chromium?url=` (streaming).
- Wire the already-installed gaming stack (RetroArch + 6 libretro cores + xpadneo + mrboom, done May 6) into the menu's 🎮 Games submenu.
- Bevy menu POSTs to the daemon; on child-process exit, Bevy regains focus (cage stays the compositor).
- **Gate before kiosk auto-start** (`shannon-display.service` stays `disabled`): one ≥1 h unattended soak (overnight, bedroom display unused) confirming multi-hour stability. 5-6 min proved the confound; only a long soak proves 24/7. This is the lone Fleet-gated residue.

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
