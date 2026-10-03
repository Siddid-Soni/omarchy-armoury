//! Owns music lighting: runs the capture child, analyses each chunk and writes frames to
//! the keyboard. Active mode only. Every stop (off, failure, handback) puts asusd's effect
//! back. Frames go over USB HID to the keyboard's controller only (no EC/ACPI calls).
use super::analyze::{Analyzer, CHUNK};
use super::perkey::{Frame, Layout, PACKET_LEN, init_packet, keystone_animation_packets, packets};
use super::render::{Lit, render};
use crate::config::MusicConfig;
use armoury_proto::MusicState;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

pub enum Cmd {
    Env { active: bool },
    /// Off stops (effect restored) and acks once the keyboard is back to its effect.
    Set { on: bool, ack: Option<oneshot::Sender<()>> },
    Config(MusicConfig),
    /// Zones on while awake.
    Lit(Lit),
    /// Keyboard backlight dimmed for idle: stop sending frames until it's back.
    Idle(bool),
    /// Handback: stop, restore the effect, and ack, before asusd stops.
    Release(oneshot::Sender<()>),
    /// The Keystone went in: play the firmware's Keystone animation over the music frames.
    KeystoneAnimation,
}

/// Published state: what the snapshot shows.
pub type Status = Arc<Mutex<(MusicState, Option<String>)>>;

#[async_trait::async_trait]
pub trait Capture: Send {
    /// The next CHUNK of mono f32 samples.
    async fn next(&mut self) -> std::io::Result<Vec<f32>>;
}

#[async_trait::async_trait]
pub trait Keyboard: Send {
    async fn write(&mut self, packets: Vec<[u8; PACKET_LEN]>) -> std::io::Result<()>;
}

/// asusd's side of the lighting: which zones are lit, and putting the effect back.
#[async_trait::async_trait]
pub trait Effects: Send + Sync {
    async fn lit(&self) -> anyhow::Result<Lit>;
    async fn restore(&self) -> anyhow::Result<()>;
}

/// Opens the keyboard's hidraw node for direct frames.
pub type OpenKeyboard = Box<dyn Fn() -> std::io::Result<Box<dyn Keyboard>> + Send + Sync>;

/// Band levels for the UI (latest wins); empty while music isn't running.
pub type Bands = tokio::sync::watch::Sender<Vec<f32>>;

pub struct Io {
    /// Where band levels go for the UI (~10 per second).
    pub bands: Option<Bands>,
    pub capture: Box<dyn Fn() -> std::io::Result<Box<dyn Capture>> + Send + Sync>,
    pub keyboard: OpenKeyboard,
    pub effects: Arc<dyn Effects>,
}

/// Fastest frame rate sent to the keyboard (it takes ~45).
const MIN_FRAME_GAP: Duration = Duration::from_millis(30);
/// The whole frame is re-sent this often even when unchanged: the keyboard drops packets.
const REPAINT_EVERY: Duration = Duration::from_secs(5);
/// The boot clock got this far ahead of the monotonic one: the laptop slept, and the firmware
/// drops direct mode on resume, so it is entered again. Never on a timer: `5d bc 00` blanks the
/// keyboard until the next frame (a blink), and frames alone re-enter direct mode with the
/// light bars copying keys.
const SLEPT: Duration = Duration::from_secs(1);

/// Time spent suspended since boot: CLOCK_BOOTTIME counts it, CLOCK_MONOTONIC doesn't.
fn suspended_total() -> Duration {
    use nix::time::{ClockId, clock_gettime};
    let t = |c| clock_gettime(c).map(Duration::from).unwrap_or_default();
    t(ClockId::CLOCK_BOOTTIME).saturating_sub(t(ClockId::CLOCK_MONOTONIC))
}

/// Whether the laptop slept between two `suspended_total` readings.
fn slept(before: Duration, now: Duration) -> bool { now.saturating_sub(before) >= SLEPT }
/// How often band levels go to the UI.
const BANDS_EVERY: Duration = Duration::from_millis(100);
/// More failures than this within a minute: music turns itself off.
const MAX_FAILURES_PER_MIN: usize = 3;

enum End {
    /// Stopped on request (off / release / observe): nothing to report.
    Stopped,
    Failed(String),
}

pub async fn run_worker(io: Io, layout: Option<Layout>, mut cmds: mpsc::Receiver<Cmd>, status: Status, mut cfg: MusicConfig) {
    let mut want = cfg.on;
    let (mut active, mut idle, mut lit_override): (bool, bool, Option<Lit>) = (false, false, None);
    let mut failures: Vec<Instant> = Vec::new();
    let mut failed: Option<String> = None;
    loop {
        let publish = |failed: &Option<String>, active: bool| {
            *status.lock().unwrap() = match (layout.is_some() && active, failed) {
                (false, _) => (MusicState::Unavailable, None),
                (true, Some(e)) => (MusicState::Failed, Some(e.clone())),
                (true, None) => (MusicState::Off, None),
            };
        };
        publish(&failed, active);
        // wait until there is something to run
        while !(active && want && failed.is_none() && layout.is_some()) {
            match cmds.recv().await {
                None => return,
                Some(Cmd::Env { active: a }) => active = a,
                Some(Cmd::Set { on, ack }) => {
                    want = on;
                    if on { failed = None; failures.clear(); }
                    if let Some(ack) = ack { let _ = ack.send(()); }
                }
                Some(Cmd::Config(c)) => cfg = c,
                Some(Cmd::Lit(l)) => lit_override = Some(l),
                Some(Cmd::Idle(i)) => idle = i,
                Some(Cmd::Release(ack)) => { active = false; let _ = ack.send(()); }
                Some(Cmd::KeystoneAnimation) => {}
            }
            publish(&failed, active);
        }
        let layout = layout.as_ref().unwrap();
        *status.lock().unwrap() = (MusicState::On, None);
        let lit = match lit_override { Some(l) => l, None => io.effects.lit().await.unwrap_or_default() };
        let mut ack: Option<oneshot::Sender<()>> = None;
        eprintln!("armouryd: music: started");
        let end = session(&io, layout, &mut cmds, &mut cfg, lit, &mut idle, &mut want, &mut active, &mut lit_override, &mut ack).await;
        if let Some(b) = &io.bands { b.send_if_modified(|v| { let was = !v.is_empty(); v.clear(); was }); }
        match io.effects.restore().await {
            Ok(()) => eprintln!("armouryd: music: stopped, effect restored"),
            Err(e) => eprintln!("armouryd: music: stopped; restoring the lighting effect: {e:#}"),
        }
        if let Some(ack) = ack { let _ = ack.send(()); }
        if let End::Failed(e) = end {
            eprintln!("armouryd: music: {e}");
            let now = Instant::now();
            failures.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
            failures.push(now);
            if failures.len() > MAX_FAILURES_PER_MIN {
                eprintln!("armouryd: music turned off after repeated failures");
                failed = Some(e);
            } else {
                *status.lock().unwrap() = (MusicState::Off, None);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

/// One run: open, then capture → analyse → frame until stopped or failed.
#[allow(clippy::too_many_arguments)]
async fn session(
    io: &Io, layout: &Layout, cmds: &mut mpsc::Receiver<Cmd>, cfg: &mut MusicConfig, mut lit: Lit,
    idle: &mut bool, want: &mut bool, active: &mut bool, lit_override: &mut Option<Lit>, ack: &mut Option<oneshot::Sender<()>>,
) -> End {
    let mut kb = match (io.keyboard)() { Ok(k) => k, Err(e) => return End::Failed(format!("keyboard: {e}")) };
    let mut cap = match (io.capture)() { Ok(c) => c, Err(e) => return End::Failed(format!("audio capture: {e}")) };
    let mut analyzer = Analyzer::new();
    let mut last: Option<Frame> = None;
    let mut last_sent = Instant::now() - MIN_FRAME_GAP;
    let mut need_init = true;
    let mut repaint_at = Instant::now();
    let mut suspended = suspended_total();
    let mut animate = false;
    let mut bands_at: Option<Instant> = None;
    loop {
        tokio::select! {
            chunk = cap.next() => {
                let chunk = match chunk { Ok(c) => c, Err(e) => return End::Failed(format!("audio capture: {e}")) };
                let a = analyzer.feed(&chunk, cfg.sensitivity);
                let now = Instant::now();
                if let Some(b) = &io.bands && bands_at.is_none_or(|t| now.duration_since(t) >= BANDS_EVERY) {
                    b.send_replace(a.bands.to_vec());
                    bands_at = Some(now);
                }
                if *idle || now.duration_since(last_sent) < MIN_FRAME_GAP { continue; }
                let mut out = Vec::new();
                let s = suspended_total();
                if slept(suspended, s) { need_init = true; }
                suspended = s;
                if std::mem::take(&mut need_init) {
                    out.push(init_packet());
                    last = None; // repaint after entering direct mode
                    repaint_at = now;
                } else if now.duration_since(repaint_at) >= REPAINT_EVERY {
                    last = None;
                    repaint_at = now;
                }
                if std::mem::take(&mut animate) {
                    // plays over the stream, which comes back by itself afterwards
                    out.extend(keystone_animation_packets([0, 0, 0])); // the hold colour is unused: the stream comes back
                    last = None;
                }
                let frame = render(layout, &a, cfg, lit);
                if last.as_ref() == Some(&frame) { continue; }
                out.extend(packets(&frame));
                if let Err(e) = kb.write(out).await { return End::Failed(format!("keyboard: {e}")); }
                last = Some(frame);
                last_sent = now;
            }
            cmd = cmds.recv() => match cmd {
                None => return End::Stopped,
                Some(Cmd::Env { active: a }) => { *active = a; if !a { return End::Stopped; } }
                Some(Cmd::Set { on, ack: a }) => {
                    *want = on;
                    if !on { *ack = a; return End::Stopped; }
                    if let Some(a) = a { let _ = a.send(()); }
                }
                Some(Cmd::Config(c)) => { *cfg = c; last = None; }
                Some(Cmd::Lit(l)) => { lit = l; *lit_override = Some(l); last = None; }
                // back from the idle dim: enter direct mode again (the keyboard was dark anyway)
                Some(Cmd::Idle(i)) => { *idle = i; last = None; if !i { need_init = true; } }
                Some(Cmd::Release(a)) => { *active = false; *ack = Some(a); return End::Stopped; }
                Some(Cmd::KeystoneAnimation) => animate = true,
            },
        }
    }
}

// ---- real devices ----

/// `pw-record` capturing whatever plays on the default output; killed when dropped.
pub struct PwRecord {
    _child: tokio::process::Child,
    out: tokio::process::ChildStdout,
}

pub fn open_capture() -> std::io::Result<Box<dyn Capture>> {
    let mut child = tokio::process::Command::new("pw-record")
        .args(["--raw", "--format", "f32", "--rate", "48000", "--channels", "1", "--latency", "20ms",
               "-P", "{ stream.capture.sink = true, node.name = \"armoury-music\", node.description = \"Armoury music lighting\" }", "-"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let out = child.stdout.take().ok_or_else(|| std::io::Error::other("pw-record has no stdout"))?;
    Ok(Box::new(PwRecord { _child: child, out }))
}

#[async_trait::async_trait]
impl Capture for PwRecord {
    async fn next(&mut self) -> std::io::Result<Vec<f32>> {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; CHUNK * 4];
        self.out.read_exact(&mut buf).await.map_err(|e| std::io::Error::new(e.kind(), format!("pw-record stopped: {e}")))?;
        Ok(buf.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
    }
}

nix::ioctl_readwrite_buf!(hid_set_feature, b'H', 0x06, u8);

/// The N-KEY keyboard's hidraw node: an ASUS (0b05) HID device whose report descriptor
/// declares the Aura report (ID 0x5D).
pub struct Hidraw(Arc<std::fs::File>);

pub fn open_keyboard() -> std::io::Result<Box<dyn Keyboard>> {
    let node = std::fs::read_dir("/sys/class/hidraw")?.flatten().find_map(|e| {
        let dev = e.path().join("device");
        let uevent = std::fs::read_to_string(dev.join("uevent")).ok()?;
        let asus = uevent.lines().any(|l| l.starts_with("HID_ID=") && l.to_ascii_uppercase().contains(":00000B05:"));
        let desc = std::fs::read(dev.join("report_descriptor")).ok()?;
        (asus && desc.windows(2).any(|w| w == [0x85, super::perkey::REPORT_ID])).then(|| format!("/dev/{}", e.file_name().to_string_lossy()))
    }).ok_or_else(|| std::io::Error::other("no ASUS Aura keyboard (hidraw)"))?;
    let f = std::fs::OpenOptions::new().read(true).write(true).open(&node)
        .map_err(|e| std::io::Error::new(e.kind(), format!("{node}: {e}")))?;
    Ok(Box::new(Hidraw(Arc::new(f))))
}

#[async_trait::async_trait]
impl Keyboard for Hidraw {
    async fn write(&mut self, packets: Vec<[u8; PACKET_LEN]>) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        let f = self.0.clone();
        // each feature report is a blocking USB control transfer (~2 ms): keep them off the runtime
        tokio::task::spawn_blocking(move || {
            for mut p in packets {
                unsafe { hid_set_feature(f.as_raw_fd(), &mut p) }.map_err(std::io::Error::from)?;
            }
            Ok(())
        }).await.map_err(std::io::Error::other)?
    }
}

/// Lit zones and restore through asusd.
pub struct AsusdEffects(pub Box<dyn crate::hw::Aura>);

#[async_trait::async_trait]
impl Effects for AsusdEffects {
    async fn lit(&self) -> anyhow::Result<Lit> {
        let (_, power, _, _) = crate::hw::with_retry(|| self.0.info()).await?;
        Ok(Lit::from_zones(&crate::features::lighting::power_from_raw(&power)))
    }
    async fn restore(&self) -> anyhow::Result<()> {
        let (mode, _, _, _) = crate::hw::with_retry(|| self.0.info()).await?;
        crate::hw::with_retry(|| self.0.set_mode_data(mode.clone())).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::perkey::layout_for;

    /// Fake capture: chunks come from a channel (a closed channel = no more audio, pending).
    struct FakeCap(mpsc::UnboundedReceiver<std::io::Result<Vec<f32>>>);
    #[async_trait::async_trait]
    impl Capture for FakeCap {
        async fn next(&mut self) -> std::io::Result<Vec<f32>> {
            match self.0.recv().await { Some(c) => c, None => std::future::pending().await }
        }
    }
    struct FakeKb { log: Arc<Mutex<Vec<String>>>, fail: Arc<Mutex<bool>> }
    #[async_trait::async_trait]
    impl Keyboard for FakeKb {
        async fn write(&mut self, packets: Vec<[u8; PACKET_LEN]>) -> std::io::Result<()> {
            if *self.fail.lock().unwrap() { return Err(std::io::Error::other("unplugged")); }
            let init = packets[0] == init_packet();
            let keystone = packets.iter().any(|p| p[1] == 0xB3 && p[3] == 0x0D);
            self.log.lock().unwrap().push(format!("{}{}frame", if init { "init+" } else { "" }, if keystone { "keystone+" } else { "" }));
            Ok(())
        }
    }
    struct FakeFx(Arc<Mutex<Vec<String>>>);
    #[async_trait::async_trait]
    impl Effects for FakeFx {
        async fn lit(&self) -> anyhow::Result<Lit> { Ok(Lit::default()) }
        async fn restore(&self) -> anyhow::Result<()> { self.0.lock().unwrap().push("restore".into()); Ok(()) }
    }

    struct Rig {
        audio: Arc<Mutex<Option<mpsc::UnboundedSender<std::io::Result<Vec<f32>>>>>>,
        cmds: mpsc::Sender<Cmd>, log: Arc<Mutex<Vec<String>>>, status: Status, kb_fail: Arc<Mutex<bool>>,
        bands: tokio::sync::watch::Receiver<Vec<f32>>,
    }

    fn start(on: bool) -> Rig {
        let bands = tokio::sync::watch::Sender::new(Vec::new());
        let log = Arc::new(Mutex::new(Vec::new()));
        let audio = Arc::new(Mutex::new(None));
        let kb_fail = Arc::new(Mutex::new(false));
        let (a, l, f) = (audio.clone(), log.clone(), kb_fail.clone());
        let l2 = log.clone();
        let io = Io {
            bands: Some(bands.clone()),
            capture: Box::new(move || {
                let (tx, rx) = mpsc::unbounded_channel();
                *a.lock().unwrap() = Some(tx);
                Ok(Box::new(FakeCap(rx)) as Box<dyn Capture>)
            }),
            keyboard: Box::new(move || Ok(Box::new(FakeKb { log: l.clone(), fail: f.clone() }) as Box<dyn Keyboard>)),
            effects: Arc::new(FakeFx(l2)),
        };
        let (tx, rx) = mpsc::channel(8);
        let status: Status = Arc::new(Mutex::new((MusicState::Unavailable, None)));
        tokio::spawn(run_worker(io, layout_for("G533ZW"), rx, status.clone(), MusicConfig { on, ..MusicConfig::default() }));
        Rig { audio, cmds: tx, log, status, kb_fail, bands: bands.subscribe() }
    }
    async fn settle() { for _ in 0..30 { tokio::task::yield_now().await; } }
    fn tone(k: usize) -> Vec<f32> { (0..CHUNK).map(|i| 0.5 * ((k * CHUNK + i) as f32 * 0.13).sin()).collect() }
    /// One chunk every 33 ms, like pw-record.
    async fn play(r: &Rig, chunks: impl IntoIterator<Item = Vec<f32>>) {
        for c in chunks {
            if let Some(tx) = r.audio.lock().unwrap().as_ref() { let _ = tx.send(Ok(c)); }
            settle().await;
            tokio::time::sleep(Duration::from_millis(33)).await;
        }
    }
    fn log(r: &Rig) -> Vec<String> { r.log.lock().unwrap().clone() }
    fn state(r: &Rig) -> MusicState { r.status.lock().unwrap().0 }

    #[tokio::test(start_paused = true)]
    async fn runs_only_when_active_and_on() {
        let r = start(true);
        settle().await;
        assert_eq!(state(&r), MusicState::Unavailable, "observe mode");
        r.cmds.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        assert_eq!(state(&r), MusicState::On);
        play(&r, (0..3).map(tone)).await;
        let l = log(&r);
        assert_eq!(l[0], "init+frame", "direct mode entered with the first frame: {l:?}");
        assert!(l.len() >= 2 && l[1] == "frame", "{l:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn direct_mode_is_entered_once_not_on_a_timer() {
        // re-entering blinks the keyboard: 5d bc 00 blanks it until the next frame
        let r = start(true);
        r.cmds.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        play(&r, (0..400).map(tone)).await; // ~13 s
        let l = log(&r);
        assert_eq!(l.iter().filter(|l| l.starts_with("init+")).count(), 1, "{:?}", &l[..5]);
        assert_eq!(l[0], "init+frame");
    }

    #[tokio::test(start_paused = true)]
    async fn an_unchanged_frame_is_still_repainted_every_few_seconds() {
        // the keyboard drops packets: a frame sent once can stay half dark
        let r = start(true);
        r.cmds.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        play(&r, (0..360).map(|_| vec![0.0; CHUNK])).await; // ~12 s of silence: one frame, repeated
        let l = log(&r);
        assert!((3..=4).contains(&l.len()) && l[1..].iter().all(|l| l == "frame"), "first frame, then a repaint every 5 s: {l:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn the_end_of_the_idle_dim_enters_direct_mode_again() {
        let r = start(true);
        r.cmds.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        play(&r, (0..3).map(tone)).await;
        r.cmds.send(Cmd::Idle(true)).await.unwrap(); settle().await;
        play(&r, (3..6).map(tone)).await;
        let n = log(&r).len();
        r.cmds.send(Cmd::Idle(false)).await.unwrap(); settle().await;
        play(&r, (6..9).map(tone)).await;
        let l = log(&r);
        assert_eq!(l[n], "init+frame", "{l:?}");
        assert!(l[n + 1..].iter().all(|l| l == "frame"), "{l:?}");
    }

    #[test]
    fn a_suspend_is_a_jump_of_the_boot_clock() {
        let s = Duration::from_secs;
        assert!(!slept(s(10), s(10)) && !slept(s(10), s(10) + Duration::from_millis(5)), "clock jitter");
        assert!(slept(s(10), s(70)), "a minute asleep");
        assert!(!slept(s(70), s(10)), "never backwards");
        assert!(suspended_total() < s(365 * 24 * 3600), "readable");
    }

    #[tokio::test(start_paused = true)]
    async fn off_restores_the_effect_and_acks() {
        let r = start(true);
        r.cmds.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        play(&r, (0..2).map(tone)).await;
        let (ack, done) = oneshot::channel();
        r.cmds.send(Cmd::Set { on: false, ack: Some(ack) }).await.unwrap();
        done.await.unwrap();
        assert_eq!(log(&r).last().unwrap(), "restore");
        assert_eq!(state(&r), MusicState::Off);
        let n = log(&r).len();
        play(&r, (2..4).map(tone)).await;
        assert_eq!(log(&r).len(), n, "no frames after off");
        r.cmds.send(Cmd::Set { on: true, ack: None }).await.unwrap(); settle().await;
        assert_eq!(state(&r), MusicState::On);
    }

    #[tokio::test(start_paused = true)]
    async fn silence_sends_one_dark_frame() {
        let r = start(true);
        r.cmds.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        play(&r, (0..3).map(tone)).await;
        let before = log(&r).len();
        play(&r, std::iter::repeat_n(vec![0.0; CHUNK], 60)).await; // 2 s of silence
        let during = log(&r).len() - before;
        // falling bars for up to ~0.5 s, then one dark frame; re-init every 5 s at most
        assert!(during <= 20, "{during} frames sent during silence");
        let n = log(&r).len();
        play(&r, std::iter::repeat_n(vec![0.0; CHUNK], 30)).await;
        assert_eq!(log(&r).len(), n, "nothing more while silent");
    }

    #[tokio::test(start_paused = true)]
    async fn idle_pauses_frames() {
        let r = start(true);
        r.cmds.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        play(&r, (0..2).map(tone)).await;
        r.cmds.send(Cmd::Idle(true)).await.unwrap(); settle().await;
        let n = log(&r).len();
        play(&r, (2..10).map(tone)).await;
        assert_eq!(log(&r).len(), n);
        r.cmds.send(Cmd::Idle(false)).await.unwrap(); settle().await;
        play(&r, (10..12).map(tone)).await;
        assert!(log(&r).len() > n);
    }

    #[tokio::test(start_paused = true)]
    async fn release_restores_and_acks_before_handback() {
        let r = start(true);
        r.cmds.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        play(&r, (0..2).map(tone)).await;
        let (ack, done) = oneshot::channel();
        r.cmds.send(Cmd::Release(ack)).await.unwrap();
        done.await.unwrap();
        assert_eq!(log(&r).last().unwrap(), "restore");
        assert_eq!(state(&r), MusicState::Unavailable);
        // still wanted: back on at the next takeover
        r.cmds.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        assert_eq!(state(&r), MusicState::On);
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_failures_turn_it_off_with_the_error() {
        let r = start(true);
        *r.kb_fail.lock().unwrap() = true;
        r.cmds.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        for k in 0..6 {
            play(&r, [tone(k)]).await;
            tokio::time::sleep(Duration::from_millis(1100)).await; settle().await;
        }
        let (s, e) = r.status.lock().unwrap().clone();
        assert_eq!(s, MusicState::Failed);
        assert!(e.unwrap().contains("unplugged"));
        assert!(log(&r).iter().filter(|l| *l == "restore").count() >= 4, "every failed run restores: {:?}", log(&r));
        *r.kb_fail.lock().unwrap() = false;
        r.cmds.send(Cmd::Set { on: true, ack: None }).await.unwrap(); settle().await;
        assert_eq!(state(&r), MusicState::On, "turning it on again retries");
    }

    #[tokio::test(start_paused = true)]
    async fn frame_rate_is_capped() {
        let r = start(true);
        r.cmds.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        // a backlog of 10 chunks arriving at once (pw-record catching up)
        for k in 0..10 { if let Some(tx) = r.audio.lock().unwrap().as_ref() { let _ = tx.send(Ok(tone(k))); } }
        settle().await;
        assert_eq!(log(&r).len(), 1, "one frame per 30 ms, however fast audio arrives: {:?}", log(&r));
    }

    #[tokio::test(start_paused = true)]
    async fn keystone_animation_rides_on_music_frames() {
        let r = start(true);
        r.cmds.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        play(&r, (0..2).map(tone)).await;
        r.cmds.send(Cmd::KeystoneAnimation).await.unwrap(); settle().await;
        play(&r, (2..20).map(tone)).await;
        let l = log(&r);
        assert_eq!(l.iter().filter(|l| l.contains("keystone")).count(), 1, "sent once: {l:?}");
        let at = l.iter().position(|l| l.contains("keystone")).unwrap();
        assert_eq!(l[at], "keystone+frame", "with a frame, music keeps streaming: {l:?}");
        assert!(l[at + 1..].iter().all(|l| l == "frame"), "no re-init right after it: {l:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn band_levels_go_to_the_ui_while_running() {
        let mut r = start(true);
        r.cmds.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        play(&r, (0..3).map(tone)).await;
        assert_eq!(r.bands.borrow_and_update().len(), 18);
        let (ack, done) = oneshot::channel();
        r.cmds.send(Cmd::Set { on: false, ack: Some(ack) }).await.unwrap();
        done.await.unwrap();
        assert!(r.bands.borrow_and_update().is_empty(), "cleared when music stops");
    }

    #[tokio::test(start_paused = true)]
    async fn no_layout_means_unavailable() {
        let (tx, rx) = mpsc::channel(8);
        let status: Status = Arc::new(Mutex::new((MusicState::Off, None)));
        let io = Io { bands: None, capture: Box::new(|| Err(std::io::Error::other("x"))), keyboard: Box::new(|| Err(std::io::Error::other("x"))),
                      effects: Arc::new(FakeFx(Arc::new(Mutex::new(Vec::new())))) };
        tokio::spawn(run_worker(io, None, rx, status.clone(), MusicConfig { on: true, ..MusicConfig::default() }));
        tx.send(Cmd::Env { active: true }).await.unwrap(); settle().await;
        assert_eq!(status.lock().unwrap().0, MusicState::Unavailable);
    }
}
