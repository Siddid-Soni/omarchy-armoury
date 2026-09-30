//! NumberPad behaviour as a pure state machine: touches and clock ticks in, device actions out.
use super::layout::{level_byte, Hit, LIGHT_OFF, LIGHT_ON};
use super::mt::Touch;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action { Grab(bool), Light(u8), Key { code: u16, down: bool }, EnsureNumlock }

#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub hold: Duration,
    pub idle: Option<Duration>,
    pub start_level: u8,
    pub repeat_delay: Option<Duration>,
    /// After the first repeat, a resting finger repeats its key this often.
    pub repeat_every: Duration,
}

pub struct Pad {
    on: bool,
    allowed: bool,
    level: u8,
    lit: bool,
    /// Key under the resting finger, and when it next repeats (keys are typed as
    /// press+release, never held, so the desktop's own fast repeat never starts).
    repeat: Option<(u16, Instant)>,
    /// The current touch started in the right icon at this time (and hasn't left it).
    hold_since: Option<Instant>,
    hold_fired: bool,
    left_tap: bool,
    /// The current touch only woke the backlight: it types nothing.
    waking: bool,
    finger_down: bool,
    last_touch: Instant,
}

impl Pad {
    pub fn new() -> Self {
        Self { on: false, allowed: false, level: 8, lit: false, repeat: None, hold_since: None, hold_fired: false,
               left_tap: false, waking: false, finger_down: false, last_touch: Instant::now() }
    }

    pub fn is_on(&self) -> bool { self.on }

    /// When the resting finger's key repeats next (the worker wakes for it; repeats are
    /// finer than its 100 ms tick).
    pub fn next_repeat(&self) -> Option<Instant> {
        if self.on && self.finger_down { self.repeat.map(|(_, t)| t) } else { None }
    }

    /// Touchpad enabled (or allowed while off) and armouryd active; turning false while on turns it off.
    pub fn set_allowed(&mut self, allowed: bool) -> Vec<Action> {
        self.allowed = allowed;
        if !allowed && self.on { self.turn_off() } else { Vec::new() }
    }

    pub fn set_on(&mut self, on: bool, s: &Settings, now: Instant) -> Vec<Action> {
        match (on, self.on) {
            (true, false) if self.allowed => self.turn_on(s, now),
            (false, true) => self.turn_off(),
            _ => Vec::new(),
        }
    }

    fn turn_on(&mut self, s: &Settings, now: Instant) -> Vec<Action> {
        self.on = true;
        self.lit = true;
        self.level = s.start_level.clamp(1, 8);
        self.last_touch = now;
        // "on" first: the level byte alone leaves the pad at its previous brightness
        vec![Action::Grab(true), Action::Light(LIGHT_ON), Action::Light(level_byte(self.level)), Action::EnsureNumlock]
    }

    fn turn_off(&mut self) -> Vec<Action> {
        let mut a = Vec::new();
        self.repeat = None;
        self.on = false;
        self.lit = false;
        a.push(Action::Light(LIGHT_OFF));
        a.push(Action::Grab(false));
        a
    }

    pub fn touch(&mut self, t: Touch, hit: Hit, s: &Settings, now: Instant) -> Vec<Action> {
        let mut a = Vec::new();
        match t {
            Touch::Down { .. } => {
                self.finger_down = true;
                self.last_touch = now;
                self.hold_fired = false;
                self.hold_since = (hit == Hit::RightIcon).then_some(now);
                if self.on && !self.lit {
                    self.lit = true;
                    self.waking = true;
                    a.push(Action::Light(LIGHT_ON));
                    a.push(Action::Light(level_byte(self.level)));
                    return a;
                }
                if !self.on { return a; }
                match hit {
                    Hit::Key(code) => {
                        a.push(Action::Key { code, down: true });
                        a.push(Action::Key { code, down: false });
                        self.repeat = s.repeat_delay.map(|d| (code, now + d));
                    }
                    Hit::LeftIcon => self.left_tap = true,
                    _ => {}
                }
            }
            Touch::Move { .. } => {
                self.last_touch = now;
                if hit != Hit::RightIcon { self.hold_since = None; }
                if hit != Hit::LeftIcon { self.left_tap = false; }
                // a finger that slides off its key stops repeating it
                if matches!(self.repeat, Some((code, _)) if hit != Hit::Key(code)) { self.repeat = None; }
            }
            Touch::Up => {
                self.finger_down = false;
                self.last_touch = now;
                self.hold_since = None;
                self.waking = false;
                self.repeat = None;
                if std::mem::take(&mut self.left_tap) && self.on {
                    self.level = self.level % 8 + 1;
                    a.push(Action::Light(level_byte(self.level)));
                }
            }
        }
        a
    }

    pub fn tick(&mut self, s: &Settings, now: Instant) -> Vec<Action> {
        if let Some(t) = self.hold_since {
            if !self.hold_fired && now.duration_since(t) >= s.hold {
                self.hold_fired = true;
                self.hold_since = None;
                return if self.on { self.turn_off() } else if self.allowed { self.turn_on(s, now) } else { Vec::new() };
            }
        }
        if let Some((code, next)) = self.repeat {
            if self.on && self.finger_down && now >= next {
                self.repeat = Some((code, next + s.repeat_every));
                return vec![Action::Key { code, down: true }, Action::Key { code, down: false }];
            }
        }
        if let Some(idle) = s.idle {
            if self.on && self.lit && !self.finger_down && now.duration_since(self.last_touch) >= idle {
                self.lit = false;
                return vec![Action::Light(LIGHT_OFF)];
            }
        }
        Vec::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::numpad::layout::{Hit, LIGHT_OFF, LIGHT_ON, level_byte};
    use crate::features::numpad::mt::Touch;
    use std::time::{Duration, Instant};
    fn s() -> Settings { Settings { hold: Duration::from_millis(1000), idle: Some(Duration::from_secs(60)), start_level: 8, repeat_delay: Some(Duration::from_millis(600)), repeat_every: Duration::from_millis(100) } }
    fn ms(t0: Instant, n: u64) -> Instant { t0 + Duration::from_millis(n) }
    fn hold_icon(p: &mut Pad, t0: Instant) -> Vec<Action> {
        let mut a = p.touch(Touch::Down { x: 4000, y: 50 }, Hit::RightIcon, &s(), t0);
        a.extend(p.tick(&s(), ms(t0, 1001)));
        a
    }

    #[test]
    fn hold_turns_on_at_start_brightness_with_grab_and_numlock() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        let a = hold_icon(&mut p, t0);
        // "on" first, then the level: the level byte alone keeps the pad's previous brightness
        assert_eq!(a, [Action::Grab(true), Action::Light(LIGHT_ON), Action::Light(level_byte(8)), Action::EnsureNumlock]);
        assert!(p.is_on());
    }

    #[test]
    fn short_touch_on_icon_does_nothing() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        p.touch(Touch::Down { x: 4000, y: 50 }, Hit::RightIcon, &s(), t0);
        assert!(p.touch(Touch::Up, Hit::None, &s(), ms(t0, 300)).is_empty());
        assert!(p.tick(&s(), ms(t0, 2000)).is_empty());
        assert!(!p.is_on());
    }

    #[test]
    fn hold_fires_once_per_touch() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        hold_icon(&mut p, t0);
        assert!(p.tick(&s(), ms(t0, 2500)).is_empty(), "still the same touch: no toggle back off");
        assert!(p.is_on());
    }

    #[test]
    fn keys_press_on_down_release_on_up_only_while_on() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        assert!(p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(72), &s(), t0).is_empty(), "off: no keys");
        p.touch(Touch::Up, Hit::None, &s(), t0);
        hold_icon(&mut p, t0);
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        // typed once at touch (never held down, so the desktop's own fast repeat can't start)
        assert_eq!(p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(72), &s(), ms(t0, 1200)), [Action::Key { code: 72, down: true }, Action::Key { code: 72, down: false }]);
        assert!(p.touch(Touch::Move { x: 900, y: 900 }, Hit::Key(80), &s(), ms(t0, 1250)).is_empty(), "moving doesn't retype");
        assert!(p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1300)).is_empty());
    }

    #[test]
    fn left_icon_tap_steps_brightness() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        hold_icon(&mut p, t0);
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        p.touch(Touch::Down { x: 10, y: 10 }, Hit::LeftIcon, &s(), ms(t0, 1200));
        assert_eq!(p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1300)), [Action::Light(level_byte(1))], "8 wraps to 1");
    }

    #[test]
    fn idle_goes_dark_and_waking_touch_types_nothing() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        hold_icon(&mut p, t0);
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        assert_eq!(p.tick(&s(), ms(t0, 1100 + 60_000)), [Action::Light(LIGHT_OFF)]);
        assert!(p.is_on(), "still on (grabbed), just dark");
        assert_eq!(p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(72), &s(), ms(t0, 70_000)), [Action::Light(LIGHT_ON), Action::Light(level_byte(8))]);
        assert!(p.tick(&s(), ms(t0, 71_000)).is_empty(), "the waking touch doesn't repeat either");
        assert!(p.touch(Touch::Up, Hit::None, &s(), ms(t0, 71_100)).is_empty());
        assert_eq!(p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(72), &s(), ms(t0, 71_200)), [Action::Key { code: 72, down: true }, Action::Key { code: 72, down: false }]);
    }

    #[test]
    fn idle_never_when_disabled() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        hold_icon(&mut p, t0);
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        let never = Settings { idle: None, ..s() };
        assert!(p.tick(&never, ms(t0, 10_000_000)).is_empty());
    }

    #[test]
    fn hold_ignored_when_not_allowed_and_disallow_turns_off() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        assert!(hold_icon(&mut p, t0).is_empty(), "touchpad off: the hold does nothing");
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        p.set_allowed(true);
        hold_icon(&mut p, ms(t0, 2000));
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 3100));
        p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(72), &s(), ms(t0, 3200)); // finger resting on a key
        assert_eq!(p.set_allowed(false), [Action::Light(LIGHT_OFF), Action::Grab(false)]);
        assert!(p.tick(&s(), ms(t0, 5000)).is_empty(), "no repeat after turning off");
        assert!(!p.is_on());
    }

    #[test]
    fn hold_again_turns_off_releasing_nothing_stuck() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        hold_icon(&mut p, t0);
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        let a = hold_icon(&mut p, ms(t0, 2000));
        assert_eq!(a, [Action::Light(LIGHT_OFF), Action::Grab(false)]);
    }

    #[test]
    fn held_key_repeats_after_the_delay() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        hold_icon(&mut p, t0);
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(80), &s(), ms(t0, 2000));
        assert!(p.tick(&s(), ms(t0, 2500)).is_empty(), "not before 600 ms");
        let press = [Action::Key { code: 80, down: true }, Action::Key { code: 80, down: false }];
        assert_eq!(p.tick(&s(), ms(t0, 2600)), press);
        assert!(p.tick(&s(), ms(t0, 2650)).is_empty());
        assert_eq!(p.tick(&s(), ms(t0, 2700)), press, "then every 100 ms");
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 2750));
        assert!(p.tick(&s(), ms(t0, 3000)).is_empty(), "stops on lift");
        let no_repeat = Settings { repeat_delay: None, ..s() };
        p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(80), &no_repeat, ms(t0, 4000));
        assert!(p.tick(&no_repeat, ms(t0, 9000)).is_empty(), "repeat off");
    }

    #[test]
    fn repeat_rate_is_a_setting() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        hold_icon(&mut p, t0);
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        let fast = Settings { repeat_every: Duration::from_millis(25), ..s() }; // 40/s
        p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(80), &fast, ms(t0, 2000));
        assert_eq!(p.next_repeat(), Some(ms(t0, 2600)));
        assert_eq!(p.tick(&fast, ms(t0, 2600)).len(), 2);
        assert_eq!(p.next_repeat(), Some(ms(t0, 2625)), "the worker wakes for this, not its 100 ms tick");
        p.touch(Touch::Up, Hit::None, &fast, ms(t0, 2630));
        assert_eq!(p.next_repeat(), None);
    }

    #[test]
    fn sliding_off_the_key_stops_its_repeat() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        hold_icon(&mut p, t0);
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(80), &s(), ms(t0, 2000));
        assert!(p.touch(Touch::Move { x: 2, y: 2 }, Hit::Key(80), &s(), ms(t0, 2100)).is_empty(), "small move on the same key");
        p.touch(Touch::Move { x: 900, y: 900 }, Hit::Key(81), &s(), ms(t0, 2200)); // slid onto another key
        assert!(p.tick(&s(), ms(t0, 3000)).is_empty(), "no repeat of the key it left");
        assert_eq!(p.next_repeat(), None);
    }
}
