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
//!     spela-renderer <HLS_URL>
//!
//! Returns exit code 0 on clean EOS, non-zero on pipeline error.

use std::env;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;

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

    // Source: HTTP HLS via souphttpsrc → hlsdemux → tsdemux. Use the
    // LEGACY hlsdemux (NOT hlsdemux2) — hlsdemux2 requires the
    // GST_BIN_FLAG_STREAMS_AWARE which only playbin3/decodebin3 set,
    // and a hand-built bin can't satisfy. The legacy element has no
    // such constraint and is what spela-local empirically uses.
    let souphttpsrc = gst::ElementFactory::make("souphttpsrc")
        .property("location", hls_url)
        .build()
        .map_err(|e| format!("make souphttpsrc: {e}"))?;
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
    let video_queue = gst::ElementFactory::make("queue")
        .property_from_str("max-size-buffers", "200")
        .property("max-size-time", 0u64)
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
    let audio_queue = gst::ElementFactory::make("queue")
        .property_from_str("max-size-buffers", "200")
        .property("max-size-time", 0u64)
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

/// Walk the GStreamer bus until EOS or a fatal Error. Returns Ok(()) on
/// clean EOS; Err(msg) on pipeline error. State-changed messages from
/// non-pipeline elements are ignored.
fn run_until_eos(pipeline: &gst::Pipeline) -> Result<(), String> {
    let bus = pipeline
        .bus()
        .ok_or_else(|| "pipeline has no bus".to_string())?;
    loop {
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
    let audio_device = args
        .next()
        .unwrap_or_else(|| "plughw:CARD=hdmisound,DEV=0".to_string());

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
    if let Err(e) = pipeline.set_state(gst::State::Playing) {
        eprintln!("[spela-renderer] set_state(Playing) failed: {e}");
        let _ = pipeline.set_state(gst::State::Null);
        return ExitCode::from(1);
    }

    let result = run_until_eos(&pipeline);

    // Always tear down cleanly so the wrapper's trap EXIT doesn't see
    // a hanging pipeline holding the wayland socket / ALSA device.
    eprintln!("[spela-renderer] tearing down pipeline");
    let _ = pipeline.set_state(gst::State::Null);
    // brief sleep to flush async state changes
    std::thread::sleep(Duration::from_millis(200));

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
