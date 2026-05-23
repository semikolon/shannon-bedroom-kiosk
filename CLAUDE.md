# shannon-bedroom-kiosk

Bevy 0.18 retro UI for Shannon's bedroom HDMI display. Xbox-controller-driven menu surfacing lights / music / Shannon-as-spela-target / sensors / bus departures.

**Canonical plan**: `~/dotfiles/docs/shannon_bedroom_kiosk_plan_2026_05_06.md` — vision, phased roadmap, risk register, implementation log. Latest authoritative phase-detail hub: `~/dotfiles/docs/shannon_kiosk_phase3a_display_power_engine_design_2026_05_19.md` (Phase 3 design + § 13 changelog of every Phase-3 wiring).

**Phase status (2026-05-23)**: Phase 3 LIVE on Shannon (4 of 6 tiles wired with real actions: Lights, Music, Watch, Sleep). Phase-7 spela-on-Shannon keystone shipped — Watch tile dispatches to Shannon's local spela-thin-client + ALSO integrated as a "Shannon TV" target in spela's web remote.

## Architecture

`main.rs` runs Bevy 0.18 with `WinitSettings::Reactive { wait: 10ms }` + bevy_gilrs gamepad input. Cross-compiled to aarch64 via cross-rs `:edge` on Darwin (NOT on Shannon — heavy native builds wedged Shannon in May 2026). Deployed as `/usr/local/bin/shannon-kiosk`, launched under `cage` (Wayland compositor + rootless Xwayland) via `shannon-display.service` MODE=`shannon-kiosk` (mode script at `dotfiles/system/shannon/etc/shannon-modes/shannon-kiosk.sh`).

Action daemon is a separate binary `shannon-kiosk-actions` bound to `0.0.0.0:8080` since 2026-05-23 (LAN-accessible so Darwin spela can dispatch Watch sessions; was `127.0.0.1:8080` previously). Bevy POSTs to it for HA actions; spela POSTs to it for `target=shannon`. Override via env `SHANNON_KIOSK_ACTIONS_BIND` to tighten.

## Tile state (the menu surface)

Six tiles + two submenus. The South-arm (A) match in `gamepad_event_system`:

| Tile | A-button action | State |
|------|----------------|-------|
| **Lights** | Opens `LightsSubmenu` (bedroom/office/hallway). A on group → daemon `/lights/<group>/toggle` → HA toggle. X-button = global toggle bedroom+office (`X_ALL_TOGGLE_GROUPS`). | ✅ LIVE |
| **Music** | Opens `MusicSubmenu` (play_pause / previous / next). A on action → daemon `/media/default/<action>` → HA `media_player.fredrik` (spotifyd). No-op when Spotify Connect is idle. | ✅ LIVE; needs Spotify active |
| **Watch** | `try_dispatch_watch(WATCH_SMOKE_TITLE)` → daemon `/watch` → spawns `spela-local` → HLS pipeline. **Currently hardcodes one smoke title.** Fredrik's direction (verbatim 2026-05-22): *"Watch should basically behave like spela's new web UI that I coded with Claude the last week or so, read up in git history and docs on that, basically I wanna mirror/copy it, ideally without causing too much duplication - BUT it should also run snappy on Shannon. So don't fall for temptation to just display spela-web there unless it's really quite performant. I don't want a laggy UI on Shannon. Out of the question."* So: NATIVE Bevy mirror of the spela web remote (search + library grid + now-playing + scrubber), NOT a webview embed. The web-remote source-of-truth lives at `~/Projects/spela/static/remote.html` + spela's `/search`, `/library`, `/library/list`, `/status`, `/api/position`, `/hls/master.m3u8` endpoints. | 🟡 keystone wired, UI scaffolding pending |
| **Sleep** | Engine ForceOff (TV blackout via BlackoutTvPower) + `try_dispatch_lights_multi(X_ALL_TOGGLE_GROUPS, "off")`. Hallway stays presence-driven (not in the set). | ✅ LIVE |
| **Sensors** | Half opacity (passive). Preview pane mirrors Sarpetorp dashboard's WoodStoveWidget when cursor is on Sensors (indoor temp + evening prediction + sparkline). Polls `http://sarpetorp.home/data/*` on a background tokio task. | ✅ polling shipped; live-verification on TV pending |
| **Buses** | Full opacity (real data). Preview pane mirrors BusWidget — northbound (Björkvik) + southbound (Nyköping) departures with "leave now" urgency. Same polling backend. | ✅ polling shipped; live-verification on TV pending |
| **Games** | Half opacity (not-ready). Deferred — needs cage process-model design (single-client compositor + RetroArch swap) per design hub § 13.29. | ⏸ deferred |

**Idle/sleep state — CRITICAL for future agents** (Codex misdiagnosed this 2026-05-22 and burned hours on phantom Bevy windowing bugs): when the kiosk is in its Off/blackout DisplayState, the screen shows the cage clear color, sometimes with just the Xwayland cursor visible. **This is intentional** — controller input wakes it. Don't tear down render pipelines chasing a "black screen" that's actually idle. If `menu_render: level=Root cursor=...` shows in logs, Bevy IS drawing; the kiosk is just in sleep. Fresh controller input (any D-pad / button) wakes via `gamepad_event_system::fresh_controller_input`. Fredrik's verbatim diagnosis (2026-05-23, after Codex's misdiagnosis arc): *"Isnt this because everything is supposed to be hidden until the xbox controller has activated the menu...?"* — the user knew the system better than the agent did. **Lesson**: when the user's stated mental model contradicts an agent's "render-pipeline broken" hypothesis, test against the user's model FIRST before tearing down infrastructure. Cross-ref: 2026-05-22/23 Codex session in the design hub § 13.x.

## Phase-7 spela-thin-client (Watch tile + Shannon TV web target)

Watch tile → daemon POST `/watch {"title": ...}` → spawns `spela-local <title>` (shell at `~/dotfiles/system/shannon/usr/local/bin/spela-local`) → spela-local does `/search` + `/play target=vlc` against Darwin spela → fetches HLS + decodes locally. Two-path renderer in spela-local (2026-05-23):

- **Path A (HW)**: `fdsrc + tsdemux + h264parse + v4l2slh264dec + videoconvert + kmssink`. Engages `rkvdec` at `/dev/video2` for stateless H.264 decode. Verified at file level 2026-05-21 (~25% CPU spread + V4L2-request allocator activity); REGRESSED 2026-05-23 — same pipeline EOSes in <1s with zero allocator activity, cause unknown. First probe: Shannon reboot to reset rkvdec state.
- **Path B (SW fallback)**: `playbin3 + kmssink + autoaudiosink`. Plays reliably with audio but SW-decodes (97% one core on 1.008 GHz cap, visible stutter on busy scenes).

`spela-local` tries A first with an 8s preroll budget; if A dies fast it falls back to B. Full HW-decode reasoning + GStreamer rank-syntax gotchas + MODE-drift lesson: `~/dotfiles/docs/shannon_kiosk_gpu_hwaccel_research_2026_05_18.md` § 6.

**Spela web remote integration**: target picker now shows "Shannon TV" alongside "This phone" and Chromecasts. `spela /play target=shannon` POSTs to `http://192.168.4.30:8080/watch` (override via `shannon_watch_url` in spela config). Shannon's spela-local does its own /search + /play loop. **Known edge** (open loop): target=shannon currently passes ONLY the title, not the user's `result_id` choice — Shannon's own /search might pick a different torrent than the one the user clicked Watch on. Fix: thread result_id through /watch body + have spela-local accept `--result-id N`.

## Build & Deploy

**Build (cross-compile on Darwin)**:
```bash
# 1. Sync source to Darwin (excludes target/ + .git/)
rsync -av --exclude='target/' --exclude='.git/' \
  ~/Projects/shannon-bedroom-kiosk/ \
  darwin:~/shannon-kiosk-build/shannon-bedroom-kiosk/

# 2. Cross-compile (Bevy 0.18: ~10-15m first build inside Docker, ~30s-2m incremental)
ssh darwin "cd ~/shannon-kiosk-build/shannon-bedroom-kiosk && \
  PKG_CONFIG_ALLOW_CROSS=1 ~/.cargo/bin/cross build \
    --target aarch64-unknown-linux-gnu --release \
    --bin shannon-kiosk"

# NOTE 2026-05-20: dropped the documented `--config build.rustc-wrapper='""'`
# override. Darwin's ~/.cargo/config.toml intentionally does NOT set
# build.rustc-wrapper as Cargo config — sccache is provided via RUSTC_WRAPPER
# env var only (host env not forwarded into cross-rs's Docker container,
# which lacks sccache). The override broke cargo's TOML parse on Darwin.
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

### 🎨 May 20, 2026 — Slice 3a + 3b + 3c (steps 1+2) SHIPPED (autonomous-work session), NOT deployed

**Canonical hub**: `~/dotfiles/docs/shannon_kiosk_phase3a_display_power_engine_design_2026_05_19.md` § 13 (exhaustive). § 13.11 slice table + § 13.14 asset inventory + § 13.15 implementation findings re-trued same session.

**4 commits to master** (engine + assets + Bevy refactor + bg/preview pane):
- `c79e201` Slice 3a — `KioskHint` engine extension (cursor + ribbon, 11 new tests, 32 total green)
- `a56f801` Slice 3b prep — Lucide font v1.16.0 (MIT, 824 KB)
- `94ff1fd` Slice 3c step 1 — drop pixelated; Sarpetorp forest palette + Sharp Sans + Lucide + 6-tile menu + engine integration + Y=ForceOff
- `5117b82` Slice 3c step 2 — bg image cover-fit at 20% + cursor-driven preview pane (huge Lucide icon + big label + dim subtitle per tile)

## Slices 3d-3g + Bevy 0.18 + Mali HW-GLES — architectural gates for future work

Current stack on master: **Bevy 0.18.1 + wgpu 27 + vendored `wgpu-hal-27.0.4-mali-fix/` + Slices 3a–3g all in place + daemon running on Shannon**. Live-verified end-to-end (Mali-T860 Panfrost OpenGL ES 3.1 Mesa 25.0.7, daemon polling HA `last_poll_result: "ok"`, engine state machine transitions Off↔Ambient↔Kiosk on controller presence/idle). Future-session work touching this stack must preserve the load-bearing gates below.

### Critical gates (preserve, don't accidentally regress)

**`wgpu/gles` feature flag** — `Cargo.toml`'s direct `wgpu = { version = "27", default-features = false, features = ["gles", "wgsl"] }` dep is what enables the GL backend on native Linux. Bevy 0.18's top-level `webgl2` maps to `wgpu/webgl` (WASM-only), NOT `wgpu/gles`. Removing this direct dep silently kills Mali HW-GLES on Shannon — `request_adapter` returns `None` in microseconds with zero `wgpu_hal::gles` traces, identical signature to "wgpu-hal init failure" but actually missing-feature-compile. Full root-cause arc: `~/dotfiles/docs/shannon_kiosk_phase3a_display_power_engine_design_2026_05_19.md` § 13.24 + `bedroom_kiosk_gpu_research_2026_05_06.md` § H.

**Cursor pre-positioning gates on engine state-transition INTO Kiosk** — `EngineRes.was_in_kiosk: bool` flag (`src/main.rs`) is checked in `engine_tick_system`; the predicted cursor from `engine.hint_with_offer(...).cursor` is applied ONLY on the tick that transitions Kiosk-from-elsewhere. Earlier versions applied it on every non-fresh frame, snapping the user's D-pad nav back to the morning-prediction tile (Lights) within ~16 ms — visible as "cursor resets to the start every few seconds." Fix commit `73499b4`; design-intent reference: design hub § 13.7 ("stable frame, context-filled" — pre-position on RETURN, not on every idle frame).

**Mali 4096 texture-dimension limit** — `WgpuLimits::downlevel_webgl2_defaults()` floor + `max_texture_dimension_2d=4096` cap in `src/main.rs`. Background asset `assets/backgrounds/sarpetorp-clock-bg.jpg` is sized 2506×4000 specifically to fit; the source from `~/Projects/sarpetorp/dashboard/public/clock-bg.jpg` is 3320×5299 and would panic `Device::create_texture: Dimension Y value 5299 exceeds the limit of 4096` if re-imported without resizing. Use macOS `sips -Z 4000 <src> --out <dst>` if re-sourcing.

**`BlackoutTvPower` preserves Argon DA2 keepalive** — `Off` state paints the screen black via the `BlackoutOverlay` NodeBundle (`GlobalZIndex(500)`) rather than cutting HDMI signal. This is INTENTIONAL: the bedroom Argon amp's EU-EcoDesign auto-standby fires after ~10–20 min of HDMI-source silence; cutting the signal would force a 10-min cold-start the next time any audio (Ruby narration, music, spotifyd) plays. The opt-in `HdmiSignalTvPower` (`wlr-randr --output HDMI-A-1 --off`) and eventual `HaSmartPlugTvPower` paths remain alternatives for hosts without the Argon constraint. Design hub § 13.6.

**HA polling lives in the daemon, not the kiosk** — `shannon-kiosk-actions` (binary at `/usr/local/bin/shannon-kiosk-actions` on Shannon, systemd unit `shannon-kiosk-actions.service`) is the only process that holds `HA_TOKEN`. The Bevy kiosk polls the daemon's `/ha-state` endpoint via `std::thread + reqwest::blocking` at 3 s cadence; the daemon polls HA every 30 s with exponential backoff up to 5 min on failure. The kiosk never talks to HA directly. `EnvironmentFile=-/etc/default/shannon-kiosk-actions` (mode 600 root) holds `HA_TOKEN` + entity-id overrides; deployed value sourced from `~/.secrets/tier-all.env` key `HA_TOKEN_SHANNON`.

### Deferred architectural decisions (don't re-litigate without reading)

**Ribbon-action wiring — Chromecast vs Shannon spela-launch** (Slice 3e ships the OFFER text; pressing [A] is currently a no-op that falls through to the menu's default A=select). Three options on the table:
- **(a) HA `media_play` on Chromecast** — daemon POSTs `/api/services/media_player/media_play` with `entity_id: media_player.fredriks_tv`. The Chromecast resumes; the HDMI switch's auto-priority routing detects the active input + flips to that source. Simple, depends only on HA + existing Chromecast.
- **(b) Launch `spela-local` on Shannon** — needs Slice 4's patched mpv landed first + a content-resolver mapping HA's reported title → spela magnet/file. Shannon's HDMI output becomes active; switch flips to Shannon input. More moving parts.
- **(c) Both via different buttons** — A = Chromecast resume, X = play-on-Shannon. Defer until real-world taste decides.

Fredrik's load-bearing intuition (verbatim 2026-05-21): *"either the Chromecast is playing thru the HDMI switch or the Shannon is. Pressing A to resume mostly makes sense for spela-on-Shannon and won't bring the Chromecast up? Or maybe it will because the HDMI Switch senses activity on that HDMI stream?"* — the **HDMI switch's auto-priority signal-detection** behavior is what makes option (a) viable without explicit input-select control. Defer the decision until a real Chromecast-paused-media taste-test surfaces preference. Design hub § 13.27 row 3.

**Kiosk → daemon action wiring is NOT yet implemented** — pressing [A] on Lights / Music / Watch / Sensors / Sleep tiles currently logs only; no actuation. The daemon's `/lights/:group/:action` (Slice 2) is reachable for direct REST; wiring the kiosk's `gamepad_event_system` `GamepadButton::South` arm to POST when `engine_res.cursor == MenuItem::Lights` is ~15 lines + the cheapest first hookup. The engine's `Action::SetTvPower(_)` is already routed locally to `BlackoutTvPower` (`engine_tick_system`); routing it ALSO to the daemon for smart-plug actuation is a separate ~10-line addition (`reqwest::blocking::post` to a new daemon `/tv-power` endpoint or piggyback `/signal`).

### Open items for next session

| # | Item | Notes |
|---|---|---|
| 1 | **Input-to-render latency ~1 s** | Fredrik's fresh-eyes pick. First diagnostic: `RUST_LOG=bevy_render=info,bevy_winit=info` + check whether button events fire same-tick as emission. Suspects: `WinitSettings::Reactive { wait: 33 ms }` 30fps cap, gilrs polling cadence, render-system change-detection lag, cage/Xwayland event propagation. |
| 2 | **Lights tile → daemon `/lights/:group/:action`** | Cheapest first hookup for actual menu actuation. |
| 3 | **Y=ForceOff → daemon smart-plug** (or stay local-only via BlackoutTvPower) | Decide whether engine-emitted `Action::SetTvPower(_)` ALSO sends a daemon POST. Current behavior is local-blackout only (preserves Argon keepalive). |
| 4 | **Ribbon-action wiring decision** | See deferred section above. |
| 5 | **3 am artificial stress-test for 03:00 hard-hang** | Reduces wall-clock to confirm flaky-telemetry-cron mitigation. Methodology open. |
| 6 | **Multi-hour soak + `systemctl enable shannon-display.service`** | Currently intentional manual-launch per critical-constraint #5. Flip when many-nights-stable. |
| 7 | **Slice 3c step 3 — formal Xbox-styled colored chips** | Bevy 0.18+ ships UI `border_radius`; unblocked. Current text-only chrome ("A SELECT  B BACK  Y ALL OFF") works fine — cosmetic polish only. |
| 8 | **Display fit on ultrawide** | `WindowResolution::new(1920, 1080)` is smaller than 3440×1440 dev monitor → right ~1/4 shows TTY framebuffer. Bedroom production is 1080p — fits perfectly there. |

### Commit reference (for git archaeology when needed)

Slice 3d–3g + merge + cursor fix sit on master: `01b257f` (3d daemon HA polling, new `src/ha_poll.rs`, 10 unit tests), `10d8da8` (3e ribbon-offer + Bevy poller + `Engine::hint_with_offer(...)` API, 6 unit tests, `Media: Default` derive, reqwest `blocking` feature), `bb77d38` (3f `BlackoutTvPower` + `BlackoutOverlay` + `blackout_render_system`, 4 unit tests), `27e2f23` (3g `AmbientCanvas` + `ambient_render_system`), `0e42b67` (jpeg + zune-core pin + bg jpg resize), `2e00198` (merge bevy-upgrade → master, conflicts resolved: Cargo.toml combine + Bevy 0.18 API translations applied to Slice 3d-3g new code), `73499b4` (cursor snap-back fix via `was_in_kiosk` state-transition gate). 62 lib + bin tests total, all green. Full chronology in design hub `§§ 13.23–13.27`.

**Try it locally**: `cd ~/Projects/shannon-bedroom-kiosk && cargo run --bin shannon-kiosk`. With an Xbox controller paired over BT: D-pad / left stick to navigate, A select, B back, Y all-off. Without controller: the menu still renders (engine reports Off but menu UI is always-spawned at startup); a keyboard fallback for dev-without-Xbox is a candidate follow-up.

Highlights:

- **Menu**: Games / Music / Lights / Watch / Sensors / Sleep (six tiles; Sleep replaces the design-doc Settings per user 2026-05-20 — Sleep + engine `ForceOff` beats config-tile-nobody-uses).
- **Layout**: TLOU-style vertical list left + cursor-driven preview pane right + controller chrome bottom. **No display title** ("let the other stuff take focus").
- **Typography**: Sharp Sans primary (`assets/fonts/SharpSans-{Regular,Semibold,Bold}.otf`, copied from `~/Library/Fonts/`, commercial license, gitignored). Manrope alternative (`assets/fonts/Manrope-Variable.ttf`, OFL, free to commit). *Correction recorded for posterity in design hub § 13.3: the Sarpetorp dashboard's big "SARPETORP" header is Manrope ExtraBold 0.12em tracking in `ClockWidget.tsx`, not Horsemen — Horsemen.otf is bundled in `dashboard/public/fonts/` but unused; my earlier guess from the filename was wrong, Fredrik caught it.*
- **Palette + bg**: Sarpetorp forest theme (dark forest radial-gradient base + oat-milk cream text + warm-cream/amber accent). Background = `assets/backgrounds/sarpetorp-clock-bg.jpg`, copied from `~/Projects/sarpetorp/dashboard/public/clock-bg.jpg` (the Sarpetorp dashboard's top-widget bg).
- **Icons**: Lucide font (pending download), monochrome single-accent discipline (all icons render in the same cream/amber, no rainbow tinting — per Image #10).
- **TV-off actuator**: `BlackoutTvPower` default (renders black Bevy scene, preserves Argon DA2 keepalive). Opt-in `HdmiSignalTvPower` (`wlr-randr --output HDMI-A-1 --off`, mirrors Sarpetorp's `xset dpms` pattern via `~/Projects/sarpetorp/handlers/display_control_handler.rb`). `HaSmartPlugTvPower` eventual production path.
- **Y button = ALL OFF** instant shortcut. A select, B back, X reserved.
- **Engine ext (Slice 3a, pending)**: `KioskHint { cursor: Option<MenuItem>, ribbon: Option<RibbonOffer> }` via `Engine::hint(&Inputs)`. Cursor prediction heuristic: hard-off→Sleep, music-playing→Music, winddown→Watch, morning→Lights, else→None (renderer falls back to Watch). Ribbon stays silent until Slice 3e wires resume-last-watched via daemon HA polling.
- **Shannon-wide design language**: this visual system applies to Kiosk + Ambient + Off + any future surface.

**Assets in `assets/`** (as of 2026-05-20):
- `backgrounds/sarpetorp-clock-bg.jpg` — Sarpetorp top-widget bg (4 MB)
- `fonts/SharpSans-{Regular,Semibold,Bold}.otf` — primary (commercial, gitignored, copy from `~/Library/Fonts/`)
- `fonts/Manrope-Variable.ttf` — alternative (OFL, committable)
- `fonts/PressStart2P-Regular.ttf` — legacy Phase-2; remove when main.rs refactors
- Pending: Lucide icon font (.ttf)

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

**Unified (post-merge 2026-05-21)**: Bevy 0.18.1 (wgpu 27.0.1) + Slices 3a-3g — LIVE-VERIFIED on Shannon Mali T860 Panfrost OpenGL ES 3.1 Mesa 25.0.7-2 backend Gl. Uses vendored `wgpu-hal-27.0.4-mali-fix/` (3 patches: X11 Wayland-veto + `WGPU_GL_PREFER_GLES` force_gles bind_api + robustness retry extended to `BadParameter` for Mali Midgard V5) + direct `wgpu = { version = "27", default-features = false, features = ["gles", "wgsl"] }` dep in Cargo.toml + `jpeg` Bevy feature + `zune-core = "=0.5.0-rc2"` pin + bg jpg resized to 2506×4000 (Mali 4096 texture limit).

**THE actual root cause** of the Bevy 0.18 Mali blocker (after 3 wgpu-hal patches + Mesa env vars all hit the same `eglQueryDeviceStringEXT BAD_PARAMETER`): `bevy_render-0.18.1`'s wgpu dep is `default-features=false, features=["wgsl","dx12","metal","vulkan","naga-ir","fragile-send-sync-non-atomic-wasm"]` — **`gles` is NOT in the list**. Bevy 0.18's top-level `webgl2` feature maps to `wgpu/webgl` (WASM-only), NOT `wgpu/gles` (native Linux). Fix: direct `wgpu` dep with `gles` feature activates it on bevy_render's wgpu dep too via Cargo's feature-unification. The three wgpu-hal vendor patches kept as defense-in-depth — they live inside the GL backend code that didn't exist without the feature.

Master rollback path: pre-merge state preserved at git tag/SHA `d79f039`.

Full Bevy-upgrade arc: `~/dotfiles/docs/shannon_kiosk_phase3a_display_power_engine_design_2026_05_19.md` §§ 13.16-13.25 + verbatim subagent research at `~/dotfiles/docs/shannon_kiosk_bevy_upgrade_mali_research_2026_05_20.md` (preserved as a symptom-vs-cause lesson — the subagents reasoned correctly about wgpu-hal-27 EGL but missed the feature-compile layer below) + generalizable insight mirrored to `~/dotfiles/docs/bedroom_kiosk_gpu_research_2026_05_06.md` § H.

If we hit Bevy resource-use ceilings on Shannon, fallback per kiosk plan § "Why Bevy specifically" is `egui + winit` — lighter, but more glue work for retro shaders.
