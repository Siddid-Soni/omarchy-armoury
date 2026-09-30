//! Owns the NumberPad devices: reads the touchpad, drives the Pad state machine, and
//! applies its actions (grab, backlight, virtual keys). Active mode only; any device error
//! turns everything off (grab released with the fd) and the touchpad is reopened later.
use super::layout::{backlight_packet, layout_for, Area, Hit, Layout, KEY_NUMLOCK};
use super::mt::{MtDecoder, Raw, Touch};
use super::pad::{Action, Pad, Settings};
use crate::config::NumpadConfig;
use crate::hw::hypr::Hypr;
use armoury_proto::NumpadState;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

pub enum Cmd { Env { active: bool, touchpad_enabled: bool }, Set(bool), Config(NumpadConfig) }

#[async_trait::async_trait]
pub trait NumpadIo: Send {
    async fn next(&mut self) -> std::io::Result<Raw>;
    fn area(&self) -> Area;
    fn layout(&self) -> &'static Layout;
    fn grab(&mut self, on: bool) -> std::io::Result<()>;
    fn light(&mut self, v: u8) -> std::io::Result<()>;
    fn key(&mut self, code: u16, down: bool) -> std::io::Result<()>;
}

/// `kb`: the keyboard's (repeat delay ms, repeats per second), used where the NumberPad's own are 0.
fn settings(c: &NumpadConfig, kb: (u32, u32)) -> Settings {
    let rate = if c.repeat_rate_hz > 0 { c.repeat_rate_hz } else { kb.1 }.max(1);
    let delay = if c.repeat_delay_ms > 0 { c.repeat_delay_ms } else { kb.0 };
    Settings {
        hold: Duration::from_millis(c.hold_ms as u64),
        idle: (c.idle_dim_secs > 0).then(|| Duration::from_secs(c.idle_dim_secs as u64)),
        start_level: c.start_brightness,
        repeat_delay: c.key_repeat.then(|| Duration::from_millis(delay as u64)),
        repeat_every: Duration::from_millis((1000 / rate).max(1) as u64),
    }
}

const TICK: Duration = Duration::from_millis(100);
/// More device failures than this within a minute: stop (reported as unavailable).
const MAX_FAILURES_PER_MIN: usize = 3;

pub async fn run_worker(
    open: impl Fn() -> std::io::Result<Box<dyn NumpadIo>> + Send + 'static,
    mut cmds: mpsc::Receiver<Cmd>,
    state: Arc<Mutex<NumpadState>>,
    hypr: Arc<dyn Hypr>,
    mut cfg: NumpadConfig,
) {
    let (mut active, mut tp_on) = (false, true);
    let mut failures: Vec<tokio::time::Instant> = Vec::new();
    let mut backoff = 0u32;
    loop {
        // wait until armouryd is active before touching the device
        while !active {
            match cmds.recv().await {
                Some(Cmd::Env { active: a, touchpad_enabled: t }) => { active = a; tp_on = t; }
                Some(Cmd::Config(c)) => cfg = c,
                Some(Cmd::Set(_)) => {}
                None => return,
            }
        }
        let mut io = match open() {
            Ok(io) => io,
            Err(e) => {
                eprintln!("armouryd: NumberPad: {e}");
                *state.lock().unwrap() = NumpadState::Unavailable;
                tokio::time::sleep(crate::features::keys::backoff(backoff)).await;
                backoff += 1;
                continue;
            }
        };
        backoff = 0;
        *state.lock().unwrap() = NumpadState::Off;
        // the keyboard's repeat delay and rate (NumberPad repeat follows them unless set);
        // Hyprland's defaults (600 ms, 25/s) if it can't say
        let mut kb = keyboard_repeat(&*hypr, (600, 25)).await;
        let mut pad = Pad::new();
        let mut mt = MtDecoder::default();
        let area = io.area();
        let layout = io.layout();
        let _ = pad.set_allowed(tp_on || cfg.allow_when_touchpad_off); // a fresh pad is off: this only records the rule
        let mut tick = tokio::time::interval(TICK);
        let result: std::io::Result<()> = loop {
            let actions = tokio::select! {
                ev = io.next() => match ev {
                    Err(e) => break Err(e),
                    Ok(raw) => match mt.feed(raw) {
                        Some(t) => {
                            let (x, y) = match t { Touch::Down { x, y } | Touch::Move { x, y } => (x, y), Touch::Up => (0, 0) };
                            let hit = if matches!(t, Touch::Up) { Hit::None } else { layout.hit(&area, x, y) };
                            pad.touch(t, hit, &settings(&cfg, kb), tokio::time::Instant::now().into_std())
                        }
                        None => Vec::new(),
                    },
                },
                _ = tick.tick() => pad.tick(&settings(&cfg, kb), tokio::time::Instant::now().into_std()),
                // a resting finger's key repeats on its own schedule, finer than the tick
                _ = async {
                    match pad.next_repeat() {
                        Some(t) => tokio::time::sleep_until(tokio::time::Instant::from_std(t)).await,
                        None => std::future::pending().await,
                    }
                } => pad.tick(&settings(&cfg, kb), tokio::time::Instant::now().into_std()),
                cmd = cmds.recv() => match cmd {
                    None => break Ok(()),
                    Some(Cmd::Env { active: a, touchpad_enabled: t }) => {
                        active = a; tp_on = t;
                        if !active { break Ok(()); }
                        pad.set_allowed(tp_on || cfg.allow_when_touchpad_off)
                    }
                    Some(Cmd::Config(c)) => {
                        cfg = c;
                        kb = keyboard_repeat(&*hypr, kb).await;
                        pad.set_allowed(tp_on || cfg.allow_when_touchpad_off)
                    }
                    Some(Cmd::Set(on)) => pad.set_on(on, &settings(&cfg, kb), tokio::time::Instant::now().into_std()),
                },
            };
            if let Err(e) = apply(&mut *io, &*hypr, actions).await { break Err(e); }
            *state.lock().unwrap() = if pad.is_on() { NumpadState::On } else { NumpadState::Off };
        };
        // leaving: everything off; dropping `io` closes the fd, which also ends any grab
        let _ = apply(&mut *io, &*hypr, pad.set_allowed(false)).await;
        drop(io);
        *state.lock().unwrap() = NumpadState::Unavailable;
        if let Err(e) = result {
            eprintln!("armouryd: NumberPad device: {e}");
            let now = tokio::time::Instant::now();
            failures.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
            failures.push(now);
            if failures.len() > MAX_FAILURES_PER_MIN {
                eprintln!("armouryd: NumberPad stopped after repeated device failures");
                return;
            }
            tokio::time::sleep(crate::features::keys::backoff(0)).await;
        }
    }
}

async fn keyboard_repeat(hypr: &dyn Hypr, fallback: (u32, u32)) -> (u32, u32) {
    (hypr.repeat_delay().await.unwrap_or(fallback.0), hypr.repeat_rate().await.unwrap_or(fallback.1))
}

/// Backlight failures are logged, not fatal (the NumberPad works unlit); grab / key failures end the session.
async fn apply(io: &mut dyn NumpadIo, hypr: &dyn Hypr, actions: Vec<Action>) -> std::io::Result<()> {
    for a in actions {
        match a {
            Action::Grab(on) => io.grab(on)?,
            Action::Light(v) => if let Err(e) = io.light(v) { eprintln!("armouryd: NumberPad backlight: {e}"); },
            Action::Key { code, down } => io.key(code, down)?,
            Action::EnsureNumlock => {
                if hypr.numlock().await.ok() == Some(false) {
                    io.key(KEY_NUMLOCK, true)?;
                    io.key(KEY_NUMLOCK, false)?;
                }
            }
        }
    }
    Ok(())
}

use evdev::{uinput::VirtualDevice, AbsoluteAxisCode, AttributeSet, EventType, InputEvent, KeyCode};
use evdev::raw_stream::RawDevice;
use i2cdev::core::{I2CMessage, I2CTransfer};
use i2cdev::linux::{LinuxI2CBus, LinuxI2CMessage};

const TOUCHPAD_PREFIXES: &[&str] = &["ASUE", "ASUF", "ASUP", "ELAN"];

pub struct EvdevIo {
    events: evdev::raw_stream::EventStream,
    uinput: VirtualDevice,
    i2c: Option<LinuxI2CBus>,
    addr: u16,
    area: Area,
    layout: &'static Layout,
}

/// Finds the NumberPad touchpad (by name), its i2c bus, and sets up the virtual keyboard.
pub fn open_real() -> std::io::Result<EvdevIo> {
    let (path, name) = std::fs::read_dir("/sys/class/input")?.flatten()
        .filter_map(|e| { let n = e.file_name().into_string().ok()?; n.starts_with("event").then_some(n) })
        .find_map(|n| {
            let name = std::fs::read_to_string(format!("/sys/class/input/{n}/device/name")).ok()?.trim().to_string();
            (name.ends_with("Touchpad") && TOUCHPAD_PREFIXES.iter().any(|p| name.starts_with(p))).then(|| (format!("/dev/input/{n}"), name))
        })
        .ok_or_else(|| std::io::Error::other("no NumberPad touchpad"))?;
    let layout = layout_for(&name).ok_or_else(|| std::io::Error::other(format!("no NumberPad layout for {name}")))?;
    let dev = RawDevice::open(&path)?;
    let abs: Vec<_> = dev.get_absinfo()?.collect();
    let get = |code: AbsoluteAxisCode| abs.iter().find(|(c, _)| *c == code).map(|(_, i)| (i.minimum(), i.maximum()));
    let (minx, maxx) = get(AbsoluteAxisCode::ABS_MT_POSITION_X).ok_or_else(|| std::io::Error::other("touchpad has no X"))?;
    let (miny, maxy) = get(AbsoluteAxisCode::ABS_MT_POSITION_Y).ok_or_else(|| std::io::Error::other("touchpad has no Y"))?;
    let mut keys = AttributeSet::<KeyCode>::new();
    for row in layout.keys { for &k in *row { keys.insert(KeyCode(k)); } }
    keys.insert(KeyCode(KEY_NUMLOCK));
    let uinput = VirtualDevice::builder()?.name("armoury numberpad").with_keys(&keys)?.build()?;
    // the i2c bus is the i2c-N ancestor of the touchpad's i2c device (ACPI name = first word of the evdev name)
    let acpi = name.split(' ').next().unwrap_or_default();
    let bus = std::fs::canonicalize(format!("/sys/bus/i2c/devices/i2c-{acpi}")).ok()
        .and_then(|p| p.parent().and_then(|b| b.file_name()).and_then(|b| b.to_str()).map(|b| format!("/dev/{b}")));
    let i2c = bus.and_then(|b| LinuxI2CBus::new(&b).map_err(|e| eprintln!("armouryd: NumberPad i2c {b}: {e}")).ok());
    Ok(EvdevIo { events: dev.into_event_stream()?, uinput, i2c, addr: super::layout::i2c_address(&name), area: Area { minx, maxx, miny, maxy }, layout })
}

#[async_trait::async_trait]
impl NumpadIo for EvdevIo {
    async fn next(&mut self) -> std::io::Result<Raw> {
        loop {
            let ev = self.events.next_event().await?;
            let raw = match (ev.event_type(), ev.code()) {
                (EventType::ABSOLUTE, c) if c == AbsoluteAxisCode::ABS_MT_SLOT.0 => Raw::Slot(ev.value()),
                (EventType::ABSOLUTE, c) if c == AbsoluteAxisCode::ABS_MT_TRACKING_ID.0 => Raw::TrackingId(ev.value()),
                (EventType::ABSOLUTE, c) if c == AbsoluteAxisCode::ABS_MT_POSITION_X.0 => Raw::X(ev.value()),
                (EventType::ABSOLUTE, c) if c == AbsoluteAxisCode::ABS_MT_POSITION_Y.0 => Raw::Y(ev.value()),
                (EventType::SYNCHRONIZATION, 0) => Raw::Syn,
                _ => continue,
            };
            return Ok(raw);
        }
    }
    fn area(&self) -> Area { self.area }
    fn layout(&self) -> &'static Layout { self.layout }
    fn grab(&mut self, on: bool) -> std::io::Result<()> {
        if on { self.events.device_mut().grab() } else { self.events.device_mut().ungrab() }
    }
    fn light(&mut self, v: u8) -> std::io::Result<()> {
        let Some(bus) = self.i2c.as_mut() else { return Err(std::io::Error::other("no i2c bus")) };
        let data = backlight_packet(v);
        bus.transfer(&mut [LinuxI2CMessage::write(&data).with_address(self.addr)]).map(|_| ()).map_err(std::io::Error::other)
    }
    fn key(&mut self, code: u16, down: bool) -> std::io::Result<()> {
        self.uinput.emit(&[InputEvent::new(EventType::KEY.0, code, down as i32)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::numpad::layout::{G533, LIGHT_OFF, level_byte};
    use crate::hw::fake::FakeHypr;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    /// Fake touchpad: raw events come from a channel; actions are logged.
    struct FakeIo { rx: mpsc::UnboundedReceiver<std::io::Result<Raw>>, log: Arc<Mutex<Vec<String>>> }
    #[async_trait::async_trait]
    impl NumpadIo for FakeIo {
        async fn next(&mut self) -> std::io::Result<Raw> {
            match self.rx.recv().await { Some(r) => r, None => std::future::pending().await }
        }
        fn area(&self) -> Area { Area { minx: 0, maxx: 4036, miny: 0, maxy: 2299 } }
        fn layout(&self) -> &'static Layout { &G533 }
        fn grab(&mut self, on: bool) -> std::io::Result<()> { self.log.lock().unwrap().push(format!("grab {on}")); Ok(()) }
        fn light(&mut self, v: u8) -> std::io::Result<()> { self.log.lock().unwrap().push(format!("light {v:#04x}")); Ok(()) }
        fn key(&mut self, code: u16, down: bool) -> std::io::Result<()> { self.log.lock().unwrap().push(format!("key {code} {down}")); Ok(()) }
    }

    struct Rig { tx: mpsc::UnboundedSender<std::io::Result<Raw>>, cmds: mpsc::Sender<Cmd>, log: Arc<Mutex<Vec<String>>>, state: Arc<Mutex<NumpadState>>, hypr: Arc<FakeHypr> }

    fn start() -> Rig {
        let (tx, rx) = mpsc::unbounded_channel();
        let rx = Arc::new(Mutex::new(Some(rx)));
        let log = Arc::new(Mutex::new(Vec::new()));
        let (ctx, crx) = mpsc::channel(8);
        let state = Arc::new(Mutex::new(NumpadState::Unavailable));
        let hypr = Arc::new(FakeHypr::default());
        let l = log.clone();
        let open = move || -> std::io::Result<Box<dyn NumpadIo>> {
            match rx.lock().unwrap().take() {
                Some(rx) => Ok(Box::new(FakeIo { rx, log: l.clone() })),
                None => Err(std::io::Error::other("no touchpad")),
            }
        };
        tokio::spawn(run_worker(open, crx, state.clone(), hypr.clone(), NumpadConfig::default()));
        Rig { tx, cmds: ctx, log, state, hypr }
    }
    // a closed channel = the worker closed the device (e.g. observe mode): touches go nowhere
    fn touch(r: &Rig, x: i32, y: i32) { for e in [Raw::Slot(0), Raw::TrackingId(1), Raw::X(x), Raw::Y(y), Raw::Syn] { let _ = r.tx.send(Ok(e)); } }
    fn lift(r: &Rig) { for e in [Raw::TrackingId(-1), Raw::Syn] { let _ = r.tx.send(Ok(e)); } }
    async fn settle() { for _ in 0..20 { tokio::task::yield_now().await; } }
    fn log(r: &Rig) -> Vec<String> { r.log.lock().unwrap().clone() }

    #[tokio::test(start_paused = true)]
    async fn hold_on_type_hold_off() {
        let r = start();
        r.cmds.send(Cmd::Env { active: true, touchpad_enabled: true }).await.unwrap();
        *r.hypr.numlock.lock().unwrap() = false;
        settle().await;
        assert_eq!(*r.state.lock().unwrap(), NumpadState::Off);
        touch(&r, 4000, 50); settle().await;
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        lift(&r); settle().await;
        assert_eq!(*r.state.lock().unwrap(), NumpadState::On);
        touch(&r, 1000, 800); lift(&r); settle().await; // "5"
        let l = log(&r);
        assert_eq!(&l[..4], ["grab true", "light 0x01", &format!("light {:#04x}", level_byte(8)), "key 69 true"], "{l:?}");
        assert!(l.contains(&"key 76 true".to_string()) && l.contains(&"key 76 false".to_string()), "{l:?}");
        touch(&r, 4000, 50); settle().await;
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        lift(&r); settle().await;
        assert_eq!(*r.state.lock().unwrap(), NumpadState::Off);
        assert_eq!(&log(&r)[log(&r).len() - 2..], [format!("light {LIGHT_OFF:#04x}"), "grab false".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn observe_mode_turns_it_off_and_ignores_holds() {
        let r = start();
        r.cmds.send(Cmd::Env { active: true, touchpad_enabled: true }).await.unwrap();
        r.cmds.send(Cmd::Set(true)).await.unwrap(); settle().await;
        assert_eq!(*r.state.lock().unwrap(), NumpadState::On);
        r.cmds.send(Cmd::Env { active: false, touchpad_enabled: true }).await.unwrap(); settle().await;
        assert!(log(&r).ends_with(&["grab false".to_string()]), "{:?}", log(&r));
        assert_eq!(*r.state.lock().unwrap(), NumpadState::Unavailable);
        touch(&r, 4000, 50); settle().await;
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await; settle().await;
        assert!(!log(&r).iter().rev().take(1).any(|l| l == "grab true"), "no hold in observe mode");
    }

    #[tokio::test(start_paused = true)]
    async fn device_loss_releases_everything() {
        let r = start();
        r.cmds.send(Cmd::Env { active: true, touchpad_enabled: true }).await.unwrap();
        r.cmds.send(Cmd::Set(true)).await.unwrap(); settle().await;
        r.tx.send(Err(std::io::Error::other("gone"))).unwrap(); settle().await;
        assert!(log(&r).ends_with(&[format!("light {LIGHT_OFF:#04x}"), "grab false".to_string()]), "{:?}", log(&r));
        assert_eq!(*r.state.lock().unwrap(), NumpadState::Unavailable);
    }

    #[tokio::test(start_paused = true)]
    async fn config_change_applies_while_on() {
        let r = start();
        r.cmds.send(Cmd::Env { active: true, touchpad_enabled: true }).await.unwrap();
        r.cmds.send(Cmd::Set(true)).await.unwrap(); settle().await;
        r.cmds.send(Cmd::Config(NumpadConfig { idle_dim_secs: 5, ..NumpadConfig::default() })).await.unwrap(); settle().await;
        tokio::time::sleep(std::time::Duration::from_secs(6)).await; settle().await;
        assert!(log(&r).ends_with(&[format!("light {LIGHT_OFF:#04x}")]), "new idle timeout used: {:?}", log(&r));
        r.cmds.send(Cmd::Env { active: true, touchpad_enabled: false }).await.unwrap(); settle().await;
        assert_eq!(*r.state.lock().unwrap(), NumpadState::Off, "touchpad off → NumberPad off");
    }

    #[tokio::test(start_paused = true)]
    async fn repeat_follows_the_keyboard_rate_between_ticks() {
        let r = start(); // FakeHypr: repeat rate 40/s
        r.cmds.send(Cmd::Env { active: true, touchpad_enabled: true }).await.unwrap();
        r.cmds.send(Cmd::Set(true)).await.unwrap(); settle().await;
        touch(&r, 1000, 800); settle().await; // "5", finger resting
        tokio::time::sleep(std::time::Duration::from_millis(700)).await; settle().await;
        let n = log(&r).iter().filter(|l| *l == "key 76 true").count();
        assert!(n >= 1 + 4, "600 ms delay, then every 25 ms (not every 100 ms tick): {n} presses in {:?}", log(&r));
    }

    #[test]
    fn repeat_settings_follow_the_keyboard_unless_set() {
        let kb = (300, 40); // this machine's input:repeat_delay / repeat_rate
        let d = settings(&NumpadConfig::default(), kb);
        assert_eq!((d.repeat_delay, d.repeat_every), (Some(Duration::from_millis(300)), Duration::from_millis(25)));
        let own = settings(&NumpadConfig { repeat_delay_ms: 800, repeat_rate_hz: 10, ..NumpadConfig::default() }, kb);
        assert_eq!((own.repeat_delay, own.repeat_every), (Some(Duration::from_millis(800)), Duration::from_millis(100)));
        let off = settings(&NumpadConfig { key_repeat: false, ..NumpadConfig::default() }, kb);
        assert_eq!(off.repeat_delay, None);
    }
}
