//! Phase-3A bedroom display/power **context engine** (Slice 1).
//!
//! A four-state machine — `Off` / `Kiosk` / `Content` / `Ambient` — that
//! owns the bedroom TV so the user never touches the TV (or Argon) remote.
//! Pure + deterministic: the caller injects every signal (including the
//! clock); the engine never does I/O. The real HA-smart-plug actuator is a
//! later one-line `TvPower` adapter swap; tests drive `SimTvPower`.
//!
//! Decision order, presence oracle, the time-conditioned sleep-aware
//! true-OFF leash, and the passive idle-only guardrail are specified in
//! the design doc (§ 4 / § 6 / § 7). Keep this file in sync with it.

#![forbid(unsafe_code)]

use std::time::Duration;

/// Local wall-clock as minutes since midnight (`0..1440`). Injected by the
/// caller — the engine never reads a system clock, so tests are
/// deterministic (*split semantic from temporal*: engine decides WHAT, the
/// caller supplies WHEN).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClockMinutes(u16);

impl ClockMinutes {
    /// Minutes in a day.
    pub const DAY: u16 = 24 * 60;

    /// Construct from `h:m`, wrapping into `0..1440`.
    #[must_use]
    pub fn at(h: u16, m: u16) -> Self {
        Self((h.wrapping_mul(60).wrapping_add(m)) % Self::DAY)
    }

    /// Raw minutes since midnight.
    #[must_use]
    pub fn raw(self) -> u16 {
        self.0 % Self::DAY
    }
}

/// On-screen activity that *is* the point (stays on while engaged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Video,
    Game,
}

/// Adaptive-dim ambient screensaver brightness, clamped to `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Brightness(f32);

impl Brightness {
    #[must_use]
    pub fn new(v: f32) -> Self {
        Self(v.clamp(0.0, 1.0))
    }

    #[must_use]
    pub fn get(self) -> f32 {
        self.0
    }
}

/// The four bedroom display/power states.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DisplayState {
    /// Smart-plug cuts the TV; room dark.
    Off,
    /// The stable hybrid retro menu (validated Phase 2).
    Kiosk,
    /// A video or running game — the content is the point.
    Content(Activity),
    /// Calm, adaptively-dim creative screensaver while present-but-idle.
    Ambient(Brightness),
}

impl DisplayState {
    /// Is the TV powered in this state?
    #[must_use]
    pub fn powered(self) -> bool {
        !matches!(self, DisplayState::Off)
    }
}

/// What audio/video is currently playing (the engine's content signal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Media {
    #[default]
    None,
    Music,
    Video,
    Game,
}

/// The Xbox on/off button — the TV-remote replacement. Sticky until a
/// fresh controller interaction (a new deliberate intent) or the hard-off
/// floor supersedes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manual {
    ForceOn,
    ForceOff,
}

/// All observable signals for one evaluation tick. Everything is injected:
/// the engine reads no clock, no device, no network.
#[derive(Debug, Clone, Copy)]
pub struct Inputs {
    /// Local wall-clock (caller-supplied).
    pub now: ClockMinutes,
    /// Xbox controller currently BT-connected (presence: present vs gone).
    pub controller_connected: bool,
    /// Time since the last controller button/axis input (idle oracle).
    pub since_controller_input: Duration,
    /// A fresh controller input arrived this tick (supersedes a stale
    /// manual override — a new deliberate intent).
    pub fresh_controller_input: bool,
    /// What is playing right now.
    pub media: Media,
    /// Outdoor brightness `0.0..=1.0` (the solar-curve proxy that drives
    /// adaptive-dim Ambient — no in-room light sensor needed).
    pub outdoor_brightness: f32,
    /// A fresh Xbox on/off press this tick, if any.
    pub manual_press: Option<Manual>,
}

/// Tunable parameters. **Architecture-vs-parameters**: that these knobs
/// *exist* is decided; their *values* are observation-tuned — do not ask,
/// observe. Defaults are documented starting guesses.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Connected + idle ≥ this → Ambient (a short grace as Kiosk first).
    pub idle_to_ambient: Duration,
    /// Daytime idle leash before true-OFF (long; instant-on premise).
    pub day_idle_to_off: Duration,
    /// Evening idle leash before true-OFF (short; the sleep nudge).
    pub night_idle_to_off: Duration,
    /// Controller-disconnected grace held as Ambient before OFF.
    pub disconnect_grace: Duration,
    /// When the evening short-leash begins (the user's sleep rhythm —
    /// tunable; no clock-cognitive-mode assumption is baked in).
    pub winddown_start: ClockMinutes,
    /// Post-midnight hard-off floor start (mirrors bedroom-lights).
    pub hard_off_start: ClockMinutes,
    /// Hard-off floor end / day begins.
    pub hard_off_end: ClockMinutes,
    /// Ambient brightness floor at zero outdoor light (day).
    pub ambient_min: f32,
    /// Ambient brightness ceiling at full outdoor light (day).
    pub ambient_max: f32,
    /// Multiplier applied to Ambient brightness during evening wind-down
    /// (near-black at night; the user wants the room to go dark).
    pub winddown_dim_factor: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            idle_to_ambient: Duration::from_secs(3 * 60),
            day_idle_to_off: Duration::from_secs(150 * 60), // ~2.5 h
            night_idle_to_off: Duration::from_secs(30 * 60), // ~30 m
            disconnect_grace: Duration::from_secs(2 * 60),
            winddown_start: ClockMinutes::at(22, 0),
            hard_off_start: ClockMinutes::at(0, 0),
            hard_off_end: ClockMinutes::at(7, 0),
            ambient_min: 0.04,
            ambient_max: 0.45,
            winddown_dim_factor: 0.30,
        }
    }
}

impl Config {
    fn is_hard_off(&self, now: ClockMinutes) -> bool {
        (self.hard_off_start.raw()..self.hard_off_end.raw()).contains(&now.raw())
    }

    fn is_winddown(&self, now: ClockMinutes) -> bool {
        // Not hard-off, and at/after the evening short-leash start.
        !self.is_hard_off(now) && now.raw() >= self.winddown_start.raw()
    }

    fn idle_leash(&self, now: ClockMinutes) -> Duration {
        if self.is_winddown(now) {
            self.night_idle_to_off
        } else {
            self.day_idle_to_off
        }
    }

    /// Adaptive-dim Ambient brightness. Monotonic in outdoor light;
    /// strictly dimmer during wind-down at equal outdoor light; bounded.
    #[must_use]
    pub fn ambient_brightness(&self, now: ClockMinutes, outdoor: f32) -> Brightness {
        let o = outdoor.clamp(0.0, 1.0);
        let base = self.ambient_min + (self.ambient_max - self.ambient_min) * o;
        let scaled = if self.is_winddown(now) {
            base * self.winddown_dim_factor
        } else {
            base
        };
        Brightness::new(scaled)
    }
}

/// A side-effect the host (axum daemon / Bevy) must apply. Power is
/// emitted as an action (the engine *decides* autonomously; the adapter
/// *applies*) so the engine stays pure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// Drive the TV smart-plug (autonomous power).
    SetTvPower(bool),
    /// Paint this state (the host renders Kiosk/Content/Ambient).
    Show(DisplayState),
}

/// Result of one tick.
#[derive(Debug, Clone)]
pub struct StepOutcome {
    pub state: DisplayState,
    pub actions: Vec<Action>,
}

/// A menu tile in the Kiosk-state retro menu (Slice 3 visual layer).
/// The canonical six per design § 13.1: Sleep replaces the original
/// design-doc Settings (Sleep paired with engine `ForceOff` is more
/// useful at the moment of interaction than a meta-config tile).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuItem {
    Games,
    Music,
    Lights,
    Watch,
    Sensors,
    Sleep,
}

impl MenuItem {
    /// Slice-3 cursor fallback when the engine emits no confident
    /// prediction. Per Vision: *"most often watch something."* Renderer
    /// uses this whenever `KioskHint::cursor == None`.
    #[must_use]
    pub const fn default_fallback() -> Self {
        Self::Watch
    }
}

/// Single-line content offer for the Kiosk-state ribbon. `text` is the
/// user-facing string the renderer paints (e.g. *Resume "The Boys"
/// S04E01*). The action surface (what pressing `[A]` does) is wired in
/// a later sub-slice (3e) once the daemon polls last-watched media
/// state. Slice 3a ships the type; offers remain `None` until then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RibbonOffer {
    pub text: String,
}

/// What to render in the Kiosk state beyond just "show the menu" — the
/// engine's two small Kiosk-state outputs per design § 1 ("stable frame,
/// context-filled"). Renderer reads it only when in Kiosk state;
/// computing it in other states is harmless (cheap pure function).
///
/// `cursor == None` ⇒ no confident prediction; renderer falls back to
/// `MenuItem::default_fallback()` (= Watch).
/// `ribbon == None` ⇒ silent ribbon. An empty ribbon never trains
/// distrust; a weak guess shown anyway does (design § 1: "confident or
/// quiet, never noisy").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KioskHint {
    pub cursor: Option<MenuItem>,
    pub ribbon: Option<RibbonOffer>,
}

/// The TV power port. Slice 1 ships `SimTvPower`; the real
/// `HaSmartPlugTvPower` (HA REST) is a Slice-2 adapter — same trait.
pub trait TvPower {
    fn set_power(&mut self, on: bool);
    fn is_on(&self) -> bool;
}

/// In-memory `TvPower` recorder for deterministic tests (and a dev stub).
#[derive(Debug, Default, Clone)]
pub struct SimTvPower {
    on: bool,
    /// Every power level the engine drove, in order.
    pub history: Vec<bool>,
}

impl TvPower for SimTvPower {
    fn set_power(&mut self, on: bool) {
        self.on = on;
        self.history.push(on);
    }

    fn is_on(&self) -> bool {
        self.on
    }
}

/// The context engine. Carries the current state + the sticky manual
/// override; `step` is the only mutator.
#[derive(Debug, Clone)]
pub struct Engine {
    state: DisplayState,
    manual: Option<Manual>,
    cfg: Config,
}

impl Engine {
    #[must_use]
    pub fn new(cfg: Config) -> Self {
        Self {
            state: DisplayState::Off,
            manual: None,
            cfg,
        }
    }

    #[must_use]
    pub fn state(&self) -> DisplayState {
        self.state
    }

    /// Pure decision given the carried manual override (precedence: design
    /// doc § 4 — first match wins).
    fn decide(&self, i: &Inputs) -> DisplayState {
        let cfg = &self.cfg;

        // 1. Post-midnight hard-off floor — "off no matter what" (rides
        //    the bedroom-lights floor; one room winds down together).
        if cfg.is_hard_off(i.now) {
            return DisplayState::Off;
        }

        // 2. Manual override dominates inferred state.
        match self.manual {
            Some(Manual::ForceOff) => return DisplayState::Off,
            Some(Manual::ForceOn) => {
                return match i.media {
                    Media::Video => DisplayState::Content(Activity::Video),
                    Media::Game => DisplayState::Content(Activity::Game),
                    Media::Music | Media::None => DisplayState::Kiosk,
                };
            }
            None => {}
        }

        // 3. Content engagement — never shortened by the night leash
        //    (the passive guardrail; design doc § 7).
        match i.media {
            Media::Video => return DisplayState::Content(Activity::Video),
            Media::Game => return DisplayState::Content(Activity::Game),
            Media::Music => {
                // 4. Music beats present-idle: TV off while music plays,
                //    UNLESS actively using the controller (then Kiosk).
                let active =
                    i.controller_connected && i.since_controller_input < cfg.idle_to_ambient;
                return if active {
                    DisplayState::Kiosk
                } else {
                    DisplayState::Off
                };
            }
            Media::None => {}
        }

        // 5. Controller-presence/idle oracle.
        if i.controller_connected {
            if i.since_controller_input < cfg.idle_to_ambient {
                DisplayState::Kiosk
            } else if i.since_controller_input < cfg.idle_leash(i.now) {
                DisplayState::Ambient(cfg.ambient_brightness(i.now, i.outdoor_brightness))
            } else {
                DisplayState::Off
            }
        } else {
            // 6. Disconnected: brief grace as Ambient, then OFF.
            if i.since_controller_input < cfg.disconnect_grace {
                DisplayState::Ambient(cfg.ambient_brightness(i.now, i.outdoor_brightness))
            } else {
                DisplayState::Off
            }
        }
    }

    /// Advance one tick. Updates the sticky manual override, recomputes
    /// the state, and emits power + render actions on change.
    pub fn step(&mut self, i: &Inputs) -> StepOutcome {
        // Sticky manual: a fresh press sets it; a fresh controller
        // interaction or the hard-off floor clears it (new intent /
        // floor reset).
        if let Some(m) = i.manual_press {
            self.manual = Some(m);
        }
        if i.fresh_controller_input || self.cfg.is_hard_off(i.now) {
            self.manual = None;
        }

        let prev = self.state;
        let next = self.decide(i);

        let mut actions = Vec::new();
        if next.powered() != prev.powered() {
            actions.push(Action::SetTvPower(next.powered()));
        }
        if next != prev {
            actions.push(Action::Show(next));
        }
        self.state = next;

        StepOutcome {
            state: next,
            actions,
        }
    }

    /// Predict cursor start tile + ribbon offer for the Kiosk state.
    /// Pure function of `Inputs` + `Config`. Emit `None` whenever not
    /// confident (design § 1: *"confident or quiet, never noisy"*).
    /// Does NOT mutate engine state — safe to call freely each tick.
    ///
    /// Use [`Engine::hint_with_offer`] when the host has a candidate
    /// resume title from the HA media-player poll (Slice 3e).
    #[must_use]
    pub fn hint(&self, i: &Inputs) -> KioskHint {
        self.hint_with_offer(i, None)
    }

    /// Same as [`Engine::hint`] but takes an optional candidate resume
    /// title from outside (Slice 3e: the daemon polls `media_player.
    /// fredriks_tv` and forwards its `media_title` when state is
    /// `paused`/`idle`). The engine still gates confidence — the
    /// ribbon only surfaces when:
    ///   1. A title is supplied (caller's HA data has something to resume)
    ///   2. The host signals no fresh user input recently (cursor-prediction
    ///      precondition — same gate as cursor: respect deliberate nav)
    ///   3. Outside the hard-off window (resume offers when the room is
    ///      winding down for sleep would be noise)
    ///
    /// Conditions 2-3 are deliberately conservative — design § 1's
    /// *"confident-or-quiet"* discipline. The renderer treats `None`
    /// as "leave the ribbon line blank/dim, no `[A]` chip" so the
    /// silent state never trains distrust.
    #[must_use]
    pub fn hint_with_offer(&self, i: &Inputs, resumable_title: Option<&str>) -> KioskHint {
        KioskHint {
            cursor: self.predict_cursor(i),
            ribbon: self.compute_ribbon(i, resumable_title),
        }
    }

    /// Compute the ribbon offer per the hint_with_offer doc-comment
    /// gates. Pure helper; tested directly.
    fn compute_ribbon(&self, i: &Inputs, resumable_title: Option<&str>) -> Option<RibbonOffer> {
        let title = resumable_title?.trim();
        if title.is_empty() {
            return None;
        }
        // Hard-off window: room is winding down — no resume offer
        // (sleep nudge by gentle absence, design § 7).
        if self.cfg.is_hard_off(i.now) {
            return None;
        }
        // Fresh user input: they're actively navigating; don't paper
        // over with an unrelated offer. Re-emit on the next idle tick.
        if i.fresh_controller_input {
            return None;
        }
        Some(RibbonOffer {
            text: format!("Resume {title}"),
        })
    }

    /// Time-of-day + media driven cursor prediction. Initial heuristic;
    /// thresholds are observation-tunable (Architecture-vs-parameters:
    /// the levers exist; the values are not yet measured-from-use).
    fn predict_cursor(&self, i: &Inputs) -> Option<MenuItem> {
        let cfg = &self.cfg;
        // Hard-off window: Sleep is the obvious tile (room is winding
        // down; mirrors the bedroom-lights hard-off floor).
        if cfg.is_hard_off(i.now) {
            return Some(MenuItem::Sleep);
        }
        // Music playing: Music tile (cursor lands on what's active;
        // likely the user wants to adjust the currently-playing track).
        if matches!(i.media, Media::Music) {
            return Some(MenuItem::Music);
        }
        // Evening wind-down: Watch (Vision: *"most often watch
        // something"*).
        if cfg.is_winddown(i.now) {
            return Some(MenuItem::Watch);
        }
        // Morning daylight (07:00–11:00 default — the 4 h window after
        // the hard-off floor ends): Lights (greet the room;
        // lights-before-games per delight-per-effort roadmap § 7).
        let morning_start = cfg.hard_off_end.raw();
        let morning_end = morning_start.saturating_add(4 * 60);
        if (morning_start..morning_end).contains(&i.now.raw()) {
            return Some(MenuItem::Lights);
        }
        // Otherwise: no confident prediction → renderer falls back to
        // `MenuItem::default_fallback()` (= Watch).
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// Sensible mid-day, controller-connected, just-interacted baseline.
    fn base() -> Inputs {
        Inputs {
            now: ClockMinutes::at(15, 0),
            controller_connected: true,
            since_controller_input: Duration::from_secs(1),
            fresh_controller_input: false,
            media: Media::None,
            outdoor_brightness: 0.6,
            manual_press: None,
        }
    }

    fn eng() -> Engine {
        Engine::new(Config::default())
    }

    #[test]
    fn recent_controller_input_is_kiosk() {
        let mut e = eng();
        assert_eq!(e.step(&base()).state, DisplayState::Kiosk);
    }

    #[test]
    fn connected_idle_becomes_ambient_then_off_by_day_leash() {
        let mut e = eng();
        let mut i = base();
        i.since_controller_input = Duration::from_secs(10 * 60); // > 3m, < 2.5h
        assert!(matches!(e.step(&i).state, DisplayState::Ambient(_)));
        i.since_controller_input = Duration::from_secs(200 * 60); // > 2.5h day leash
        assert_eq!(e.step(&i).state, DisplayState::Off);
    }

    #[test]
    fn night_leash_is_aggressive_day_leash_is_not_same_idle() {
        // 40 min idle: still Ambient in the day window, OFF in wind-down.
        let mut e = eng();
        let mut i = base();
        i.since_controller_input = Duration::from_secs(40 * 60);

        i.now = ClockMinutes::at(15, 0); // day
        assert!(matches!(e.step(&i).state, DisplayState::Ambient(_)));

        i.now = ClockMinutes::at(23, 0); // evening wind-down
        assert_eq!(e.step(&i).state, DisplayState::Off);
    }

    #[test]
    fn content_is_never_shortened_by_the_night_leash_guardrail() {
        // Video playing, deep into the evening, very long since input —
        // engagement must NOT be cut. (Design § 7 passive guardrail.)
        let mut e = eng();
        let mut i = base();
        i.media = Media::Video;
        i.now = ClockMinutes::at(23, 30);
        i.since_controller_input = Duration::from_secs(180 * 60);
        assert_eq!(e.step(&i).state, DisplayState::Content(Activity::Video));
    }

    #[test]
    fn hard_off_floor_dominates_everything() {
        let mut e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(3, 0); // inside 00:00–07:00 floor
        i.media = Media::Video; // would otherwise be Content
        i.manual_press = Some(Manual::ForceOn); // even an explicit ForceOn
        assert_eq!(e.step(&i).state, DisplayState::Off);
    }

    #[test]
    fn music_with_no_active_controller_turns_tv_off() {
        let mut e = eng();
        let mut i = base();
        i.media = Media::Music;
        i.since_controller_input = Duration::from_secs(30 * 60); // idle
        assert_eq!(e.step(&i).state, DisplayState::Off);
    }

    #[test]
    fn music_but_actively_controllering_stays_kiosk() {
        let mut e = eng();
        let mut i = base();
        i.media = Media::Music;
        i.since_controller_input = Duration::from_secs(2); // active
        assert_eq!(e.step(&i).state, DisplayState::Kiosk);
    }

    #[test]
    fn manual_force_off_overrides_content_until_fresh_input_clears_it() {
        let mut e = eng();
        let mut i = base();
        i.media = Media::Video;
        i.manual_press = Some(Manual::ForceOff);
        assert_eq!(e.step(&i).state, DisplayState::Off); // override beats Content

        i.manual_press = None;
        assert_eq!(e.step(&i).state, DisplayState::Off); // sticky

        i.fresh_controller_input = true; // new deliberate intent clears it
        assert_eq!(e.step(&i).state, DisplayState::Content(Activity::Video));
    }

    #[test]
    fn manual_force_on_shows_content_if_playing_else_kiosk() {
        let mut e = eng();
        let mut i = base();
        i.since_controller_input = Duration::from_secs(999 * 60); // would be Off
        i.manual_press = Some(Manual::ForceOn);
        assert_eq!(e.step(&i).state, DisplayState::Kiosk);

        let mut e2 = eng();
        let mut i2 = base();
        i2.media = Media::Game;
        i2.manual_press = Some(Manual::ForceOn);
        assert_eq!(e2.step(&i2).state, DisplayState::Content(Activity::Game));
    }

    #[test]
    fn disconnect_grace_holds_ambient_then_off() {
        let mut e = eng();
        let mut i = base();
        i.controller_connected = false;
        i.since_controller_input = Duration::from_secs(30); // within 2m grace
        assert!(matches!(e.step(&i).state, DisplayState::Ambient(_)));
        i.since_controller_input = Duration::from_secs(5 * 60); // past grace
        assert_eq!(e.step(&i).state, DisplayState::Off);
    }

    #[test]
    fn power_action_emitted_only_on_powered_transition() {
        let mut e = eng();
        let mut sim = SimTvPower::default();

        // Off -> Kiosk : power on
        for a in e.step(&base()).actions {
            if let Action::SetTvPower(on) = a {
                sim.set_power(on);
            }
        }
        // Kiosk -> Ambient : still powered, no power action
        let mut i = base();
        i.since_controller_input = Duration::from_secs(10 * 60);
        for a in e.step(&i).actions {
            if let Action::SetTvPower(on) = a {
                sim.set_power(on);
            }
        }
        // Ambient -> Off : power off
        i.since_controller_input = Duration::from_secs(300 * 60);
        for a in e.step(&i).actions {
            if let Action::SetTvPower(on) = a {
                sim.set_power(on);
            }
        }

        assert_eq!(sim.history, vec![true, false]);
        assert!(!sim.is_on());
    }

    #[test]
    fn idempotent_tick_emits_no_actions() {
        let mut e = eng();
        let i = base();
        let first = e.step(&i);
        assert!(!first.actions.is_empty()); // Off -> Kiosk
        let second = e.step(&i);
        assert!(second.actions.is_empty()); // no change
    }

    #[test]
    fn ambient_brightness_monotonic_bounded_and_dimmer_at_night() {
        let c = Config::default();
        let day = ClockMinutes::at(14, 0);
        let night = ClockMinutes::at(23, 0);

        let dark = c.ambient_brightness(day, 0.0).get();
        let bright = c.ambient_brightness(day, 1.0).get();
        assert!(bright > dark, "more outdoor light => brighter");
        assert!((0.0..=1.0).contains(&dark) && (0.0..=1.0).contains(&bright));

        let day_b = c.ambient_brightness(day, 0.7).get();
        let night_b = c.ambient_brightness(night, 0.7).get();
        assert!(night_b < day_b, "wind-down is strictly dimmer");
        assert!(approx(night_b, day_b * c.winddown_dim_factor));

        // clamps even with absurd outdoor input
        assert!((0.0..=1.0).contains(&c.ambient_brightness(day, 9.0).get()));
    }

    #[test]
    fn clock_minutes_wrap_and_raw() {
        assert_eq!(ClockMinutes::at(25, 0).raw(), 60);
        assert_eq!(ClockMinutes::at(23, 59).raw(), 23 * 60 + 59);
    }

    // ─── Slice 3a: KioskHint cursor + ribbon prediction ──────────────

    #[test]
    fn hint_hard_off_predicts_sleep() {
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(3, 0); // 00:00–07:00 hard-off
        let h = e.hint(&i);
        assert_eq!(h.cursor, Some(MenuItem::Sleep));
        assert_eq!(h.ribbon, None); // ribbon silent until Slice 3e wires media polling
    }

    #[test]
    fn hint_music_playing_predicts_music() {
        let e = eng();
        let mut i = base();
        i.media = Media::Music;
        i.now = ClockMinutes::at(15, 0); // midday, music plays
        assert_eq!(e.hint(&i).cursor, Some(MenuItem::Music));
    }

    #[test]
    fn hint_winddown_predicts_watch_per_vision() {
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(22, 30); // ≥22:00 wind-down
        assert_eq!(e.hint(&i).cursor, Some(MenuItem::Watch));
    }

    #[test]
    fn hint_morning_predicts_lights() {
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(8, 0); // 07:00–11:00 morning window
        assert_eq!(e.hint(&i).cursor, Some(MenuItem::Lights));
    }

    #[test]
    fn hint_midday_default_is_none_renderer_falls_back_to_watch() {
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(14, 0); // midday: no confident prediction
        assert_eq!(e.hint(&i).cursor, None);
        // Renderer fallback (asserted at the type level, not by the engine).
        assert_eq!(MenuItem::default_fallback(), MenuItem::Watch);
    }

    #[test]
    fn hint_precedence_hard_off_beats_music() {
        // Music inside the hard-off window: Sleep still wins. The room is
        // winding down; music doesn't override the floor.
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(3, 0);
        i.media = Media::Music;
        assert_eq!(e.hint(&i).cursor, Some(MenuItem::Sleep));
    }

    #[test]
    fn hint_precedence_music_beats_winddown_window() {
        // Music inside wind-down: Music wins. Active intent supersedes
        // time-of-day default.
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(22, 30);
        i.media = Media::Music;
        assert_eq!(e.hint(&i).cursor, Some(MenuItem::Music));
    }

    #[test]
    fn hint_ribbon_silent_when_no_resumable_title_from_host() {
        // Without a host-supplied resume title (Slice 3e: from HA poll),
        // ribbon stays silent across the entire (hour × media) matrix.
        // The engine never invents content — it only gates an offer
        // when the host has something concrete.
        let e = eng();
        for hour in 0..24u16 {
            let mut i = base();
            i.now = ClockMinutes::at(hour, 0);
            for media in [Media::None, Media::Music, Media::Video, Media::Game] {
                i.media = media;
                assert_eq!(
                    e.hint(&i).ribbon,
                    None,
                    "ribbon must stay silent (no title) at hour={hour} media={media:?}"
                );
                // hint_with_offer(None) is the same as hint() — invariant.
                assert_eq!(
                    e.hint_with_offer(&i, None).ribbon,
                    None,
                    "hint_with_offer(None) silent at hour={hour} media={media:?}"
                );
            }
        }
    }

    // ─── Slice 3e: ribbon offer wiring (resume-last-watched from HA) ────

    #[test]
    fn ribbon_emits_resume_offer_when_title_supplied_and_idle() {
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(15, 0); // midday — outside hard-off
        i.fresh_controller_input = false; // user not actively navigating
        let h = e.hint_with_offer(&i, Some("The Boys S04E01"));
        assert_eq!(
            h.ribbon,
            Some(RibbonOffer {
                text: "Resume The Boys S04E01".to_string()
            })
        );
    }

    #[test]
    fn ribbon_silent_during_fresh_user_input() {
        // User pressing buttons right now = they have intent. Don't
        // paper over with an offer. Next idle tick can re-offer.
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(15, 0);
        i.fresh_controller_input = true;
        assert_eq!(
            e.hint_with_offer(&i, Some("Some Movie")).ribbon,
            None,
            "fresh user input must suppress ribbon"
        );
    }

    #[test]
    fn ribbon_silent_during_hard_off_window() {
        // Inside the 00:00–07:00 hard-off floor, resume offers would
        // contradict the sleep-encouragement-by-gentle-absence guardrail.
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(3, 0); // post-midnight
        i.fresh_controller_input = false;
        assert_eq!(
            e.hint_with_offer(&i, Some("Some Movie")).ribbon,
            None,
            "ribbon must stay silent during hard-off window"
        );
    }

    #[test]
    fn ribbon_silent_for_whitespace_only_title() {
        // Defensive: HA can return whitespace-only `media_title` for
        // some apps before/between media. Treat as no offer.
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(15, 0);
        i.fresh_controller_input = false;
        assert_eq!(e.hint_with_offer(&i, Some("   ")).ribbon, None);
        assert_eq!(e.hint_with_offer(&i, Some("")).ribbon, None);
    }

    #[test]
    fn ribbon_offer_text_trims_title_whitespace() {
        // HA can return "  My Show   " with stray spaces from poorly
        // tagged media. Normalize on the way out.
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(15, 0);
        i.fresh_controller_input = false;
        assert_eq!(
            e.hint_with_offer(&i, Some("  My Show   ")).ribbon,
            Some(RibbonOffer {
                text: "Resume My Show".to_string()
            })
        );
    }

    #[test]
    fn ribbon_is_pure_no_state_mutation() {
        // hint_with_offer must not mutate engine state — call it many
        // times with the same inputs and same title; result is stable.
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(15, 0);
        i.fresh_controller_input = false;
        let title = "Show X";
        let h1 = e.hint_with_offer(&i, Some(title));
        let h2 = e.hint_with_offer(&i, Some(title));
        let h3 = e.hint_with_offer(&i, Some(title));
        assert_eq!(h1, h2);
        assert_eq!(h2, h3);
    }

    #[test]
    fn hint_is_pure_no_state_mutation_idempotent() {
        // hint() must NOT mutate self.state or self.manual; calling it
        // many times with the same inputs returns equal results. The
        // engine is intentionally NOT bound `mut` here: hint() takes
        // `&self`, so any need for `mut` would be a regression.
        let e = eng();
        let i = base();
        let state_before = e.state();
        let _ = e.hint(&i);
        let _ = e.hint(&i);
        let _ = e.hint(&i);
        assert_eq!(e.state(), state_before);
        assert_eq!(e.hint(&i), e.hint(&i));
    }

    #[test]
    fn hint_boundary_at_winddown_start_exactly_2200() {
        // is_winddown is inclusive of 22:00 (the >= comparison in Config).
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(22, 0);
        assert_eq!(e.hint(&i).cursor, Some(MenuItem::Watch));
    }

    #[test]
    fn hint_boundary_at_morning_start_exactly_0700() {
        // hard_off_end = 07:00; (07:00..11:00) contains 07:00 → Lights.
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(7, 0);
        assert_eq!(e.hint(&i).cursor, Some(MenuItem::Lights));
    }

    #[test]
    fn hint_boundary_at_morning_end_exactly_1100_is_none() {
        // Morning end is exclusive (11:00 ∉ [07:00, 11:00)) → None midday.
        let e = eng();
        let mut i = base();
        i.now = ClockMinutes::at(11, 0);
        assert_eq!(e.hint(&i).cursor, None);
    }
}
