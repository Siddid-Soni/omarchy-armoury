use crate::config::Config;
use crate::control::{Control, ModeHandle, handback, takeover};
use crate::features::fan;
use crate::features::limits::{self, Bounds, Limit};
use crate::features::perf::{HwCtx, apply_mode, apply_nv};
use crate::hw::{Asusd, Gfx, Nvidia, Services, Sysfs, with_retry};
use crate::state::collect;
use anyhow::bail;
use armoury_proto::{ControlMode, Event, ModeSettings, Profile, Request, Response, Snapshot, UndervoltState};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, watch};

pub struct Daemon {
    sys: Box<dyn Sysfs>,
    gfx: Box<dyn Gfx>,
    svc: Box<dyn Services>,
    asusd: Box<dyn Asusd>,
    nv: Box<dyn Nvidia>,
    pub control: Mutex<Control>,
    mode: ModeHandle,
    snap: watch::Sender<Option<Snapshot>>,
    pub config: Mutex<Config>,
    config_path: PathBuf,
    /// Serialises every apply (poll tick, SetProfile, SetModeSettings).
    apply: Mutex<ApplyState>,
    config_error: Option<String>,
    /// Undervolt probe result (None until probed in active mode).
    uv: std::sync::Mutex<Option<bool>>,
    last_reapply: std::sync::Mutex<tokio::time::Instant>,
}

impl Daemon {
    pub fn new(sys: Box<dyn Sysfs>, gfx: Box<dyn Gfx>, svc: Box<dyn Services>, asusd: Box<dyn Asusd>, control: Control, config_path: PathBuf, nv: Box<dyn Nvidia>) -> Arc<Self> {
        let (config, config_error) = load_config(&config_path);
        if let Some(e) = &config_error { eprintln!("armouryd: {e}"); }
        let mode = control.mode_handle();
        Arc::new(Self {
            sys, gfx, svc, asusd, nv, control: Mutex::new(control), mode, snap: watch::channel(None).0,
            config: Mutex::new(config), config_path, config_error,
            apply: Mutex::new(ApplyState::default()),
            uv: std::sync::Mutex::new(None), last_reapply: std::sync::Mutex::new(tokio::time::Instant::now()),
        })
    }

    /// One poll: refresh, then (active only) apply the mode's settings when the
    /// profile changed from any source, and re-apply on the configured interval.
    /// Returns the errors from any apply done in this tick.
    pub async fn tick(&self) -> Vec<String> {
        self.tick_inner(false).await
    }

    async fn tick_inner(&self, force: bool) -> Vec<String> {
        // Lock before reading the profile, so a slow poll can never apply a mode
        // that a concurrent SetProfile has already switched away from.
        let mut st = self.apply.lock().await;
        let snap = self.refresh().await;
        if self.mode.get() != ControlMode::Active {
            *st = ApplyState::default(); // re-apply after the next takeover
            return Vec::new();
        }
        if self.uv.lock().unwrap().is_none() { self.probe_undervolt().await; }
        let Some(profile) = snap.perf.profile else { return Vec::new() };
        let dgpu_active = snap.gpu.dgpu_active == Some(true);
        let woke = dgpu_active && !st.dgpu_was_active;
        st.dgpu_was_active = dgpu_active;
        let (settings, secs) = {
            let cfg = self.config.lock().await;
            (cfg.mode(profile), cfg.reapply_power_secs)
        };
        let now = tokio::time::Instant::now();
        let due = secs > 0 && now.duration_since(*self.last_reapply.lock().unwrap()) >= Duration::from_secs(secs as u64);
        if st.applied == Some(profile) && !due {
            // NVIDIA settings could not be sent while the dGPU slept; send them now it is awake.
            return if woke { apply_nv(&settings, &*self.svc).await } else { Vec::new() };
        }
        if !force && st.failed == Some(profile) && st.retry_at.is_some_and(|t| now < t) { return Vec::new(); }
        let errs = apply_mode(profile, &settings, self.hw_ctx(dgpu_active), &*self.asusd, &*self.svc).await;
        if errs.is_empty() {
            *st = ApplyState { applied: Some(profile), dgpu_was_active: dgpu_active, ..Default::default() };
            *self.last_reapply.lock().unwrap() = now;
        } else {
            for e in &errs { eprintln!("armouryd: apply {}: {e}", profile.sysfs()); }
            *st = ApplyState { applied: None, failed: Some(profile), retry_at: Some(now + RETRY_BACKOFF), dgpu_was_active: dgpu_active };
        }
        errs
    }

    fn hw_ctx(&self, dgpu_active: bool) -> HwCtx {
        HwCtx { uv_unlocked: self.uv.lock().unwrap().unwrap_or(false), dgpu_active }
    }

    /// Asks armoury-root whether the BIOS leaves the undervolt mailbox writable.
    /// Any failure counts as locked, so undervolt is never sent blind.
    async fn probe_undervolt(&self) -> bool {
        let unlocked = match self.svc.output(&[PKEXEC, ROOT_HELPER, "undervolt", "probe"]).await {
            Ok(out) => out.trim() == "unlocked",
            Err(e) => { eprintln!("armouryd: undervolt probe: {e:#}"); false }
        };
        *self.uv.lock().unwrap() = Some(unlocked);
        unlocked
    }

    async fn write_guard(&self) -> Result<(), Response> {
        self.control.lock().await.require_active().map_err(|e| Response::err(e.to_string()))
    }

    async fn curves(&self, p: Profile) -> Response {
        match with_retry(|| self.asusd.fan_curves(p.to_asusd())).await {
            Ok(raw) => Response::ok(serde_json::to_value(raw.iter().filter_map(fan::from_raw).collect::<Vec<_>>()).unwrap()),
            Err(e) => Response::err(format!("fan curves: {e:#}")),
        }
    }

    /// Firmware ranges where reported, fallbacks otherwise.
    async fn bounds(&self) -> impl Fn(Limit) -> Bounds + use<> {
        let mut found = Vec::new();
        for l in Limit::ALL {
            found.push((l, limits::bounds(l, with_retry(|| self.asusd.armoury_range(l.attr())).await.ok())));
        }
        move |l| found.iter().find(|(k, _)| *k == l).map(|(_, b)| *b).unwrap()
    }

    async fn snapshot_after(&self, r: anyhow::Result<()>) -> Response {
        if let Err(e) = r { return Response::err(format!("{e:#}")); }
        let errs = self.tick_inner(true).await;
        if !errs.is_empty() {
            return Response::err(format!("mode changed, but its settings failed: {}", errs.join("; ")));
        }
        Response::ok(serde_json::to_value(self.refresh().await).unwrap())
    }

    /// Collects a fresh snapshot (no NVML) and publishes it to subscribers only if it changed.
    pub async fn refresh(&self) -> Snapshot {
        let s = self.snapshot(false).await;
        self.snap.send_if_modified(|cur| {
            if cur.as_ref() == Some(&s) { false } else { *cur = Some(s.clone()); true }
        });
        s
    }

    async fn snapshot(&self, gpu_detail: bool) -> Snapshot {
        let mode = self.mode.get();
        let mut s = collect(&*self.sys, &*self.gfx, &*self.svc, &*self.nv, mode, gpu_detail).await;
        s.config_error = self.config_error.clone();
        s.perf.undervolt = self.uv.lock().unwrap().map(|unlocked| UndervoltState { unlocked });
        s
    }

    pub async fn handle(&self, req: Request) -> Response {
        match req {
            Request::Ping => Response::ok("pong".into()),
            Request::Status => {
                self.refresh().await;
                Response::ok(serde_json::to_value(self.snapshot(true).await).unwrap())
            }
            Request::Subscribe => Response::ok(serde_json::Value::Null),
            Request::Takeover | Request::Handback => {
                let result = {
                    let mut ctl = self.control.lock().await;
                    if req == Request::Takeover { takeover(&mut ctl, &*self.svc).await } else { handback(&mut ctl, &*self.svc).await }
                };
                let snap = self.refresh().await;
                match result {
                    Ok(()) => Response::ok(serde_json::to_value(snap).unwrap()),
                    Err(e) => Response::err(format!("{e:#}")),
                }
            }
            Request::SetProfile { profile } => {
                if let Err(r) = self.write_guard().await { return r; }
                self.snapshot_after(with_retry(|| self.asusd.set_profile(profile.to_asusd())).await).await
            }
            Request::NextProfile => {
                if let Err(r) = self.write_guard().await { return r; }
                self.snapshot_after(with_retry(|| self.asusd.next_profile()).await).await
            }
            Request::FanCurves { profile } => self.curves(profile).await,
            Request::ProbeUndervolt => {
                if let Err(r) = self.write_guard().await { return r; }
                Response::ok(serde_json::json!({"unlocked": self.probe_undervolt().await}))
            }
            Request::SetFanCurve { profile, curve } => {
                if let Err(r) = self.write_guard().await { return r; }
                if let Err(e) = fan::validate(&curve) { return Response::err(e); }
                if let Err(e) = with_retry(|| self.asusd.set_fan_curve(profile.to_asusd(), fan::to_raw(&curve))).await {
                    return Response::err(format!("{e:#}"));
                }
                self.curves(profile).await
            }
            Request::ResetFanCurves { profile } => {
                if let Err(r) = self.write_guard().await { return r; }
                if let Err(e) = with_retry(|| self.asusd.reset_fan_curves(profile.to_asusd())).await { return Response::err(format!("{e:#}")); }
                self.curves(profile).await
            }
            Request::ModeSettings { profile } => {
                let b = self.bounds().await;
                let bounds: serde_json::Map<String, serde_json::Value> =
                    Limit::ALL.iter().map(|l| (l.key().to_string(), serde_json::json!([b(*l).min, b(*l).max]))).collect();
                Response::ok(serde_json::json!({"settings": self.config.lock().await.mode(profile), "bounds": bounds}))
            }
            Request::SetModeSettings { profile, settings } => {
                if let Err(r) = self.write_guard().await { return r; }
                let mut st = self.apply.lock().await; // check → apply → save as one step
                let merged = merge(self.config.lock().await.mode(profile), settings);
                if let Err(e) = limits::validate(&merged, self.bounds().await) { return Response::err(e); }
                let snap = self.refresh().await;
                if snap.perf.profile == Some(profile) {
                    // apply everything stored, not just the delta, so earlier drift is repaired
                    let dgpu_active = snap.gpu.dgpu_active == Some(true);
                    let errs = apply_mode(profile, &merged, self.hw_ctx(dgpu_active), &*self.asusd, &*self.svc).await;
                    if !errs.is_empty() { return Response::err(errs.join("; ")); }
                    *st = ApplyState { applied: Some(profile), dgpu_was_active: dgpu_active, ..Default::default() };
                }
                let mut cfg = self.config.lock().await;
                cfg.modes.insert(profile, merged);
                if let Err(e) = cfg.save(&self.config_path) { return Response::err(format!("save config: {e}")); }
                Response::ok(serde_json::to_value(merged).unwrap())
            }
        }
    }

    pub async fn serve(self: Arc<Self>, listener: UnixListener) -> anyhow::Result<()> {
        loop {
            let (stream, _) = listener.accept().await?;
            let me = self.clone();
            tokio::spawn(async move { let _ = me.connection(stream).await; });
        }
    }

    async fn connection(&self, stream: UnixStream) -> anyhow::Result<()> {
        let (r, mut w) = stream.into_split();
        let mut lines = BufReader::new(r).lines();
        while let Some(line) = lines.next_line().await? {
            let resp = match serde_json::from_str::<Request>(&line) {
                Ok(Request::Subscribe) => {
                    write_line(&mut w, &Response::ok(serde_json::Value::Null)).await?;
                    return self.stream_events(&mut w).await;
                }
                Ok(req) => self.handle(req).await,
                Err(e) => Response::err(format!("invalid request: {e}")),
            };
            write_line(&mut w, &resp).await?;
        }
        Ok(())
    }

    async fn stream_events(&self, w: &mut OwnedWriteHalf) -> anyhow::Result<()> {
        let mut rx = self.snap.subscribe();
        let current = rx.borrow_and_update().clone();
        let first = match current {
            Some(s) => s,
            None => {
                self.refresh().await;
                // mark the refresh we just triggered as seen, or it is sent twice
                rx.borrow_and_update().clone().unwrap_or_default()
            }
        };
        write_line(w, &Event::Snapshot(first)).await?;
        while rx.changed().await.is_ok() {
            let Some(s) = rx.borrow_and_update().clone() else { continue };
            write_line(w, &Event::Snapshot(s)).await?;
        }
        Ok(())
    }

    pub async fn poll_loop(self: Arc<Self>, every: Duration) {
        loop {
            self.tick().await;
            tokio::time::sleep(every).await;
        }
    }
}

use crate::control::{PKEXEC, ROOT_HELPER};

const RETRY_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Default)]
struct ApplyState {
    /// Profile whose settings are in force.
    applied: Option<Profile>,
    /// Profile whose last apply failed, and when to try it again.
    failed: Option<Profile>,
    retry_at: Option<tokio::time::Instant>,
    /// dGPU state at the last tick, to catch it waking up.
    dgpu_was_active: bool,
}

/// Loads config.toml. An unparsable file is moved to config.toml.bad (so the next
/// save cannot destroy it); modes with out-of-range values are dropped. Either is reported.
fn load_config(path: &Path) -> (Config, Option<String>) {
    let (mut config, err) = Config::load(path);
    if let Some(e) = err {
        let bad = path.with_extension("toml.bad");
        let moved = std::fs::rename(path, &bad).is_ok();
        let where_ = if moved { format!("moved to {}", bad.display()) } else { "ignored".into() };
        return (config, Some(format!("config.toml was invalid and was {where_}: {e}")));
    }
    let mut dropped = Vec::new();
    config.modes.retain(|p, s| match limits::validate(s, |l| limits::bounds(l, None)) {
        Ok(()) => true,
        Err(e) => { dropped.push(format!("[modes.{}] ignored: {e}", p.sysfs())); false }
    });
    (config, (!dropped.is_empty()).then(|| format!("config.toml: {}", dropped.join("; "))))
}

/// Fields present in `new` overwrite `old`.
fn merge(old: ModeSettings, new: ModeSettings) -> ModeSettings {
    ModeSettings {
        pl1: new.pl1.or(old.pl1),
        pl2: new.pl2.or(old.pl2),
        nv_boost: new.nv_boost.or(old.nv_boost),
        nv_temp: new.nv_temp.or(old.nv_temp),
        epp: new.epp.or(old.epp),
        cpu_boost: new.cpu_boost.or(old.cpu_boost),
        uv_mv: new.uv_mv.or(old.uv_mv),
        gpu_core_offset: new.gpu_core_offset.or(old.gpu_core_offset),
        gpu_mem_offset: new.gpu_mem_offset.or(old.gpu_mem_offset),
        gpu_core_lock: new.gpu_core_lock.or(old.gpu_core_lock),
        gpu_mem_lock: new.gpu_mem_lock.or(old.gpu_mem_lock),
    }
}

async fn write_line<T: serde::Serialize>(w: &mut OwnedWriteHalf, v: &T) -> anyhow::Result<()> {
    let mut buf = serde_json::to_vec(v)?;
    buf.push(b'\n');
    w.write_all(&buf).await?;
    Ok(())
}

pub fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            bail!("armouryd already running (socket {} is live)", path.display());
        }
        std::fs::remove_file(path)?;
    }
    Ok(UnixListener::bind(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::fake::{FakeAsusd, FakeGfx, FakeNvidia, FakeServices, FakeSysfs};
    use armoury_proto::ControlMode;
    use std::path::PathBuf;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    fn daemon(dir: &Path) -> Arc<Daemon> {
        let sys = FakeSysfs::with(&[("sys/devices/platform/asus-nb-wmi/keystone", "1")]);
        Daemon::new(Box::new(sys), Box::new(FakeGfx::default()), Box::new(FakeServices::default()),
            Box::new(FakeAsusd::default()), Control::load(dir), dir.join("config.toml"), Box::new(FakeNvidia::default()))
    }

    async fn start(dir: &Path) -> (Arc<Daemon>, PathBuf) {
        let sock = dir.join("t.sock");
        let d = daemon(dir);
        let l = bind(&sock).unwrap();
        tokio::spawn(d.clone().serve(l));
        (d, sock)
    }

    async fn roundtrip(sock: &Path, line: &str) -> serde_json::Value {
        let s = UnixStream::connect(sock).await.unwrap();
        let (r, mut w) = s.into_split();
        w.write_all(format!("{line}\n").as_bytes()).await.unwrap();
        let mut lines = BufReader::new(r).lines();
        serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn status_returns_snapshot() {
        let d = tempfile::tempdir().unwrap();
        let (_, sock) = start(d.path()).await;
        let v = roundtrip(&sock, r#"{"cmd":"status"}"#).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["data"]["keystone"], true);
        assert_eq!(v["data"]["control"], "observe");
    }

    #[tokio::test]
    async fn invalid_line_gets_error_and_connection_survives() {
        let d = tempfile::tempdir().unwrap();
        let (_, sock) = start(d.path()).await;
        let s = UnixStream::connect(&sock).await.unwrap();
        let (r, mut w) = s.into_split();
        let mut lines = BufReader::new(r).lines();
        w.write_all(b"not json\n{\"cmd\":\"ping\"}\n").await.unwrap();
        let e: serde_json::Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(e["ok"], false);
        assert!(e["error"].as_str().unwrap().starts_with("invalid request"));
        let p: serde_json::Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(p, serde_json::json!({"ok": true, "data": "pong"}));
    }

    #[tokio::test]
    async fn subscribe_streams_snapshot_then_changes() {
        let d = tempfile::tempdir().unwrap();
        let (daemon, sock) = start(d.path()).await;
        let s = UnixStream::connect(&sock).await.unwrap();
        let (r, mut w) = s.into_split();
        let mut lines = BufReader::new(r).lines();
        w.write_all(b"{\"cmd\":\"subscribe\"}\n").await.unwrap();
        let ack: serde_json::Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(ack["ok"], true);
        let first: serde_json::Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(first["event"], "snapshot");
        daemon.control.lock().await.set(ControlMode::Active).unwrap();
        daemon.refresh().await;
        let second: serde_json::Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(second["data"]["control"], "active");
    }

    #[tokio::test]
    async fn subscriber_disconnect_does_not_kill_server() {
        let d = tempfile::tempdir().unwrap();
        let (daemon, sock) = start(d.path()).await;
        {
            let mut s = UnixStream::connect(&sock).await.unwrap();
            s.write_all(b"{\"cmd\":\"subscribe\"}\n").await.unwrap();
        }
        daemon.control.lock().await.set(ControlMode::Active).unwrap();
        daemon.refresh().await;
        let v = roundtrip(&sock, r#"{"cmd":"ping"}"#).await;
        assert_eq!(v["data"], "pong");
    }

    #[tokio::test]
    async fn bind_replaces_stale_socket() {
        let d = tempfile::tempdir().unwrap();
        let sock = d.path().join("s.sock");
        drop(std::os::unix::net::UnixListener::bind(&sock).unwrap()); // leaves a dead socket file
        assert!(sock.exists());
        bind(&sock).unwrap();
    }

    #[tokio::test]
    async fn bind_refuses_live_socket() {
        let d = tempfile::tempdir().unwrap();
        let sock = d.path().join("s.sock");
        let _live = bind(&sock).unwrap();
        let err = bind(&sock).unwrap_err();
        assert!(err.to_string().contains("already running"));
    }

    #[tokio::test]
    async fn takeover_request_reports_failure_as_error() {
        let d = tempfile::tempdir().unwrap();
        let svc = FakeServices::default();
        *svc.fail_on.lock().unwrap() = Some("pkexec".into());
        let daemon = Daemon::new(Box::new(FakeSysfs::default()), Box::new(FakeGfx::default()), Box::new(svc),
            Box::new(FakeAsusd::default()), Control::load(d.path()), d.path().join("config.toml"), Box::new(FakeNvidia::default()));
        let r = daemon.handle(Request::Takeover).await;
        assert!(!r.ok);
        assert!(r.error.unwrap().contains("G-Helper restored"));
    }

    #[tokio::test]
    async fn status_not_blocked_by_transition() {
        let d = tempfile::tempdir().unwrap();
        let daemon = daemon(d.path());
        let _held = daemon.control.lock().await; // a takeover in progress
        let r = tokio::time::timeout(Duration::from_secs(1), daemon.handle(Request::Status)).await;
        assert!(r.expect("status blocked by control lock").ok);
    }

    struct Rig { d: Arc<Daemon>, sys: Arc<FakeSysfs>, asusd: Arc<FakeAsusd>, svc: Arc<FakeServices>, dir: tempfile::TempDir }

    fn rig(active: bool) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let sys = Arc::new(FakeSysfs::with(&[("sys/firmware/acpi/platform_profile", "balanced")]));
        let asusd = Arc::new(FakeAsusd::default());
        let svc = Arc::new(FakeServices::default());
        let mut ctl = Control::load(dir.path());
        if active { ctl.set(ControlMode::Active).unwrap(); }
        let d = Daemon::new(Box::new(sys.clone()), Box::new(FakeGfx::default()), Box::new(svc.clone()),
            Box::new(asusd.clone()), ctl, dir.path().join("config.toml"), Box::new(FakeNvidia::default()));
        Rig { d, sys, asusd, svc, dir }
    }

    fn set_mode(profile: &str, pl1: i32, pl2: i32) -> Request {
        serde_json::from_value(serde_json::json!({"cmd":"set_mode_settings","profile":profile,"settings":{"pl1":pl1,"pl2":pl2}})).unwrap()
    }

    #[tokio::test]
    async fn observe_mode_never_applies() {
        let r = rig(false);
        let resp = r.d.handle(set_mode("balanced", 60, 80)).await;
        assert!(!resp.ok && resp.error.unwrap().contains("observe"));
        r.sys.files.lock().unwrap().insert("sys/firmware/acpi/platform_profile".into(), "performance".into());
        r.d.tick().await;
        assert!(r.asusd.calls.lock().unwrap().is_empty() && r.svc.calls.lock().unwrap().is_empty());
    }

    fn limit_calls(r: &Rig) -> Vec<String> {
        r.svc.calls.lock().unwrap().iter().filter(|c| c.contains("set-limits")).cloned().collect()
    }

    #[tokio::test]
    async fn set_mode_settings_applies_current_and_saves() {
        let r = rig(true);
        r.d.tick().await; // first tick records current profile (nothing stored yet)
        let resp = r.d.handle(set_mode("balanced", 60, 80)).await;
        assert!(resp.ok, "{resp:?}");
        assert!(limit_calls(&r).last().unwrap().ends_with("set-limits pl1=60 pl2=80 cpu_boost=on"), "{:?}", limit_calls(&r));
        let text = std::fs::read_to_string(r.dir.path().join("config.toml")).unwrap();
        assert!(text.contains("pl1 = 60"), "{text}");
    }

    #[tokio::test]
    async fn set_mode_settings_for_other_mode_only_saves() {
        let r = rig(true);
        r.d.tick().await;
        assert!(r.d.handle(set_mode("performance", 120, 150)).await.ok);
        assert!(!limit_calls(&r).iter().any(|c| c.contains("pl1=")), "{:?}", limit_calls(&r));
    }

    #[tokio::test]
    async fn external_profile_change_applies_mode() {
        let r = rig(true);
        r.d.tick().await;
        assert!(r.d.handle(set_mode("performance", 120, 150)).await.ok);
        r.sys.files.lock().unwrap().insert("sys/firmware/acpi/platform_profile".into(), "performance".into()); // Fn+F5
        r.d.tick().await;
        assert!(limit_calls(&r).last().unwrap().ends_with("set-limits pl1=120 pl2=150 cpu_boost=on"), "{:?}", limit_calls(&r));
    }

    #[tokio::test]
    async fn invalid_settings_rejected_and_not_saved() {
        let r = rig(true);
        let resp = r.d.handle(set_mode("balanced", 140, 100)).await;
        assert!(resp.error.unwrap().contains("PL1 must not exceed PL2"));
        assert!(!r.dir.path().join("config.toml").exists());
        assert!(limit_calls(&r).is_empty());
    }

    #[tokio::test]
    async fn failed_set_does_not_save() {
        let r = rig(true);
        r.d.tick().await;
        *r.svc.fail_on.lock().unwrap() = Some("set-limits".into());
        let resp = r.d.handle(set_mode("balanced", 60, 80)).await;
        assert!(!resp.ok && resp.error.unwrap().contains("power limits"));
        assert!(!r.dir.path().join("config.toml").exists());
    }

    #[tokio::test]
    async fn fan_curve_validation_and_set() {
        let r = rig(true);
        let bad = serde_json::json!({"cmd":"set_fan_curve","profile":"quiet","curve":{"fan":"cpu","temps":[40,50,60,70,80,90,95,97],"percent":[50,40,50,60,70,80,90,100],"enabled":true}});
        let resp = r.d.handle(serde_json::from_value(bad).unwrap()).await;
        assert!(resp.error.unwrap().contains("fan speed"));
        let good = serde_json::json!({"cmd":"set_fan_curve","profile":"quiet","curve":{"fan":"cpu","temps":[40,50,60,70,80,90,95,97],"percent":[0,10,20,30,40,60,80,100],"enabled":true}});
        let resp = r.d.handle(serde_json::from_value(good).unwrap()).await;
        assert!(resp.ok, "{resp:?}");
        assert_eq!(resp.data.unwrap()[0]["percent"][7], 100);
        assert!(r.asusd.calls.lock().unwrap().contains(&"set_fan_curve 2 CPU".to_string()));
    }

    #[tokio::test(start_paused = true)]
    async fn reapply_timer() {
        let r = rig(true);
        r.d.tick().await;
        assert!(r.d.handle(set_mode("balanced", 60, 80)).await.ok);
        r.svc.calls.lock().unwrap().clear();
        r.d.config.lock().await.reapply_power_secs = 10;
        tokio::time::advance(Duration::from_secs(5)).await;
        r.d.tick().await;
        assert!(limit_calls(&r).is_empty());
        tokio::time::advance(Duration::from_secs(6)).await;
        r.d.tick().await;
        assert_eq!(limit_calls(&r).len(), 1);
    }

    #[tokio::test]
    async fn set_profile_reports_apply_failure() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[modes.balanced]\npl1 = 45\npl2 = 65\n").unwrap();
        let sys = Arc::new(FakeSysfs::with(&[("sys/firmware/acpi/platform_profile", "balanced")]));
        let svc = FakeServices::default();
        *svc.fail_on.lock().unwrap() = Some("set-limits".into());
        let mut ctl = Control::load(dir.path());
        ctl.set(ControlMode::Active).unwrap();
        let d = Daemon::new(Box::new(sys), Box::new(FakeGfx::default()), Box::new(svc),
            Box::new(FakeAsusd::default()), ctl, dir.path().join("config.toml"), Box::new(FakeNvidia::default()));
        let resp = d.handle(Request::SetProfile { profile: Profile::Balanced }).await;
        let err = resp.error.expect("apply failure must be reported");
        assert!(err.contains("settings failed") && err.contains("power limits"), "{err}");
    }

    fn rig_with_config(toml: &str, active: bool) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), toml).unwrap();
        let sys = Arc::new(FakeSysfs::with(&[("sys/firmware/acpi/platform_profile", "balanced")]));
        let asusd = Arc::new(FakeAsusd::default());
        let svc = Arc::new(FakeServices::default());
        let mut ctl = Control::load(dir.path());
        if active { ctl.set(ControlMode::Active).unwrap(); }
        let d = Daemon::new(Box::new(sys.clone()), Box::new(FakeGfx::default()), Box::new(svc.clone()),
            Box::new(asusd.clone()), ctl, dir.path().join("config.toml"), Box::new(FakeNvidia::default()));
        Rig { d, sys, asusd, svc, dir }
    }

    #[tokio::test]
    async fn takeover_after_handback_reapplies() {
        let r = rig_with_config("[modes.balanced]\npl1 = 45\npl2 = 65\n", true);
        r.d.tick().await;
        r.d.control.lock().await.set(ControlMode::Observe).unwrap(); // handback
        r.d.tick().await;
        r.d.control.lock().await.set(ControlMode::Active).unwrap(); // takeover, same mode
        r.d.tick().await;
        assert_eq!(limit_calls(&r).len(), 2, "{:?}", limit_calls(&r));
    }

    #[tokio::test(start_paused = true)]
    async fn failed_apply_retried_after_backoff() {
        let r = rig_with_config("[modes.balanced]\npl1 = 45\npl2 = 65\n", true);
        *r.svc.fail_on.lock().unwrap() = Some("set-limits".into());
        r.d.tick().await;
        r.d.tick().await;
        assert_eq!(limit_calls(&r).len(), 1, "no retry inside backoff");
        *r.svc.fail_on.lock().unwrap() = None;
        tokio::time::advance(Duration::from_secs(31)).await;
        r.d.tick().await;
        assert_eq!(limit_calls(&r).len(), 2, "retried after backoff");
        r.d.tick().await;
        assert_eq!(limit_calls(&r).len(), 2, "no re-apply once it worked");
    }

    #[tokio::test]
    async fn set_mode_settings_applies_merged() {
        let r = rig_with_config("[modes.balanced]\npl1 = 45\npl2 = 65\ncpu_boost = false\n", true);
        let req = serde_json::from_value(serde_json::json!({"cmd":"set_mode_settings","profile":"balanced","settings":{"pl2":60}})).unwrap();
        assert!(r.d.handle(req).await.ok);
        assert!(limit_calls(&r).last().unwrap().ends_with("set-limits pl1=45 pl2=60 cpu_boost=off"), "{:?}", limit_calls(&r));
    }

    #[tokio::test]
    async fn invalid_config_values_dropped_and_reported() {
        let r = rig_with_config("[modes.performance]\npl1 = 200\npl2 = 150\n[modes.balanced]\npl1 = 45\npl2 = 65\n", true);
        assert_eq!(r.d.config.lock().await.mode(Profile::Performance), ModeSettings::default());
        assert_eq!(r.d.config.lock().await.mode(Profile::Balanced).pl1, Some(45));
        let s = r.d.refresh().await;
        assert!(s.config_error.unwrap().contains("performance"));
    }

    #[tokio::test]
    async fn corrupt_config_moved_aside_and_reported() {
        let r = rig_with_config("modes = 7\n", true);
        assert!(r.dir.path().join("config.toml.bad").exists());
        assert!(r.d.refresh().await.config_error.unwrap().contains("config.toml.bad"));
        assert!(r.d.handle(set_mode("balanced", 60, 80)).await.ok);
        assert_eq!(std::fs::read_to_string(r.dir.path().join("config.toml.bad")).unwrap(), "modes = 7\n");
    }

    struct NvRig { d: Arc<Daemon>, svc: Arc<FakeServices>, nv: Arc<FakeNvidia> }

    fn rig_with_nv(toml: &str, probe_output: &str) -> NvRig {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), toml).unwrap();
        let sys = Arc::new(FakeSysfs::with(&[("sys/firmware/acpi/platform_profile", "balanced")]));
        let svc = Arc::new(FakeServices::default());
        svc.outputs.lock().unwrap().insert("undervolt probe".into(), probe_output.into());
        let nv = Arc::new(FakeNvidia::with_active(Some(false)));
        let mut ctl = Control::load(dir.path());
        ctl.set(ControlMode::Active).unwrap();
        let d = Daemon::new(Box::new(sys), Box::new(FakeGfx::default()), Box::new(svc.clone()),
            Box::new(FakeAsusd::default()), ctl, dir.path().join("config.toml"), Box::new(nv.clone()));
        std::mem::forget(dir);
        NvRig { d, svc, nv }
    }

    #[tokio::test]
    async fn nv_applied_when_dgpu_wakes() {
        let r = rig_with_nv("[modes.balanced]\ngpu_core_offset = 50\n", "locked\n");
        r.d.tick().await;
        assert!(!r.svc.calls.lock().unwrap().iter().any(|c| c.contains("nv-clocks")));
        *r.nv.active.lock().unwrap() = Some(true);
        r.d.tick().await;
        assert!(r.svc.calls.lock().unwrap().iter().any(|c| c.ends_with("nv-clocks 50 0 off off")), "{:?}", r.svc.calls.lock().unwrap());
    }

    #[tokio::test]
    async fn probe_result_reported_and_used() {
        let r = rig_with_nv("[modes.balanced]\nuv_mv = -30\n", "unlocked\n");
        r.d.tick().await;
        let calls = r.svc.calls.lock().unwrap().clone();
        assert!(calls.iter().any(|c| c.ends_with("undervolt probe")));
        assert!(calls.iter().any(|c| c.ends_with("undervolt set -30")), "{calls:?}");
        assert_eq!(r.d.refresh().await.perf.undervolt, Some(armoury_proto::UndervoltState { unlocked: true }));
    }

    #[tokio::test]
    async fn locked_probe_never_sends_undervolt() {
        let r = rig_with_nv("[modes.balanced]\nuv_mv = -30\n", "locked\n");
        r.d.tick().await;
        assert!(!r.svc.calls.lock().unwrap().iter().any(|c| c.contains("undervolt set")));
        assert_eq!(r.d.refresh().await.perf.undervolt, Some(armoury_proto::UndervoltState { unlocked: false }));
    }

    #[tokio::test]
    async fn poll_never_reads_nvml() {
        // measured: 2 s NVML polling kept the dGPU awake indefinitely
        let r = rig_with_nv("", "locked\n");
        *r.nv.active.lock().unwrap() = Some(true);
        for _ in 0..3 { r.d.tick().await; }
        assert_eq!(r.nv.status_reads.load(std::sync::atomic::Ordering::SeqCst), 0);
        let v = r.d.handle(Request::Status).await.data.unwrap();
        assert_eq!(v["gpu"]["nvidia"]["core_mhz"], 1500, "explicit status still reports NVIDIA details");
        assert_eq!(r.nv.status_reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
