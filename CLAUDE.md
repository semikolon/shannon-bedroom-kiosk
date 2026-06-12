# shannon-bedroom-kiosk

Bevy 0.18 retro UI for Shannon's bedroom HDMI display. Xbox-controller-driven menu surfacing lights / music / Shannon-as-spela-target / sensors / bus departures.

**⚠ 2026-06-12 status — DORMANT pending future bedroom monitor**: Lukas took the bedroom TV to his Sparreholm summer house. No replacement imminent (gates on Fredrik's income from work not yet landed, ~3-4 months horizon). `shannon-display.service` STOPPED + DISABLED 2026-06-12 (cage was running with no HDMI sink). All Phase 1–7 work that requires a display sits dormant on disk: Watch UI Phases 1–5 + scrubber + voice search, shannon-games + STK + RetroArch + Xbox controller cage handoff, spela-renderer v5 + v4 fallback (the HW-decode 99% → 33-38% sustained CPU solution shipped + production-deployed), shannon-kmsgrab, global TV-power hotkey daemon (still wired but the smart plug powers nothing visible), Plymouth + transparent xcursor + Jun 8-9 UI cleanup wave. Revival is hours-not-days when any monitor returns. Shannon's primary role shifts to IoT hub + bedroom voice endpoint + audio source + ambient sensor poller. Values lens worth continually revisiting consciously: `~/.claude/CLAUDE.md` § "Bedroom screentime reframe". Cross-refs: `~/dotfiles/CLAUDE.md` § Shannon Phase 9 → "2026-06-12 reorient" + `~/dotfiles/docs/personal_iot.md` § Bedroom kiosk. Sections below describe the kiosk surfaces as they EXIST on disk — treat as the to-revive specification, not the current operational state.

**Canonical plan**: `~/dotfiles/docs/shannon_bedroom_kiosk_plan_2026_05_06.md` — vision, phased roadmap, risk register, implementation log. Latest authoritative phase-detail hub: `~/dotfiles/docs/shannon_kiosk_phase3a_display_power_engine_design_2026_05_19.md` (Phase 3 design + § 13 changelog of every Phase-3 wiring).

**Global Xbox controller TV-power toggle** (2026-06-09 night): `shannon-global-hotkey.service` runs INDEPENDENTLY of kiosk/games/playback. Watches Xbox controller events non-exclusively; on BTN_SELECT + BTN_START held simultaneously for 2 s, fires POST `/tv-power-toggle` on this daemon → HA `switch.toggle switch.bedroom_tv_plug_bedroom_tv_plug`. Works regardless of which process owns the screen. Spec: `~/dotfiles/docs/global_tv_hotkey_design_2026_06_09.md`. Daemon source: `~/dotfiles/system/shannon/usr/local/bin/shannon-global-hotkey`.

**Phase status (2026-06-09)**: Phase 3 LIVE (Lights / Music / Watch / Sleep / Sensors / Buses tiles wired). Phase-6 Games tile end-to-end-verified 2026-06-08 (route + binaries + RetroArch Xbox autoconfig + STK cgroup escape). Phase-7 spela-on-Shannon keystone shipped (Watch tile → Shannon spela-local + "Shannon TV" web-remote target); **spela-renderer pad-link bug fixed 2026-06-09 (commit `dbcd8ca`) so v5 is now THE primary playback path** (was always silently falling back to v4 gst-launch). **Watch UI Phases 1, 1.5, 2A, 2B, 3, 4 LIVE** (2026-05-29 batch added Phase 4 scrubber visual + spela-renderer IPC + DPad-LR/X bindings); **Phase 5 (voice search) IS THE LAST GAP** — scoped per the broader brain-endpoint plan, build is in-progress per Fredrik 2026-05-29 *"build voice now please"* + reactive-only direction (see `~/dotfiles/docs/brain_endpoint_phase1_design_2026_05_04.md` § Reactive-only ship + `~/dotfiles/docs/shannon_watch_ui_design_2026_05_24.md`). **2026-06-09 UI cleanup wave**: state-badge removed, menu+chrome shifted up 30 px, poster-grid `is_changed()` change-detection bug fixed (posters now actually load), WatchPendingOverlay sidebar overlap fixed + auto-hides on NowPlaying, Y-button reverted to design-intent engine-only ForceOff per May-21 `4d60ab1`. Cursor hidden via udev `LIBINPUT_IGNORE_DEVICE=1` on xpadneo's stick→pointer + ydotoold virtual mouse.

**Controller-during-playback architecture** (2026-05-29, closed the deferred Phase-4 follow-up): spela-renderer opens its own `gilrs::Gilrs` instance during playback and applies controller commands directly to the GStreamer pipeline — no Unix-socket round-trip, no kiosk-overlay-on-playback compositor work, no separate evdev daemon. The renderer that owns the display also owns the input. Button map: DPad-Left = seek -30 s, DPad-Right = seek +30 s, X (West) = restart from start, A (South) = play/pause, B (East) = quit-back-to-kiosk. Kiosk's Phase-4 Bevy bindings stay live for the cold-start window only; once spela-renderer starts, the kiosk gilrs and renderer gilrs naturally hand off. Same pattern works whenever a future process owns the display during a non-kiosk activity (games, etc.). Source: `src/bin/spela-renderer.rs::spawn_controller_listener` + `src/spela_control_proto.rs`.

**HW-decode current state (2026-06-09)**: spela-renderer (Rust gstreamer-rs replacement for the gst-launch shell pipeline) is **NOW the primary path** via `spela-local` v5 fallback chain — was silently failing 2026-05-29 → 2026-06-09 with `build_pipeline` returning Err on `hlsdemux.link(&tsdemux)` (dynamic-pad element static-link failure); spela-local kept falling back to v4 gst-launch (the prior 35-38% CPU empirical). Fixed by `connect_pad_added` (commit `dbcd8ca`) so hlsdemux's source pad links to tsdemux's sink when it materializes. Live-verified 2026-06-09 20s smoke test: PLAYING reached, control socket opened, controller listener spawned, ran clean to SIGTERM-timeout. 2026-05-29 tuning preserved: time-based 10 s queue + curlhttpsrc (libcurl HTTP source, better keep-alive than libsoup). **Sustained CPU measurement at TV pending** (renderer now works; verify the 33% target). Werner's 15 % CPU floor on Bookworm+Kwiboo mpv is a DIFFERENT stack; the target within our GStreamer-on-Trixie constraints is **as-low-as-makes-the-experience-feel-optimal**, not specifically 15 %. Detail in `~/dotfiles/docs/2026-05-25-rk3399-hw-decode-canonical-survey.md` § H.6.

**Lights latency target met** (~360-830 ms kiosk-button → physical lamp via daemon HTTP path, was ~2-2.5 s subprocess path; see `~/dotfiles/system/shannon/README.md` § lights-daemon).

**Boot path (2026-05-24 update)**: Plymouth graphical splash + seamless splash→kiosk handoff LIVE — `bootlogo=true` + `console=serial` + initramfs DRM modules deploy via the dotfiles `shannon-boot-splash-apply` script (commit `98b6ace5`); `shannon-display.service` now declares `Conflicts=getty@tty1` + `After=plymouth-quit-wait.service` + `ExecStartPre=-/usr/bin/plymouth quit --retain-splash` for clean DRM-master handoff. Full architecture in `~/dotfiles/system/shannon/README.md` § "Plymouth boot splash + kiosk handoff architecture". Visual verification by Fredrik pending; technical render path confirmed via `plymouthd --debug`. The `fb0-capture` tool is busted for visual probing — modern DRM/KMS bypasses `/dev/fb0` (cage + plymouthd take DRM master directly), captures land as pure white; kmsgrab replacement tracked in dotfiles `TODO.md`.

**Watch UI architecture (2026-05-24 v1)**: new `src/watch_ui.rs` module — `LibraryEntry` + `LibrarySnapshot` + `spawn_library_poller` (background thread, polls spela `/library` every 120s with tolerant `serde_json::Value`-based parser; handles both bare-array and wrapped `{"library": [...]}` JSON shapes since spela's format has evolved; env `SPELA_BASE_URL` overrides the default `http://darwin.home:7890`). `MenuLevel::WatchSubmenu` variant with dynamic content (`submenu_for` returns `&[]` — special-cased in `gamepad_event_system` + `menu_render_system` to use `LibrarySnapshotRes` instead of static slice). Submenu slots pre-spawned at `WATCH_VISIBLE_SLOTS=10` (was 3 for Lights/Music — Lights/Music submenus only fill 3, rest stay `Visibility::Hidden`). `window_around(cursor, total)` centers the visible window on cursor when possible. A on Root.Watch enters WatchSubmenu; A on entry calls existing `try_dispatch_watch(raw_name)`; B returns to Root with cursor restored on Watch. Empty-library fallback dispatches `WATCH_SMOKE_TITLE` so the tile is never dead.

**Watch UI Legibility Pass SHIPPED 2026-05-25 (kiosk `cef7cbd`, Shannon deploy 00:40 CEST; renamed from "Phase 1.5" per *Self-decoding labels* directive — "Phase 1.5" was an anonymous handle, the new name signifies the work: spatial legibility (sidebar stays + library moves right) + temporal legibility / response affordance (STARTING overlay + 30s debounce))**:
- **Layout fix**: `menu_render_system` uses `show_root = !in_submenu || is_watch` so the sidebar stays visible during WatchSubmenu (with the Watch row highlighted via the new `root_selected_idx` independent of `selected`/`submenu_cursor`). The pre-spawned `LightsSubmenuLabel/Icon` entities get `node.left` mutated each render to `x=620/560` when is_watch (vs `x=200/130` for Lights/Music) so the library list reads as a column distinct from the sidebar. `preview_render_system` hides preview-pane icon/label/subtitle during WatchSubmenu to avoid overlap with the library list now occupying that area.
- **Immediate A-press feedback**: new `WatchPendingOverlay` (translucent backing card + "STARTING `<title>`…" label, center-screen) driven by `EngineRes.pending_watch: Option<(String, Instant)>` + new `pending_watch_overlay_system`. Set BEFORE `try_dispatch_watch` so the overlay paints immediately even if the daemon POST hangs; auto-clears after `WATCH_OVERLAY_TIMEOUT_MS = 90s` (sized to cover Darwin's worst-case NVENC cold-start ~60s + safety margin).
- **`WATCH_DEBOUNCE_MS` 2→30s** (defense in depth, paired with the overlay): closes the symptom that fired two parallel spela-local sessions on rapid A-presses. The overlay is the real fix because it removes the trigger; the debounce bump is suspenders.
- **Rollback**: if a visual regression surfaces, `ssh shannon 'mv /usr/local/bin/shannon-kiosk.bak-2026-05-25 /usr/local/bin/shannon-kiosk && systemctl restart shannon-display.service'`.

**Lights path (2026-05-24 v2)**: `shannon-kiosk-actions` daemon `lights_handler` POSTs to persistent `shannon-lights-daemon` on `127.0.0.1:8081` (was: spawned `shannon-lights` subprocess per call, ~2-2.5s; now: HTTP to warm daemon, ~360-830ms). Plan: `plan_lights(group, action)` → HA group entity → POST `http://127.0.0.1:8081/{verb}?group={entity}` with verb mapped from HA service name (turn_on/turn_off/toggle → on/off/toggle). Daemon expands group via HA REST (TTL-cached), iterates members with per-device locks (tinytuya is NOT thread-safe), retries on stale-socket 904 errors transparently. LocalTuya HA integration still silent-fails (root cause open) but every kiosk surface bypasses it now.

## Architecture

`main.rs` runs Bevy 0.18 with adaptive `WinitSettings::Reactive` (`ACTIVE_WAIT_MS=10` during navigation, `IDLE_WAIT_MS=5000` after `IDLE_THRESHOLD_SECS=3` of no input, switched per-tick by `dynamic_wait_system`) + bevy_gilrs gamepad input. **gilrs-wake bridge** — a dedicated thread owns a sidecar `gilrs::Gilrs` instance and true-blocks on `next_event_blocking(None)` (epoll/inotify in the kernel; zero CPU between events); on any gamepad event it calls `EventLoopProxy::send_event(WinitUserEvent::WakeUp)`. This decouples gamepad latency from `wait` at the root — wait can be 1000-5000ms at idle without degrading controller responsiveness. Net: idle CPU ~1.0% (vs the pre-bridge 21% at fixed 10ms wait); first-press-after-idle wakes in <5ms. See `spawn_gilrs_wake_thread` + `dynamic_wait_system` + `gilrs_wake_loop` in `main.rs`. Cross-compiled to aarch64 via cross-rs `:edge` on Darwin (NOT on Shannon — heavy native builds wedged Shannon in May 2026). Deployed as `/usr/local/bin/shannon-kiosk`, launched under `cage` (Wayland compositor + rootless Xwayland) via `shannon-display.service` MODE=`shannon-kiosk` (mode script at `dotfiles/system/shannon/etc/shannon-modes/shannon-kiosk.sh`).

Action daemon is a separate binary `shannon-kiosk-actions` bound to `0.0.0.0:8080` since 2026-05-23 (LAN-accessible so Darwin spela can dispatch Watch sessions; was `127.0.0.1:8080` previously). Bevy POSTs to it for HA actions; spela POSTs to it for `target=shannon`. Override via env `SHANNON_KIOSK_ACTIONS_BIND` to tighten.

## Tile state (the menu surface)

Six tiles + two submenus. The South-arm (A) match in `gamepad_event_system`:

| Tile | A-button action | State |
|------|----------------|-------|
| **Lights** | Opens `LightsSubmenu` (bedroom/office/hallway). A on group → daemon `/lights/<group>/toggle` → HA toggle. X-button = global toggle bedroom+office (`X_ALL_TOGGLE_GROUPS`). | ✅ LIVE |
| **Music** | A on MUSIC at Root: if music is playing or paused (HA `media_player.music` reports `playing`/`paused`), routes **directly into Music-NowPlaying** view (track title + artist + state + transport hints; A=pause/play, DPad-LR=prev/next, B=back to Root). Otherwise opens `MusicSubmenu` (play_pause / previous / next). **Wake routing on Off→Kiosk transition**: when music is active, controller wake lands on Music-NowPlaying directly (1-press pause) instead of Root — for the smart-plug-TV-off mode where the user wakes the kiosk just to pause/skip without navigating. Fredrik's voice on the wake target (2026-05-28, verbatim, approving this design): *"Yes well then wake directly into a now-playing view, sounds great."* All music actions route through daemon `/media/default/<action>` → HA `media_player.music` (operator-configured via `HA_MUSIC_PLAYER_ENTITY` env, typically the spotifyd Spotify-Connect endpoint). | ✅ LIVE 2026-05-28 |
| **Watch** | A on Watch → `MenuLevel::WatchSubmenu` showing live library (polled from spela `/library` via `watch_ui::LibrarySnapshotRes`). A on a library entry → `try_dispatch_watch_post(raw_name)` → daemon `/watch` → spawns `spela-local` → spela-local hits Darwin `/search`+`/play target=vlc` → cage+waylandsink HW-decode (33% CPU). Empty-library fallback dispatches `WATCH_SMOKE_TITLE` so Watch is never a dead button. Library entries dispatch title-only (no magnet — library taps don't have one); search results from web remote also reach this path via spela's `target=shannon` dispatch with magnet passthrough (3-layer: spela server `c6e3de6` + kiosk-actions `1cde18b` + spela-local `58c677c6`). Fredrik's direction (verbatim 2026-05-22): *"Watch should basically behave like spela's new web UI... mirror/copy it... should also run snappy on Shannon... NOT a webview embed."* NATIVE Bevy mirror of search + library grid + now-playing + scrubber. Library grid + **NowPlaying view (Phase 3)** + **single-poster preview (Phase 2A)** LIVE; search + scrubber + full poster grid (Phase 2B) PENDING. After A-press on a library entry: transition to NowPlaying view shows title + state + position polled from spela; B returns to library. Selected library entry's TMDB poster is rendered at x=1310 (right of the text list). End-to-end validated 2026-05-28 via `kiosk-inject-gamepad` (BTN_SOUTH on Aladdin → spela-local session start + Darwin /play accepted). | ✅ library grid + dispatch + NowPlaying + single-poster preview LIVE; search + scrubber + grid pending |
| **Sleep** | Engine ForceOff (TV blackout via BlackoutTvPower) + `try_dispatch_lights_multi(X_ALL_TOGGLE_GROUPS, "off")`. Hallway stays presence-driven (not in the set). | ✅ LIVE |
| **Sensors** | Half opacity (passive). Preview pane mirrors Sarpetorp dashboard's WoodStoveWidget when cursor is on Sensors (indoor temp + evening prediction + sparkline). Polls `http://sarpetorp.home/data/*` on a background tokio task. | ✅ LIVE |
| **Buses** | Full opacity (real data). Preview pane mirrors BusWidget — northbound (Björkvik) + southbound (Nyköping) departures with "leave now" urgency. Same polling backend. | ✅ LIVE |
| **Games** | A on GAMES opens `GamesSubmenu` (2 entries: `SUPERTUXKART` + `RETROARCH`). A on entry → `try_dispatch_game` → daemon `/game/<name>` → `shannon-games` subprocess. Script stops `shannon-display.service` (releases DRM master), launches game with `kmsdrm` direct rendering, restarts display via cleanup-trap on exit. RetroArch entry boots into the RGUI main menu (no `-L` core, no ROM) where the user picks cores + ROMs with the Xbox controller. `GAMES_DEBOUNCE_MS=2000` because launching is heavy. Daemon-side `/game/<name>` allowlist gates to `stk` + `retroarch` only — ROM-requiring `snes`/`gba` subcommands need a ROM-list view we haven't built. | ✅ LIVE 2026-06-06 |

## Smart-plug TV-power swap — pre-flip checklist (PENDING, post-2026-05-28-session)

Fredrik 2026-05-28 verbatim: *"Shannon is so stable these days that I want to use the smart plug it's hooked up to (which was used to power cycle it when it wedged in the past) to control the TV instead (hoping to get rid of the TV remote, as originally planned). I should do this after a safe shutdown of Shannon..."*

**Why this is good per directives**: per global *Self-referential life-support — a node must not be able to kill what keeps it alive* (anchored to the 2026-05-18 incident where HA on Shannon cut Shannon's own power for 7 h via this same plug), MOVING the plug from Shannon to TV REMOVES the self-referential trap — the plug becomes a controllable-victim instead of life-support.

**Pre-flip checklist** (in order; verify each before re-cabling):

| # | Item | Notes |
|---|---|---|
| 1 | Shannon's new power source | Wall outlet NOT on any HA-controllable plug (else the trap is moved, not removed) |
| 2 | TV auto-power-on setting | Most modern TVs default to standby on power-on. For true "no remote", flip TV's "auto power-on" / "boot from off" / "wake on HDMI signal" setting first. Otherwise plug-on → TV in standby → still need remote / CEC wake. Fredrik to verify on his TV. |
| 3 | HDMI switch independent power | Confirmed by `~/dotfiles/docs/bedroom_audio_station_2026_04_26.md`: switch → optical → Argon, switch separately powered → music plays with TV off as designed. ✓ |
| 4 | HA entity rename | Per `~/dotfiles/docs/personal_iot.md § "Naming SSoT preference"`: rename in Smart Life first (e.g., "Bedroom TV power"), HA picks up on next LocalTuya reload. The expected entity_id is something like `switch.bedroom_tv_power`. |
| 5 | Kiosk env config | Set `HA_TV_POWER_PLUG=switch.bedroom_tv_power` (exact entity_id) in `/etc/default/shannon-kiosk-actions` after the rename + reload completes. |
| 6 | Group exclusion | Per the 2026-05-18 *Self-referential life-support* anchoring + the Closet inclusion fix precedent: ensure the new TV-power plug's switch entity is EXCLUDED from `group.bedroom_lights` and every kiosk Sleep/X-toggle sweep. (Once it's not powering Shannon, the self-loop is gone, but group-hygiene is still right.) |

**Engine-side**: `Action::SetTvPower(bool)` already abstracts the actuator decision. The `PlugTvPower` actuator (Task C in `dotfiles/TODO.md § Shannon kiosk open loops post-2026-05-28`) routes that action in parallel with the existing `BlackoutTvPower` (paint-black overlay), so the visual transition is instant + the plug catches up 1-2 s later for a smooth perceptual cutover.

**Safe shutdown sequence**: `ssh shannon "systemctl stop shannon-display.service; sleep 3; systemctl poweroff"` → wait ~30 s for clean SD flush → LEDs dark → re-cable.

## Idle/sleep state — CRITICAL for future agents

**Idle/sleep state — CRITICAL for future agents** (Codex misdiagnosed this 2026-05-22 and burned hours on phantom Bevy windowing bugs): when the kiosk is in its Off/blackout DisplayState, the screen shows the cage clear color, sometimes with just the Xwayland cursor visible. **This is intentional** — controller input wakes it. Don't tear down render pipelines chasing a "black screen" that's actually idle. If `menu_render: level=Root cursor=...` shows in logs, Bevy IS drawing; the kiosk is just in sleep. Fresh controller input (any D-pad / button) wakes via `gamepad_event_system::fresh_controller_input`. **Programmatic wake without physical controller** (2026-05-28): `ssh shannon /usr/local/bin/kiosk-inject-gamepad BTN_DPAD_DOWN` injects a uinput Xbox 360 event; gilrs sees it, bridge sends WakeUp, menu renders. Then `shannon-screenshot` captures the result. Future agents diagnosing "is the kiosk dead or idle?" should run this 3-line test BEFORE tearing down render pipelines. Fredrik's verbatim diagnosis (2026-05-23, after Codex's misdiagnosis arc): *"Isnt this because everything is supposed to be hidden until the xbox controller has activated the menu...?"* — the user knew the system better than the agent did. **Lesson**: when the user's stated mental model contradicts an agent's "render-pipeline broken" hypothesis, test against the user's model FIRST before tearing down infrastructure. Cross-ref: 2026-05-22/23 Codex session in the design hub § 13.x.

## Phase-7 spela-thin-client (Watch tile + Shannon TV web target)

Watch tile → daemon POST `/watch {"title": ...}` → spawns `spela-local <title>` (shell at `~/dotfiles/system/shannon/usr/local/bin/spela-local`) → spela-local does `/search` + `/play target=vlc` against Darwin spela → fetches HLS + decodes locally.

**🎯 2026-05-26 ~00:00 CEST — HW decode CPU breakthrough**: cage + waylandsink direct-element renderer shipped at **35-38% sustained CPU** (cage ~8% + gst-launch ~27% for 1080p H.264+AAC HLS) — **65% reduction from the prior 99%**. Cause of the 99% was definitively identified: `playbin3 + capsfilter` inserts 3x software videoconvert passes (proven via GST_DEBUG_DUMP_DOT_DIR dot dump showing the autoplug chain `v4l2slh264dec → conv → scale → videobalance → conv2 → vconv → kmssink`). The video-filter capsfilter ended up DOWNSTREAM of the inserted converters, exactly the playbin3-sandwich predicted in the canonical-survey § A. Cure: `waylandsink` (under cage) natively handles DMABuf-with-VideoMeta via wl_buffer, exactly v4l2slh264dec's required negotiation contract — no converters needed. Full empirical proof + element chain + critical incantations: [`~/dotfiles/docs/2026-05-25-rk3399-hw-decode-canonical-survey.md`](~/dotfiles/docs/2026-05-25-rk3399-hw-decode-canonical-survey.md) § H.6 + [`~/dotfiles/system/shannon/README.md`](~/dotfiles/system/shannon/README.md) § "spela-local v4 — cage + waylandsink HW-decode renderer". Future ceiling toward Werner's empirical 15% would need either v4l2convert+RGA tuning (tonight's preroll-fail) or apt.undo.it Kwiboo mpv switch.

Current renderer state in spela-local (cage+waylandsink primary v4, playbin3+capsfilter+kmssink fallback, Path A v3 retained as code):

- **cage + waylandsink renderer (v4, primary, commit `854c0950`)**: 35-38% sustained CPU. Element chain `souphttpsrc ! hlsdemux ! tsdemux name=demux  demux. ! queue ! h264parse ! v4l2slh264dec ! waylandsink  demux. ! queue ! aacparse ! avdec_aac ! audioconvert ! audioresample ! alsasink` under `cage -- env -u WAYLAND_DISPLAY ...`. Critical: `env -u WAYLAND_DISPLAY` strips inherited parent-shell display so child connects to cage's compositor not whatever the parent had.
- **playbin3 + capsfilter NV12 + kmssink (fallback only)**: 99% CPU but reliable. Fallback only fires if cage fails to take DRM master or waylandsink crashes. Pre-2026-05-26 this was the primary; replaced after empirical CPU breakthrough.
- **Path A v3 (manual gst-launch + souphttpsrc+hlsdemux+v4l2slh264dec+kmssink direct)**: retained as code but not invoked — `v4l2slh264dec ! kmssink` direct fails negotiation: "DMABuf caps negotiated without the mandatory support of VideoMeta" (kmssink doesn't accept VideoMeta on DMABuf, which v4l2sl decoder marks mandatory for tiled formats).

**Cross-build deploy automation (new 2026-05-26)**: `~/.local/bin/deploy-shannon-kiosk` (nit-tracked) — single command does Mac→Darwin rsync, cross-rs aarch64 cross-build, cat-pipe + atomic deploy to Shannon, optional service restart. Replaces the documented 3-step manual procedure below. Cold build ~5min; incremental ~30s-2min.

Full HW-decode reasoning + iteration archaeology + diagnose-first plan + Fredrik's voice anchors: `~/dotfiles/docs/shannon_kiosk_gpu_hwaccel_research_2026_05_18.md` § 6 (canonical aggregator). Supporting: `~/dotfiles/docs/2026-05-25-shannon-hw-decode-archaeology.md` (595 lines, every attempt named) + `~/dotfiles/docs/2026-05-25-rkvdec-detile-bypass-research.md` (why detile bypass alone didn't fix CPU) + `~/dotfiles/docs/2026-05-25-hw-decode-research.md` + **`~/dotfiles/docs/2026-05-25-rk3399-hw-decode-canonical-survey.md`** (empirical floor IS achievable: Werner ~15% on Bookworm+Kwiboo; Trixie+Kwiboo has mpv 0.40 regression; flat-manual-pipeline = <30-min diagnostic). The Watch UI Phase 1.5 work (sidebar-stays + STARTING overlay + 30s debounce) is now named **Watch UI Legibility Pass** per *Self-decoding labels* directive — "Phase 1.5" is deprecated as an anonymous handle.

**Spela web remote integration** ✅ end-to-end + magnet passthrough: target picker shows "Shannon TV" alongside "This phone" and Chromecasts. `spela /play target=shannon` POSTs to `http://192.168.4.30:8080/watch` (override via `shannon_watch_url` in spela config). **Magnet passthrough closed the result-divergence silent bug 2026-05-26** (3-layer chain shipped): spela's do_play resolves `result_id` → `req.magnet` before the shannon dispatch, then forwards `magnet` + `file_index` in the /watch body; shannon-kiosk-actions parses + passes to spela-local via `--magnet M --file-index N` args; spela-local SKIPS its own /search and POSTs Darwin /play with the exact magnet (commits: spela `c6e3de6`, shannon-bedroom-kiosk `1cde18b`, dotfiles `58c677c6`). Library taps still take the title-only path (no magnet for library entries — intentional, falls through cleanly).

## Build & Deploy

**One-command path (recommended)**: `~/.local/bin/deploy-shannon-kiosk` (Mac, nit-tracked) does all three steps below — rsync to Darwin, cross-rs build, atomic deploy to Shannon, optional service restart. `deploy-shannon-kiosk --help` for flags (per-binary build, --no-restart). Cold ~5min, incremental ~30s-2min. See `~/dotfiles/system/shannon/README.md` § "Cross-build deploy automation".

**Manual 3-step (reference for when the automation needs debugging)**:
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

## Status (current — 2026-05-28)

**Headline performance + capability state**:
- **Kiosk idle CPU: ~1.0%** (from baseline 21%; **21× reduction**). Three-layer fix shipped 2026-05-26: `dynamic_wait_system` (commit `22a3d0a`, 50ms idle wait baseline) → gilrs-wake bridge (commit `da4b879`, decouples input latency from `wait` via `EventLoopProxy::send_event`) → `IDLE_WAIT_MS=5000` (commit `063c11f`, 0.2 Hz idle render). gilrs-wake-bridge thread parks in `do_epoll_wait` between events; verified via `wchan` inspection.
- **HW-decode playback CPU: ~33% sustained** (cage ~9% + gst-launch ~24% at 1080p H.264 + AAC, was 99% with playbin3+capsfilter — XDG silent regression closed 2026-05-26 dotfiles `b09297d4`). rkvdec engaged via v4l2slh264dec → waylandsink DMABuf zero-copy. **Next-probe to push toward Werner's empirical 15% floor**: `perf top` on the gst-launch process during playback (decision tree by top kernel symbols documented in `~/Projects/spela/TODO.md § Rank 8`).
- **Layout fix shipped** (commit `c80524b`, 2026-05-28): red-burlap atmospheric backdrop constrained to right area (`BG_FIT_WIDTH 1920→1380, x-offset 270`). Sidebar zone now renders FOREST_BG + wood faded as designed.
- **Repo flipped PUBLIC 2026-05-28**: `gh repo edit semikolon/shannon-bedroom-kiosk --visibility public` after rename pass genericized HA entity defaults (`media_player.fredriks_tv` → `media_player.tv`, `media_player.fredrik` → `media_player.music`); real entity names live in Shannon's `/etc/default/shannon-kiosk-actions` env file (HA_MEDIA_PLAYER_ENTITY + HA_MUSIC_PLAYER_ENTITY + HA_TOKEN). No source-side personal markers remain except the deliberately-kept "Sarpetorp" palette name + "Fredrik YYYY-MM-DD" attribution in comments.
- **Watch UI Phase 3 NowPlaying — SHIPPED + DEPLOYED + VISUALLY VERIFIED LIVE 2026-05-28** (kiosk commit `a94df3c`, deployed to Shannon via canonical `deploy-shannon-kiosk shannon-kiosk` at 17:37, visually verified at 17:47 via `kiosk-inject-gamepad BTN_SOUTH` on a library entry → screenshot showed full NowPlaying panel with real spela data: title from `/status` (`Aladdin S03E08`), state (`■ PROCESS DEAD` — accurate for the spela /status idle-with-stale-current edge case), position `0:00 / 21:48`, sidebar visible per `show_root` extension, hint row). New `src/now_playing.rs` module + `MenuLevel::NowPlaying` variant + `WatchSubmenu A-press` transitions to NowPlaying after dispatch + B returns + full-screen panel renders title + state + position (M:SS or H:MM:SS); polls spela `/status` + `/api/position` every 2s; position interpolates between polls. 17 unit tests + cargo clippy/fmt clean.
- **Watch UI Phase 2B grid — SHIPPED + DEPLOYED + VISUALLY VERIFIED LIVE 2026-05-28** (kiosk commit `0225e41`, visually verified at 23:14 — screenshot showed Aladdin (1992) header above a 5×2 grid of 10 posters with cursor-highlight on Aladdin tile). `WATCH_GRID_SLOTS=10` + `WATCH_GRID_COLS=5`; new `WatchPosterTile` + `WatchPosterTileTitle` + `WatchGridCursorHeader` markers; tiles 240×360 (3:2 TMDB aspect) at x=320 (right of sidebar); cursor highlight via `ImageNode.color` alpha (1.0 vs 0.55); DPad-UD steps by `WATCH_GRID_COLS` in WatchSubmenu, DPad-LR adds new ±1 arm. Legacy `LightsSubmenuLabel/Icon` hidden when `is_watch` (grid replaces them); `WatchPosterPreview` retired (always-hidden, kept spawned for future selected-poster-zoom hook). Phase 1 text-list is fully superseded.
- **Music-NowPlaying view — SHIPPED + DEPLOYED 2026-05-28** (kiosk commit `d2664bd`): daemon polls second media_player (`HA_MUSIC_PLAYER_ENTITY`) and exposes it in `/ha-state` alongside the existing TV media_player; kiosk's `HaSnapshot` carries `music_state` + `music_title` + `music_artist`; new `MenuLevel::MusicNowPlaying` + `music_now_playing_render_system` paints title (large) + artist (medium) + state badge (▶ PLAYING / ⏸ PAUSED / ■ IDLE / ✕ OFF). Wake routing: just_entered_kiosk + music playing/paused → menu_level=MusicNowPlaying (both fresh-input + non-fresh paths). Manual Root→Music nav with music active → MusicNowPlaying too (unified UX); no-music falls through to existing MusicSubmenu. A=play_pause via daemon, DPad-LR=prev/next, B=back. Visual TV verification deferred until next at-TV-with-spotifyd-active session.
- **Watch UI Phase 2A Posters foundation — SHIPPED + DEPLOYED + VISUALLY VERIFIED LIVE 2026-05-28** (kiosk commit `87536d3`, visually verified at 17:44-17:45 via cursor navigation in WatchSubmenu: Aladdin poster + Beasts Of The Southern Wild poster rendered correctly at x=1310 as cursor moved). New `src/posters.rs` module with `PosterRegistry` + background downloader thread (reqwest blocking, 8s timeout, in-memory bytes-cache ~4.5 MB cap for current 57-entry library); main-thread systems request URLs from library snapshot → promote `Bytes` to `Handle<Image>` via `Image::from_buffer(JPEG)` → swap into `WatchPosterPreview` `ImageNode` on cursor moves. 4 unit tests + clean gates. **Phase 2B (5×2 grid layout sharing the same registry) is a follow-up** — deferred because 2A already gives most user-visible win at much lower layout-risk.
- **spela-renderer first slice — SHIPPED + AARCH64 BUILT + DEPLOYED 2026-05-28** (kiosk commit `6254b1d` + MutexGuard fix `e250f18`, 373 KB ELF aarch64 PIE deployed to `/usr/local/bin/spela-renderer` via the extended `deploy-shannon-kiosk spela-renderer` wrapper). New `src/bin/spela-renderer.rs` (~280 lines) + `gst-renderer` opt-in feature + Cross.toml gstreamer-libs delta. THE structural win: proper `tsdemux` pad-added signal handling for the audio branch (the `gst-launch d.` syntax limitation the design doc named). Default cargo build (`shannon-kiosk` + `shannon-kiosk-actions`) still lean — gstreamer pulled only with `--features gst-renderer`. Currently NOT wired into spela-local (which keeps using the v4 shell pipeline) — opt-in available for future testing. Path-A→B fallback + cage takeover logic are v2 slices.
- **HW-decode probe RAN 2026-05-28 evening — decision tree answer = (c) HTTP/HLS**: per-thread pidstat during live Inception cage+waylandsink session showed total ≈ 32% CPU (matches documented baseline), with souphttpsrc (HTTP fetch + HLS) sustaining 2-7% per-tick spikes and queue buffer management ~5% combined ≈ 7-12% of total CPU is HLS plumbing. Decoder + waylandsink negotiate cleanly via DMABuf (GstWlDisplay only 0.40%, no pixman/convert_NV12 anywhere). **Architecture A (v4l2convert RGA) would NOT help** — there's no detile work to absorb. Concrete next steps documented in spela TODO Rank 8: queue-size tuning, pre-fetching wrapper, persistent HTTP/2 connection.

**Unattended-testing primitives** (shipped 2026-05-28; future sessions don't need physical hardware to validate handler paths):
- `kiosk-inject-key <KEY_NAME>...` (dotfiles `fb4c1f9f`) — uinput keyboard injection. Tests `keyboard_event_system` (dev-only handler). Up/Down/X/Q have REAL actions; Enter/Esc log-only.
- `kiosk-inject-gamepad <BTN_NAME>...` (dotfiles `039f7342`) — uinput Xbox 360 gamepad emulation. Tests **PRODUCTION** `gamepad_event_system` including the full South-arm (open submenu, fire `/watch`, etc.). ALSO validates the gilrs-wake bridge end-to-end. Validated 2026-05-28: BTN_SOUTH on Aladdin fired the full chain through to Darwin spela `/play`.
- `shannon-screenshot` (dotfiles `084f16ee`) — auto-discovers cage's XDG_RUNTIME_DIR from `/proc/$cage_pid/environ`; works for BOTH the kiosk's `/run/cage` AND spela-local's `/run/cage-spela-local`. **KNOWN LIMITATION** documented in-script: waylandsink HW-scanout video is INVISIBLE to wlr-screencopy + single-plane kmsgrab (multi-day custom DRM-grab tool would be needed). For test purposes: audio + log inspection + service state give equivalent verification.

**Magnet passthrough on `target=shannon`** ✅ end-to-end (closes the result-divergence silent bug). 3-layer chain shipped 2026-05-26; see "Spela web remote integration" in Phase-7 section above.

**Closet bulb in `group.bedroom_lights`** ✅ (HA configuration.yaml, deployed 2026-05-28 via dotfiles `a783726c`). Every "all off" path (kiosk Sleep tile, Y-button ForceOff, X-toggle, HA nighttime automations) now includes Closet automatically — closes the "Closet stayed on after lights-off" leak.

---

## Status (May 13-14, 2026 — evening)

**User direction on GPU stability tradeoffs** (May 13, 2026, verbatim — load-bearing for any future "should we accept software rendering?" temptation): *"I want you to keep working to get GPU support working. I do not want you to revert to software rendering. It's too slow. I want GPU acceleration working."* Lavapipe (CPU software Vulkan) is the documented Phase 1 fallback but is NOT acceptable as a long-term answer. The mitigation strategies in research doc § G2 are ordered around finding a stable HW-GPU path, not accepting a software-render answer.


### ✅ HW-GLES path UNLOCKED end-to-end through wgpu/Bevy stack

Vendored wgpu-hal 0.21.1-mali-fix at `vendored/wgpu-hal-0.21.1-mali-fix/` with 4-patch evolution (`dc09bfb`, `715917a`, `cd615e0`, `2128543`):
1. **Robustness retry loop** Core→Ext→None on `BadAttribute|BadMatch|BadConfig` (backports upstream PRs #7952 + #9153)
2. **`WGPU_GL_PREFER_GLES=1` env override** forces `bind_api(OPENGL_ES_API)` instead of probing CLIENT_APIS for "OpenGL" — Panfrost desktop-GL caps at 3.1 below wgpu's 3.3 floor; its GLES 3.1 satisfies wgpu's GLES-backend min of 3.0+
3. **`EGL_PLATFORM=x11` veto** on Wayland EGL platform selection — matches the X11 window Bevy emits via Xwayland under cage; ALSO drop the `wayland` Bevy feature so winit can only emit X11 windows (bypasses the wgpu-hal Wayland re-init bug that terminates the AdapterContext's display)
4. **`max_texture_dimension_2d=4096`** to fit ultrawide 3440×1440 monitor (default 2048 is the WebGL2 floor)

Bevy `WgpuSettings::priority=WebGL2` + custom limits keep us in the GLES 3.0/WebGL2 feature subset (Panfrost-on-Midgard doesn't implement VERTEX_STORAGE and caps compute_workgroup at 128). `WinitSettings::Reactive { wait: ... }` — `wait` is now adaptive (see "Architecture" section: `dynamic_wait_system` + gilrs-wake bridge); historically was fixed at 33 ms (~30 fps cap), then 10 ms post-2026-05-21 A/B, now ACTIVE=10ms/IDLE=5000ms post-2026-05-26 structural fix.

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

**`BlackoutTvPower` paints black instead of cutting HDMI signal** — `Off` state paints the screen black via the `BlackoutOverlay` NodeBundle (`GlobalZIndex(500)`) rather than cutting HDMI signal. **Historical rationale (now moot for bedroom)**: the bedroom Argon DA2's EU-EcoDesign auto-standby fired after ~10–20 min of HDMI-source silence, so cutting the signal would force a 10-min cold-start the next time any audio (Ruby narration, music, spotifyd) played. Since the **Jun 6, 2026 amp swap** (Argon to desk, NAD 7220PE to bedroom — see `~/dotfiles/docs/bedroom_audio_station_2026_04_26.md` Jun-6 header note), the bedroom has NO APD constraint. `BlackoutTvPower` remains the default for unrelated reasons: zero re-acquisition latency on wake (no TV input-source renegotiation), no HDMI-EDID handshake flicker, and the existing audio-pipeline-keepalive coverage on the Mac Mini desk side. The opt-in `HdmiSignalTvPower` (`wlr-randr --output HDMI-A-1 --off`) and eventual `PlugTvPower` (smart-plug, post-physical-swap) remain alternatives. Design hub § 13.6.

**HA polling lives in the daemon, not the kiosk** — `shannon-kiosk-actions` (binary at `/usr/local/bin/shannon-kiosk-actions` on Shannon, systemd unit `shannon-kiosk-actions.service`) is the only process that holds `HA_TOKEN`. The Bevy kiosk polls the daemon's `/ha-state` endpoint via `std::thread + reqwest::blocking` at 3 s cadence; the daemon polls HA every 30 s with exponential backoff up to 5 min on failure. The kiosk never talks to HA directly. `EnvironmentFile=-/etc/default/shannon-kiosk-actions` (mode 600 root) holds `HA_TOKEN` + entity-id overrides; deployed value sourced from `~/.secrets/tier-all.env` key `HA_TOKEN_SHANNON`.

**Bevy `is_changed()` is BLIND to `Arc<Mutex<>>`-wrapped resources** — Bevy's change detection tracks DerefMut access through the `Res<T>` / `ResMut<T>` wrapper. Resources that wrap interior-mutable state (`Arc<Mutex<T>>` written from a background thread) NEVER trip `is_changed()` because the wrapper itself isn't mutated through Bevy's ECS path. Symptom: a system gated on `res.is_changed()` early-returns forever, even when the inner data has changed. The 2026-06-09 poster grid bug was exactly this — `watch_poster_request_system` gated on `library_snap.is_changed()` was always false, posters never reached the downloader. Fix pattern: use a `Local<T>` to track the last-observed value of whatever inner field signals change (entry count, mtime, version counter), recompute on mismatch.

**Dynamic-pad elements need `connect_pad_added`, not static `link()`** — `gst-launch`'s `!` operator handles GStreamer dynamic-pad elements (`hlsdemux`, `tsdemux`, `decodebin`, `parsebin`, `urisourcebin`) invisibly via internal pad-added wiring. The gstreamer-rs Rust binding does NOT — calling `.link(&downstream)` on a dynamic-pad element fails at construction time because the source pad doesn't exist yet. Spela-renderer's `hlsdemux.link(&tsdemux)` was silently failing for weeks (always-fallback-to-v4-gst-launch); fix in kiosk commit `dbcd8ca` uses `hlsdemux.connect_pad_added(...)` to wire the dynamic pad to tsdemux's static sink when the HLS playlist parser materializes the muxed-TS variant. The same pattern already worked correctly for tsdemux → queues; the bug was the missing hlsdemux → tsdemux callback (also dynamic).

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

- **Menu**: Games / Music / Lights / Watch / Sensors / Buses / Sleep (seven tiles; Sleep replaces the design-doc Settings per user 2026-05-20 — Sleep + engine `ForceOff` beats config-tile-nobody-uses).
- **Layout**: TLOU-style vertical list left + cursor-driven preview pane right + controller chrome bottom. **No display title** ("let the other stuff take focus").
- **Typography**: Sharp Sans primary (`assets/fonts/SharpSans-{Regular,Semibold,Bold}.otf`, copied from `~/Library/Fonts/`, commercial license, gitignored). Manrope alternative (`assets/fonts/Manrope-Variable.ttf`, OFL, free to commit). *Correction recorded for posterity in design hub § 13.3: the Sarpetorp dashboard's big "SARPETORP" header is Manrope ExtraBold 0.12em tracking in `ClockWidget.tsx`, not Horsemen — Horsemen.otf is bundled in `dashboard/public/fonts/` but unused; my earlier guess from the filename was wrong, Fredrik caught it.*
- **Palette + bg**: Sarpetorp forest theme (dark forest radial-gradient base + oat-milk cream text + warm-cream/amber accent). Background = `assets/backgrounds/sarpetorp-clock-bg.jpg`, copied from `~/Projects/sarpetorp/dashboard/public/clock-bg.jpg` (the Sarpetorp dashboard's top-widget bg).
- **Icons**: Lucide font (pending download), monochrome single-accent discipline (all icons render in the same cream/amber, no rainbow tinting — per Image #10).
- **TV-off actuator**: `BlackoutTvPower` default (renders black Bevy scene; zero HDMI re-acquisition latency on wake + no EDID flicker). Historical rationale "preserves Argon DA2 keepalive" no longer applies to bedroom since the **Jun 6 amp swap** (Argon to desk, NAD 7220PE to bedroom — NAD has no APD); see `~/dotfiles/docs/bedroom_audio_station_2026_04_26.md` Jun-6 note. Opt-in `HdmiSignalTvPower` (`wlr-randr --output HDMI-A-1 --off`, mirrors Sarpetorp's `xset dpms` pattern via `~/Projects/sarpetorp/handlers/display_control_handler.rb`). `PlugTvPower` (smart-plug; post-physical-swap, TODO entry C) eventual production path.
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
- ~~Wire the already-installed gaming stack (RetroArch + 6 libretro cores + xpadneo + mrboom, done May 6) into the menu's 🎮 Games submenu.~~ ✅ **Done 2026-06-06** — `GamesSubmenu` (SuperTuxKart + RetroArch-main-menu), `try_dispatch_game` + daemon `/game/<name>` route → `shannon-games` subprocess. See tile-status table above for details.
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
