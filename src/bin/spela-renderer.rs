//! `spela-renderer` — Rust gstreamer-rs replacement for `spela-local`'s
//! shell pipeline. First slice (2026-05-28): the cage+waylandsink HW
//! pipeline with proper pad-added signal handling on tsdemux for the
//! audio branch.
//!
//! Why this exists (independent of the rkvdec "regression" misdiagnosis,
//! per `~/dotfiles/docs/shannon_gstreamer_rs_renderer_design_2026_05_24.md`):
//! 1. **Audio branch needs pad-added signal handling** that `gst-launch`
//!    syntax cannot express. `tsdemux` is a dynamic-pad element — pads
//!    materialize only when input data is decoded, so `tsdemux name=d
//!    ! ...  d. ! aacparse ...` syntax fails at pipeline construction.
//! 2. **Per-V4L2-ioctl error visibility** for legitimate future decoder
//!    issues — Rust path intercepts cleanly instead of bubbling as EOS.
//! 3. Foundation for **deterministic Path A→B fallback** (v2 slice).
//!
//! Lifecycle (matches `spela-local`'s cage+waylandsink path):
//! - Caller (wrapper script) has already stopped `shannon-display.service`,
//!   launched `cage`, and exec'd this binary under the cage compositor
//!   with `XDG_RUNTIME_DIR=/run/cage-spela-local` + `env -u WAYLAND_DISPLAY`.
//! - This binary builds + runs the GStreamer pipeline against the HLS
//!   URL given as `argv[1]`, listens to the bus until EOS or error,
//!   exits cleanly so the wrapper's `trap EXIT` restores the kiosk.
//!
//! Usage:
//!     spela-renderer <HLS_URL> [audio_device]
//!
//! Returns exit code 0 on clean EOS, non-zero on pipeline error.
//!
//! Control IPC (Phase 4, 2026-05-29) — Unix socket at
//! `$XDG_RUNTIME_DIR/spela-renderer.sock` (falls back to
//! `/tmp/spela-renderer.sock` when XDG_RUNTIME_DIR is unset, e.g. ad-hoc
//! manual runs). Newline-delimited text protocol:
//!
//!   seek_relative <signed-seconds>\n  → seek N seconds from current
//!   seek_absolute <seconds>\n         → seek to absolute position
//!   quit\n                            → graceful EOS + shutdown
//!
//! Each command receives a single-line response on the same connection:
//!   ok pos=<secs>\n                   → seek/quit accepted
//!   err <message>\n                   → bad parse, no pipeline, etc.
//!
//! The listener thread holds an Arc<Mutex<Pipeline>>; commands translate
//! to `Pipeline::seek_simple` with FLUSH + KEY_UNIT flags (KEY_UNIT keeps
//! seek points on H.264 keyframes — Mali rkvdec stateless decoder doesn't
//! support arbitrary-frame seek; FLUSH discards in-flight buffers).
//! Socket is removed on pipeline teardown so the next session starts fresh.

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;

use shannon_kiosk::spela_control_proto::{parse_command, Command};

/// Build the cage+waylandsink HW pipeline. Mirrors the empirically-
/// validated `spela-local` v4 chain:
///   souphttpsrc ! hlsdemux ! tsdemux name=demux
///   demux. ! queue ! h264parse ! v4l2slh264dec ! waylandsink
///   demux. ! queue ! aacparse ! avdec_aac ! audioconvert
///         ! audioresample ! alsasink
///
/// audioconvert + audioresample are LOAD-BEARING (not optional CPU
/// overhead — see spela CLAUDE.md "Shannon spela-local audio chain"
/// for the experimentally-verified link-time failure when they're
/// removed: avdec_aac advertises F32LE, alsasink with plughw advertises
/// HDMI-native S16LE, no intersection → cannot link without the
/// format-bridge elements).
///
/// audio_device: ALSA PCM device for the audio branch (e.g.
/// "plughw:CARD=hdmisound,DEV=0" for Shannon's HDMI). Pulled from
/// the CLI rather than hardcoded so the same binary works on dev
/// hardware with a different sound card.
fn build_pipeline(hls_url: &str, audio_device: &str) -> Result<gst::Pipeline, String> {
    let pipeline = gst::Pipeline::with_name("spela-renderer");

    // Source: HTTP HLS via curlhttpsrc (preferred, libcurl backend with
    // better connection-reuse + HTTP/1.1 keep-alive across segments) →
    // hlsdemux → tsdemux. Falls back to souphttpsrc if curlhttpsrc is
    // unavailable (older GStreamer installs). Use the LEGACY hlsdemux
    // (NOT hlsdemux2) — hlsdemux2 requires the GST_BIN_FLAG_STREAMS_AWARE
    // which only playbin3/decodebin3 set, and a hand-built bin can't
    // satisfy. The legacy element has no such constraint.
    let http_src = match gst::ElementFactory::make("curlhttpsrc")
        .property("location", hls_url)
        .build()
    {
        Ok(el) => {
            eprintln!("[spela-renderer] HTTP source: curlhttpsrc");
            el
        }
        Err(_) => {
            eprintln!("[spela-renderer] curlhttpsrc unavailable, using souphttpsrc");
            gst::ElementFactory::make("souphttpsrc")
                .property("location", hls_url)
                .build()
                .map_err(|e| format!("make souphttpsrc: {e}"))?
        }
    };
    let souphttpsrc = http_src;
    let hlsdemux = gst::ElementFactory::make("hlsdemux").build().map_err(|e| {
        format!("make hlsdemux (legacy needed; hlsdemux2 won't work in a plain bin): {e}")
    })?;
    let tsdemux = gst::ElementFactory::make("tsdemux")
        .build()
        .map_err(|e| format!("make tsdemux: {e}"))?;

    // Video chain: queue → h264parse → v4l2slh264dec → waylandsink.
    // No videoconvert and NO format=NV12 capsfilter between
    // v4l2slh264dec and waylandsink (per the empirical 2026-05-26
    // breakthrough: extra elements between decoder and sink trigger
    // the playbin3-sandwich autoplug pattern that pegs CPU at 99%).
    // waylandsink natively handles DMABuf-with-VideoMeta via wl_buffer.
    //
    // 2026-05-29 — queue tuning per spela TODO Rank 8 perf probe finding:
    // souphttpsrc + queue management was ~7-12% of the 32% total CPU.
    // The 200-buffer / unlimited-time cap means the queue empties between
    // HLS segment fetches, forcing souphttpsrc to spike at fetch time.
    // Switch to a TIME-based cap (10 seconds of video) so the queue stays
    // full across segment boundaries and souphttpsrc's pulses smooth out.
    // 10s * 30fps = ~300 frames @ 1080p H.264 ~6 Mbps ≈ 7.5 MB — well
    // within Shannon's 4 GB RAM. max-size-buffers raised to 600 as a
    // safety ceiling against bursty keyframe densities.
    const QUEUE_TIME_NS: u64 = 10_000_000_000; // 10 seconds
    let video_queue = gst::ElementFactory::make("queue")
        .property_from_str("max-size-buffers", "600")
        .property("max-size-time", QUEUE_TIME_NS)
        .property("max-size-bytes", 0u32)
        .build()
        .map_err(|e| format!("make video queue: {e}"))?;
    let h264parse = gst::ElementFactory::make("h264parse")
        .build()
        .map_err(|e| format!("make h264parse: {e}"))?;
    let v4l2dec = gst::ElementFactory::make("v4l2slh264dec")
        .build()
        .map_err(|e| {
            format!(
                "make v4l2slh264dec (Rockchip rkvdec stateless H.264 decoder; required for HW path): {e}"
            )
        })?;
    let waylandsink = gst::ElementFactory::make("waylandsink")
        .build()
        .map_err(|e| format!("make waylandsink: {e}"))?;

    // Audio chain: queue → aacparse → avdec_aac → audioconvert →
    // audioresample → alsasink. The convert+resample pair is mandatory
    // (load-bearing format bridge — see CLAUDE.md "Shannon spela-local
    // audio chain are LOAD-BEARING").
    // Same time-based cap as the video queue (10s) — paired buffering
    // keeps A/V sync windows consistent under fetch jitter.
    let audio_queue = gst::ElementFactory::make("queue")
        .property_from_str("max-size-buffers", "600")
        .property("max-size-time", QUEUE_TIME_NS)
        .property("max-size-bytes", 0u32)
        .build()
        .map_err(|e| format!("make audio queue: {e}"))?;
    let aacparse = gst::ElementFactory::make("aacparse")
        .build()
        .map_err(|e| format!("make aacparse: {e}"))?;
    let avdec_aac = gst::ElementFactory::make("avdec_aac")
        .build()
        .map_err(|e| format!("make avdec_aac: {e}"))?;
    let audioconvert = gst::ElementFactory::make("audioconvert")
        .build()
        .map_err(|e| format!("make audioconvert: {e}"))?;
    let audioresample = gst::ElementFactory::make("audioresample")
        .build()
        .map_err(|e| format!("make audioresample: {e}"))?;
    let alsasink = gst::ElementFactory::make("alsasink")
        .property("device", audio_device)
        .property("sync", false)
        .build()
        .map_err(|e| format!("make alsasink: {e}"))?;

    // Add everything to the pipeline + link the static portions.
    pipeline
        .add_many([&souphttpsrc, &hlsdemux, &tsdemux])
        .map_err(|e| format!("add source elements: {e}"))?;
    pipeline
        .add_many([
            &video_queue,
            &h264parse,
            &v4l2dec,
            &waylandsink,
            &audio_queue,
            &aacparse,
            &avdec_aac,
            &audioconvert,
            &audioresample,
            &alsasink,
        ])
        .map_err(|e| format!("add sink elements: {e}"))?;

    // Static linkage. tsdemux's output pads are dynamic — wired in the
    // pad-added callback below.
    souphttpsrc
        .link(&hlsdemux)
        .map_err(|e| format!("link souphttpsrc→hlsdemux: {e}"))?;
    hlsdemux
        .link(&tsdemux)
        .map_err(|e| format!("link hlsdemux→tsdemux: {e}"))?;
    gst::Element::link_many([&video_queue, &h264parse, &v4l2dec, &waylandsink])
        .map_err(|e| format!("link video chain: {e}"))?;
    gst::Element::link_many([
        &audio_queue,
        &aacparse,
        &avdec_aac,
        &audioconvert,
        &audioresample,
        &alsasink,
    ])
    .map_err(|e| format!("link audio chain: {e}"))?;

    // Pad-added signal — THE reason we're in Rust rather than
    // gst-launch. tsdemux produces dynamic output pads as it parses
    // the MPEG-TS stream; we need to link the right type into the
    // right sink-chain at runtime.
    //
    // Race notes:
    // - tsdemux can fire pad-added multiple times across stream
    //   restarts; the `is_linked()` guards keep us idempotent.
    // - The video-pad type is `video/x-h264` (after hlsdemux+tsdemux
    //   demuxing); the audio-pad type is `audio/mpeg` (AAC).
    let video_sink_pad = video_queue
        .static_pad("sink")
        .ok_or_else(|| "video_queue has no sink pad".to_string())?;
    let audio_sink_pad = audio_queue
        .static_pad("sink")
        .ok_or_else(|| "audio_queue has no sink pad".to_string())?;
    // Wrap in Arc<Mutex<>> to satisfy Send + Fn capture across the
    // signal closure (pads themselves are not Sync on all platforms;
    // wrapping keeps the API portable).
    let video_sink_pad = Arc::new(Mutex::new(video_sink_pad));
    let audio_sink_pad = Arc::new(Mutex::new(audio_sink_pad));
    let vsp = video_sink_pad.clone();
    let asp = audio_sink_pad.clone();
    tsdemux.connect_pad_added(move |_demux, src_pad| {
        let caps = match src_pad.current_caps() {
            Some(c) => c,
            None => return,
        };
        let structure = match caps.structure(0) {
            Some(s) => s,
            None => return,
        };
        let name = structure.name();
        if name.starts_with("video/x-h264") {
            if let Ok(sink) = vsp.lock() {
                if !sink.is_linked() {
                    let _ = src_pad.link(&*sink);
                }
            }
        } else if name.starts_with("audio/mpeg") {
            if let Ok(sink) = asp.lock() {
                if !sink.is_linked() {
                    let _ = src_pad.link(&*sink);
                }
            }
        }
        // Other pad types (subtitles, metadata, etc.) — drop silently.
    });

    Ok(pipeline)
}

/// Phase 4 — resolve the control socket path. Uses XDG_RUNTIME_DIR when
/// available (the daemon-launched case via cage RuntimeDirectory), falls
/// back to /tmp for ad-hoc manual runs.
fn control_socket_path() -> PathBuf {
    if let Ok(xdg) = env::var("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("spela-renderer.sock");
        }
    }
    PathBuf::from("/tmp/spela-renderer.sock")
}

/// Apply a parsed command to the live pipeline. Returns the resulting
/// position in seconds for the success response (best-effort — query
/// failure returns 0). Quit triggers `quit_signal` (set to true) which
/// the bus loop polls to break out cleanly.
fn apply_command(
    cmd: Command,
    pipeline: &gst::Pipeline,
    quit_signal: &Arc<Mutex<bool>>,
) -> Result<u64, String> {
    let flags = gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT;
    match cmd {
        Command::SeekRelative(delta) => {
            let current_ns = pipeline
                .query_position::<gst::ClockTime>()
                .ok_or_else(|| "query_position failed".to_string())?
                .nseconds() as i128;
            let delta_ns = (delta as i128) * 1_000_000_000;
            let mut target_ns = current_ns + delta_ns;
            if target_ns < 0 {
                target_ns = 0;
            }
            // Clamp to duration if known (avoid seeking past EOS).
            if let Some(dur) = pipeline.query_duration::<gst::ClockTime>() {
                let dur_ns = dur.nseconds() as i128;
                if target_ns > dur_ns {
                    target_ns = dur_ns;
                }
            }
            let target = gst::ClockTime::from_nseconds(target_ns as u64);
            pipeline
                .seek_simple(flags, target)
                .map_err(|e| format!("seek_simple: {e}"))?;
            Ok(target.seconds())
        }
        Command::SeekAbsolute(secs) => {
            let target = gst::ClockTime::from_seconds(secs);
            pipeline
                .seek_simple(flags, target)
                .map_err(|e| format!("seek_simple: {e}"))?;
            Ok(secs)
        }
        Command::PlayPause => {
            // Toggle Paused ↔ Playing. Use the pipeline's CURRENT state
            // as the truth (set_state target is a one-shot; we want a
            // toggle relative to where the pipeline actually is now).
            let (_, current, _) = pipeline.state(Some(gst::ClockTime::ZERO));
            let target = match current {
                gst::State::Playing => gst::State::Paused,
                _ => gst::State::Playing,
            };
            pipeline
                .set_state(target)
                .map_err(|e| format!("set_state({target:?}): {e}"))?;
            let pos = pipeline
                .query_position::<gst::ClockTime>()
                .map(|t| t.seconds())
                .unwrap_or(0);
            Ok(pos)
        }
        Command::Quit => {
            if let Ok(mut q) = quit_signal.lock() {
                *q = true;
            }
            let pos = pipeline
                .query_position::<gst::ClockTime>()
                .map(|t| t.seconds())
                .unwrap_or(0);
            Ok(pos)
        }
    }
}

/// Spawn the control-socket listener thread. Owns the socket file
/// lifecycle: creates on start, removes on drop / EOS via best-effort
/// remove_file. Each accepted connection is line-delimited; the thread
/// reads one command, applies it, writes the response, closes the
/// connection. Errors are isolated per-connection so a malformed client
/// doesn't take down the listener.
fn spawn_control_listener(
    pipeline: Arc<Mutex<gst::Pipeline>>,
    quit_signal: Arc<Mutex<bool>>,
    socket_path: PathBuf,
) -> Result<(), String> {
    // Remove any stale socket from a previous crash before binding.
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)
        .map_err(|e| format!("bind {}: {}", socket_path.display(), e))?;
    // Non-blocking accept lets us drop the listener cleanly on quit;
    // a small sleep keeps the loop from busy-spinning.
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking: {e}"))?;
    eprintln!(
        "[spela-renderer] control socket listening at {}",
        socket_path.display()
    );
    let socket_for_cleanup = socket_path.clone();
    thread::Builder::new()
        .name("spela-renderer-control".to_string())
        .spawn(move || {
            loop {
                // Check the quit flag — when the bus loop sets it (via a
                // quit command OR EOS/error), we tear down.
                if quit_signal.lock().map(|q| *q).unwrap_or(false) {
                    break;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        let pipeline = pipeline.clone();
                        let quit_signal = quit_signal.clone();
                        // Handle each connection on its own thread so a
                        // slow/hung client can't block the listener.
                        let _ = thread::Builder::new()
                            .name("spela-renderer-conn".to_string())
                            .spawn(move || {
                                handle_connection(stream, pipeline, quit_signal);
                            });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) => {
                        eprintln!("[spela-renderer] accept error: {e}");
                        thread::sleep(Duration::from_millis(200));
                    }
                }
            }
            // Best-effort cleanup. Renderer EXIT trap handles the
            // shannon-display.service restart; we just remove the socket.
            let _ = std::fs::remove_file(&socket_for_cleanup);
            eprintln!("[spela-renderer] control listener exiting");
        })
        .map_err(|e| format!("spawn control listener: {e}"))?;
    Ok(())
}

/// Handle a single connection: read ONE line, apply, respond.
fn handle_connection(
    stream: UnixStream,
    pipeline: Arc<Mutex<gst::Pipeline>>,
    quit_signal: Arc<Mutex<bool>>,
) {
    // Modest read timeout — a misbehaving client shouldn't hang us.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    let response = match reader.read_line(&mut line) {
        Ok(_) => match parse_command(&line) {
            Ok(Some(cmd)) => match pipeline.lock() {
                Ok(p) => match apply_command(cmd, &p, &quit_signal) {
                    Ok(pos) => format!("ok pos={pos}\n"),
                    Err(e) => format!("err {e}\n"),
                },
                Err(e) => format!("err pipeline lock poisoned: {e}\n"),
            },
            Ok(None) => "err empty command\n".to_string(),
            Err(e) => format!("err {e}\n"),
        },
        Err(e) => format!("err read_line: {e}\n"),
    };
    let mut writer = stream;
    let _ = writer.write_all(response.as_bytes());
}

/// Phase 4 (2026-05-29) — Xbox controller listener for during-playback
/// transport control. The kiosk Bevy app is killed during spela-local
/// playback (shannon-display.service stopped for scanout handoff), so
/// the kiosk's gilrs handler is offline. We open a SECOND gilrs
/// instance here that runs while playback is active and applies
/// commands directly to the pipeline (skipping the Unix socket
/// round-trip the kiosk uses during cold-start).
///
/// Button map:
///   DPad-Left  → seek -SEEK_STEP_SECS
///   DPad-Right → seek +SEEK_STEP_SECS
///   West (X)   → seek_absolute 0 (restart from start)
///   South (A)  → play_pause
///   East (B)   → quit (returns to kiosk via spela-local's trap EXIT)
///
/// Failure-soft: a gilrs init failure logs and skips the listener;
/// playback proceeds without controller input (caller can still use
/// the Unix socket from another process).
const SEEK_STEP_SECS: i64 = 30;

fn spawn_controller_listener(
    pipeline: Arc<Mutex<gst::Pipeline>>,
    quit_signal: Arc<Mutex<bool>>,
) -> Result<(), String> {
    use gilrs::{Button, EventType, Gilrs};
    let mut gilrs = Gilrs::new().map_err(|e| format!("gilrs init: {e}"))?;
    eprintln!(
        "[spela-renderer] controller listener spawned ({} gamepad(s) connected)",
        gilrs.gamepads().count()
    );
    thread::Builder::new()
        .name("spela-renderer-controller".to_string())
        .spawn(move || {
            loop {
                if quit_signal.lock().map(|q| *q).unwrap_or(false) {
                    break;
                }
                while let Some(event) = gilrs.next_event() {
                    let EventType::ButtonPressed(button, _) = event.event else {
                        continue;
                    };
                    let cmd = match button {
                        Button::DPadLeft => Some(Command::SeekRelative(-SEEK_STEP_SECS)),
                        Button::DPadRight => Some(Command::SeekRelative(SEEK_STEP_SECS)),
                        Button::West => Some(Command::SeekAbsolute(0)),
                        Button::South => Some(Command::PlayPause),
                        Button::East => Some(Command::Quit),
                        _ => None,
                    };
                    if let Some(cmd) = cmd {
                        let result = match pipeline.lock() {
                            Ok(p) => apply_command(cmd, &p, &quit_signal),
                            Err(e) => Err(format!("pipeline lock poisoned: {e}")),
                        };
                        match result {
                            Ok(pos) => {
                                eprintln!("[spela-renderer] controller {cmd:?} → ok pos={pos}")
                            }
                            Err(e) => eprintln!("[spela-renderer] controller {cmd:?} → err {e}"),
                        }
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }
            eprintln!("[spela-renderer] controller listener exiting");
        })
        .map_err(|e| format!("spawn controller listener: {e}"))?;
    Ok(())
}

/// Walk the GStreamer bus until EOS or a fatal Error. Returns Ok(()) on
/// clean EOS; Err(msg) on pipeline error. State-changed messages from
/// non-pipeline elements are ignored.
///
/// Phase 4: also polls the quit_signal each tick — when a `quit` IPC
/// command arrives, the listener sets the flag and we issue EOS so the
/// pipeline tears down cleanly.
fn run_until_eos(pipeline: &gst::Pipeline, quit_signal: &Arc<Mutex<bool>>) -> Result<(), String> {
    let bus = pipeline
        .bus()
        .ok_or_else(|| "pipeline has no bus".to_string())?;
    loop {
        // Poll the quit flag — set by the IPC `quit` command. We send
        // EOS through the bus so the pipeline flushes cleanly; the next
        // bus tick will see the EOS message and return Ok.
        if quit_signal.lock().map(|q| *q).unwrap_or(false) {
            // Idempotent — sending EOS to an already-EOS pipeline is
            // harmless. send_event blocks briefly; that's fine here.
            pipeline.send_event(gst::event::Eos::new());
        }
        let msg = bus.timed_pop(Some(gst::ClockTime::from_seconds(1)));
        let m = match msg {
            Some(m) => m,
            None => continue,
        };
        match m.view() {
            gst::MessageView::Eos(_) => return Ok(()),
            gst::MessageView::Error(err) => {
                let src = err
                    .src()
                    .map(|s| s.path_string().to_string())
                    .unwrap_or_default();
                return Err(format!(
                    "pipeline error from {src}: {} (debug: {:?})",
                    err.error(),
                    err.debug()
                ));
            }
            gst::MessageView::Warning(w) => {
                eprintln!(
                    "[spela-renderer] warning: {} (debug: {:?})",
                    w.error(),
                    w.debug()
                );
            }
            _ => {}
        }
    }
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let hls_url = match args.next() {
        Some(u) => u,
        None => {
            eprintln!(
                "usage: spela-renderer <HLS_URL> [audio_device]\n\
                 example: spela-renderer http://darwin.home:7890/hls/master.m3u8 plughw:CARD=hdmisound,DEV=0"
            );
            return ExitCode::from(2);
        }
    };
    // Audio device resolution (precedence high → low):
    //   1. argv[2] CLI arg (explicit override)
    //   2. $SHANNON_AUDIO_DEVICE env var
    //   3. "default" — PipeWire-routed ALSA default, follows wpctl
    //      set-default. The historical hardcoded value
    //      "plughw:CARD=hdmisound,DEV=0" was direct HDMI; switching to
    //      "default" lets sink-switching (kiosk Sound menu / shannon-
    //      audio-sink CLI) reroute spela playback to Bluetooth speakers
    //      etc. spela-local sets PIPEWIRE_RUNTIME_DIR=/run/user/0 so
    //      the ALSA-PipeWire bridge finds the root user PipeWire socket
    //      from inside cage's scoped XDG_RUNTIME_DIR.
    let audio_device = args.next().unwrap_or_else(|| {
        env::var("SHANNON_AUDIO_DEVICE").unwrap_or_else(|_| "default".to_string())
    });

    if let Err(e) = gst::init() {
        eprintln!("[spela-renderer] gst::init failed: {e}");
        return ExitCode::from(1);
    }

    let pipeline = match build_pipeline(&hls_url, &audio_device) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[spela-renderer] build_pipeline failed: {e}");
            return ExitCode::from(1);
        }
    };

    eprintln!("[spela-renderer] starting pipeline (HLS={hls_url} audio={audio_device})");
    // 2026-06-08 fix: explicit PAUSED → wait-for-state-change → PLAYING.
    // The prior direct-to-PLAYING path caused waylandsink to race
    // cage's xdg_surface initial-configure event, producing
    // "A configure is scheduled for an uninitialized xdg_surface"
    // (wlr_xdg_surface.c:169) and silently aborting. gst-launch
    // implicitly does PAUSED + wait_for_ASYNC_DONE + PLAYING — the
    // v4 fallback path works because of that staging. Mirror it here
    // so waylandsink can complete its Wayland handshake with cage
    // before the bus starts pushing frames.
    if let Err(e) = pipeline.set_state(gst::State::Paused) {
        eprintln!("[spela-renderer] set_state(Paused) failed: {e}");
        let _ = pipeline.set_state(gst::State::Null);
        return ExitCode::from(1);
    }
    // Wait up to 10s for the pipeline to fully reach PAUSED (waylandsink
    // surface configured by then). 10s covers cold HLS handshake +
    // demux + decoder caps negotiation. Timeout/error tears down cleanly.
    let (state_result, _current, _pending) =
        pipeline.state(Some(gst::ClockTime::from_seconds(10)));
    if let Err(e) = state_result {
        eprintln!("[spela-renderer] PAUSED state change failed/timeout: {e}");
        let _ = pipeline.set_state(gst::State::Null);
        return ExitCode::from(1);
    }
    if let Err(e) = pipeline.set_state(gst::State::Playing) {
        eprintln!("[spela-renderer] set_state(Playing) failed: {e}");
        let _ = pipeline.set_state(gst::State::Null);
        return ExitCode::from(1);
    }

    // Phase 4 — spawn the control socket listener. Wrap the pipeline in
    // Arc<Mutex<>> so the listener can apply seek commands without
    // racing the main bus loop. Failure to bind is non-fatal: log it
    // and proceed without IPC (playback still works).
    let pipeline_arc = Arc::new(Mutex::new(pipeline.clone()));
    let quit_signal = Arc::new(Mutex::new(false));
    let socket_path = control_socket_path();
    if let Err(e) = spawn_control_listener(
        pipeline_arc.clone(),
        quit_signal.clone(),
        socket_path.clone(),
    ) {
        eprintln!("[spela-renderer] control listener disabled: {e}");
    }

    // Phase 4 (2026-05-29) — Xbox controller listener for direct
    // during-playback control. Soft-failure: log + continue if gilrs
    // init fails (no controller plugged in / udev missing / etc.).
    if let Err(e) = spawn_controller_listener(pipeline_arc.clone(), quit_signal.clone()) {
        eprintln!("[spela-renderer] controller listener disabled: {e}");
    }

    let result = run_until_eos(&pipeline, &quit_signal);

    // Always tear down cleanly so the wrapper's trap EXIT doesn't see
    // a hanging pipeline holding the wayland socket / ALSA device.
    eprintln!("[spela-renderer] tearing down pipeline");
    // Signal the control listener to exit + clean up its socket.
    if let Ok(mut q) = quit_signal.lock() {
        *q = true;
    }
    let _ = pipeline.set_state(gst::State::Null);
    // brief sleep to flush async state changes
    std::thread::sleep(Duration::from_millis(200));
    // Best-effort socket cleanup (the listener also tries; both safe).
    let _ = std::fs::remove_file(&socket_path);

    match result {
        Ok(()) => {
            eprintln!("[spela-renderer] clean EOS");
            ExitCode::from(0)
        }
        Err(e) => {
            eprintln!("[spela-renderer] {e}");
            ExitCode::from(1)
        }
    }
}

// Tests for the parser live in `src/spela_control_proto.rs` so they
// can run on any host (no gstreamer system dep required).
