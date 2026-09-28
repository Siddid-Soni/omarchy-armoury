use crate::config::Config;
use crate::control::{Control, ModeHandle, handback, takeover};
use crate::features::fan;
use crate::features::limits::{self, Bounds, Limit};
use crate::features::perf::{HwCtx, apply_mode, apply_nv, nv_is_stock};
use crate::features::lighting::{effect_from_raw, effect_to_raw, power_from_raw, set_zone, validate_effect};
use crate::hw::{Asusd, Aura, Gfx, Nvidia, Services, Sysfs, with_retry};
use crate::state::collect;
use anyhow::{Context, bail};
use crate::features::gpu::plan_switch;
use armoury_proto::{ControlMode, Event, GpuMode, GpuStep, GpuSwitchResult, ModeSettings, Profile, Request, Response, Snapshot, UndervoltState};
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
    aura: Box<dyn Aura>,
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
    apply_error: std::sync::Mutex<Option<String>>,
    /// Serialises GPU plan + execute.
    gpu_lock: Mutex<()>,
    light: std::sync::Mutex<LightState>,
    state_dir: PathBuf,
    last_reapply: std::sync::Mutex<tokio::time::Instant>,
}

impl Daemon {
    pub fn new(sys: Box<dyn Sysfs>, gfx: Box<dyn Gfx>, svc: Box<dyn Services>, asusd: Box<dyn Asusd>, control: Control, config_path: PathBuf, nv: Box<dyn Nvidia>) -> Arc<Self> {
        let (config, config_error) = load_config(&config_path);
        if let Some(e) = &config_error { eprintln!("armouryd: {e}"); }
        let mode = control.mode_handle();
        let state_dir = control.state_dir();
        Arc::new(Self {
            sys, gfx, svc, asusd, nv, aura: Box::new(NoAura), control: Mutex::new(control), mode, snap: watch::channel(None).0,
            config: Mutex::new(config), config_path, config_error,
            apply: Mutex::new(ApplyState::default()),
            uv: std::sync::Mutex::new(None),
            apply_error: std::sync::Mutex::new(None),
            gpu_lock: Mutex::new(()),
            light: std::sync::Mutex::new(LightState::default()),
            state_dir, last_reapply: std::sync::Mutex::new(tokio::time::Instant::now()),
        })
    }

    /// Sets the asusd Aura client (keyboard lighting); call before the daemon is shared.
    pub fn with_aura(self: Arc<Self>, aura: Box<dyn Aura>) -> Arc<Self> {
        let mut d = Arc::try_unwrap(self).ok().expect("with_aura before the daemon is shared");
        d.aura = aura;
        Arc::new(d)
    }

    /// Applies the stored AC/battery keyboard brightness when the power source changes.
    async fn follow_power_source(&self, snap: &Snapshot) {
        let Some(on_ac) = snap.lighting.on_ac else { return };
        let target = {
            let mut l = self.light.lock().unwrap();
            if l.on_ac == Some(on_ac) { return; }
            l.on_ac = Some(on_ac);
            if l.idle_saved.is_some() { return; } // resume will restore
            let cfg = self.config.try_lock();
            let Ok(cfg) = cfg else { return };
            if on_ac { cfg.lighting.brightness_ac } else { cfg.lighting.brightness_battery }
        };
        if let Some(t) = target.filter(|t| snap.lighting.brightness != Some(*t)) {
            if let Err(e) = with_retry(|| self.aura.set_brightness(t as u32)).await { eprintln!("armouryd: keyboard brightness: {e:#}"); }
        }
    }

    async fn lighting_request(&self, req: Request) -> Response {
        if !matches!(req, Request::Lighting) {
            if let Err(r) = self.write_guard().await { return r; }
        }
        let info = || async { with_retry(|| self.aura.info()).await };
        let result: anyhow::Result<serde_json::Value> = async {
            match req {
                Request::Lighting => {
                    let (mode, power, modes, zones) = info().await?;
                    Ok(serde_json::to_value(armoury_proto::LightingInfo {
                        effect: effect_from_raw(&mode),
                        zones: power_from_raw(&power),
                        modes: modes.into_iter().filter_map(armoury_proto::AuraMode::from_code).collect(),
                        power_zones: zones.into_iter().filter_map(armoury_proto::AuraZone::from_code).collect(),
                    })?)
                }
                Request::SetBrightness { level } => {
                    anyhow::ensure!(level <= 3, "brightness must be 0–3 (off, low, med, high)");
                    with_retry(|| self.aura.set_brightness(level as u32)).await?;
                    let on_ac = self.refresh().await.lighting.on_ac.unwrap_or(true);
                    let mut cfg = self.config.lock().await;
                    if on_ac { cfg.lighting.brightness_ac = Some(level) } else { cfg.lighting.brightness_battery = Some(level) }
                    cfg.save(&self.config_path)?;
                    self.light.lock().unwrap().idle_saved = None;
                    Ok(serde_json::json!({"level": level, "on_ac": on_ac}))
                }
                Request::SetEffect { effect } => {
                    let (_, _, modes, _) = info().await?;
                    let supported: Vec<_> = modes.into_iter().filter_map(armoury_proto::AuraMode::from_code).collect();
                    validate_effect(&effect, &supported).map_err(anyhow::Error::msg)?;
                    with_retry(|| self.aura.set_mode_data(effect_to_raw(&effect))).await?;
                    Ok(serde_json::to_value(effect)?)
                }
                Request::SetZonePower { zone } => {
                    let (_, power, _, zones) = info().await?;
                    anyhow::ensure!(zones.contains(&zone.zone.code()), "{:?} lighting is not available on this laptop", zone.zone);
                    let new = set_zone(&power, &zone);
                    with_retry(|| self.aura.set_power(new.clone())).await?;
                    Ok(serde_json::to_value(power_from_raw(&new))?)
                }
                Request::KbdIdle => {
                    if self.config.lock().await.lighting.keep_on { return Ok(serde_json::json!({"dimmed": false})); }
                    let current = self.refresh().await.lighting.brightness.unwrap_or(0);
                    let first = {
                        let mut l = self.light.lock().unwrap();
                        let first = l.idle_saved.is_none();
                        if first { l.idle_saved = Some(current); }
                        first
                    };
                    if first && current > 0 { with_retry(|| self.aura.set_brightness(0)).await?; }
                    Ok(serde_json::json!({"dimmed": true}))
                }
                Request::KbdResume => {
                    let saved = self.light.lock().unwrap().idle_saved.take();
                    if let Some(level) = saved.filter(|l| *l > 0) { with_retry(|| self.aura.set_brightness(level as u32)).await?; }
                    Ok(serde_json::json!({"restored": saved}))
                }
                _ => unreachable!(),
            }
        }.await;
        match result { Ok(v) => Response::ok(v), Err(e) => Response::err(format!("{e:#}")) }
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
        self.tick_locked(&mut st, force).await
    }

    async fn tick_locked(&self, st: &mut ApplyState, force: bool) -> Vec<String> {
        let snap = self.refresh().await;
        if self.mode.get() != ControlMode::Active {
            *st = ApplyState::default(); // re-apply after the next takeover
            *self.light.lock().unwrap() = LightState::default();
            return Vec::new();
        }
        self.follow_power_source(&snap).await;
        let now = tokio::time::Instant::now();
        if self.uv.lock().unwrap().is_none() && self.config.lock().await.modes.values().any(|m| m.uv_mv.is_some())
            && st.probe_retry_at.is_none_or(|t| now >= t)
        {
            match self.probe_undervolt().await {
                None => st.probe_retry_at = Some(now + PROBE_BACKOFF),
                // the mode may already be applied without undervolt: apply it again
                Some(true) => { st.applied = None; st.uv_at_stock = true; }
                Some(false) => {}
            }
        }
        let Some(profile) = snap.perf.profile else { return Vec::new() };
        let dgpu_active = snap.gpu.dgpu_active == Some(true);
        let woke = dgpu_active && !st.dgpu_was_active;
        st.dgpu_was_active = dgpu_active;
        let (settings, secs) = {
            let cfg = self.config.lock().await;
            (cfg.mode(profile), cfg.reapply_power_secs)
        };
        let due = secs > 0 && now.duration_since(*self.last_reapply.lock().unwrap()) >= Duration::from_secs(secs as u64);
        if st.applied == Some(profile) {
            let mut errs = Vec::new();
            if woke && !(nv_is_stock(&settings) && st.nv_at_stock) {
                // NVIDIA settings could not be sent while the dGPU slept; send them now it is awake.
                errs = apply_nv(&settings, &*self.svc).await;
                st.nv_at_stock = errs.is_empty() && nv_is_stock(&settings);
            }
            if due {
                errs.extend(apply_mode(profile, &settings, HwCtx { limits_only: true, ..self.hw_ctx(dgpu_active, st) }, &*self.asusd, &*self.svc).await);
                *self.last_reapply.lock().unwrap() = now;
            }
            self.note_apply(profile, &errs);
            return errs;
        }
        if !force && st.failed == Some(profile) && st.retry_at.is_some_and(|t| now < t) { return Vec::new(); }
        let ctx = self.hw_ctx(dgpu_active, st);
        let errs = apply_mode(profile, &settings, ctx, &*self.asusd, &*self.svc).await;
        self.note_apply(profile, &errs);
        if errs.is_empty() {
            *st = ApplyState {
                applied: Some(profile),
                dgpu_was_active: dgpu_active,
                uv_at_stock: if ctx.uv_unlocked { settings.uv_mv.unwrap_or(0) == 0 } else { st.uv_at_stock },
                nv_at_stock: if dgpu_active { nv_is_stock(&settings) } else { st.nv_at_stock },
                probe_retry_at: st.probe_retry_at,
                ..Default::default()
            };
            *self.last_reapply.lock().unwrap() = now;
        } else {
            *st = ApplyState { failed: Some(profile), retry_at: Some(now + RETRY_BACKOFF), dgpu_was_active: dgpu_active, probe_retry_at: st.probe_retry_at, ..Default::default() };
        }
        errs
    }

    /// Records the outcome for `Snapshot.apply_error` and the journal.
    fn note_apply(&self, profile: Profile, errs: &[String]) {
        for e in errs { eprintln!("armouryd: apply {}: {e}", profile.sysfs()); }
        *self.apply_error.lock().unwrap() = (!errs.is_empty()).then(|| format!("{}: {}", profile.sysfs(), errs.join("; ")));
    }

    fn hw_ctx(&self, dgpu_active: bool, st: &ApplyState) -> HwCtx {
        HwCtx {
            uv_unlocked: self.uv.lock().unwrap().unwrap_or(false),
            dgpu_active,
            limits_only: false,
            uv_at_stock: st.uv_at_stock,
            nv_at_stock: st.nv_at_stock,
        }
    }

    /// Asks armoury-root whether the BIOS leaves the undervolt mailbox writable.
    /// A failed probe is not cached (None): it is retried after a backoff, and
    /// undervolt is never sent until a probe says unlocked.
    async fn probe_undervolt(&self) -> Option<bool> {
        match self.svc.output(&[PKEXEC, ROOT_HELPER, "undervolt", "probe"]).await {
            Ok(out) => {
                let unlocked = out.trim() == "unlocked";
                *self.uv.lock().unwrap() = Some(unlocked);
                Some(unlocked)
            }
            Err(e) => { eprintln!("armouryd: undervolt probe: {e:#}"); None }
        }
    }

    /// Before handing the hardware back: undervolt and NVIDIA clocks persist until
    /// reboot, so return them to stock rather than leave them under G-Helper.
    async fn reset_to_stock(&self) {
        let mut st = self.apply.lock().await;
        let dgpu_active = self.refresh().await.gpu.dgpu_active == Some(true);
        let uv_unlocked = *self.uv.lock().unwrap() == Some(true);
        if uv_unlocked && !st.uv_at_stock {
            if let Err(e) = self.svc.run(&[PKEXEC, ROOT_HELPER, "undervolt", "set", "0"]).await { eprintln!("armouryd: reset undervolt: {e:#}"); }
        }
        if dgpu_active && !st.nv_at_stock {
            for e in apply_nv(&ModeSettings::default(), &*self.svc).await { eprintln!("armouryd: reset GPU clocks: {e}"); }
        }
        *st = ApplyState::default();
    }

    /// Executes one planned GPU step (for a two-step path, only the first).
    async fn run_gpu_step(&self, step: &GpuStep) -> anyhow::Result<(bool, String)> {
        match step {
            GpuStep::FirstOfTwo { first, .. } => Box::pin(self.run_gpu_step(first)).await,
            GpuStep::OmarchyToggle { to } => {
                // Exactly what the Omarchy menu runs; it confirms, rewrites supergfxd.conf and reboots.
                // Absolute paths: at login armouryd may start before uwsm puts Omarchy's bin on PATH.
                let bin = format!("{}/bin", std::env::var("OMARCHY_PATH").unwrap_or_else(|_| "/usr/share/omarchy".into()));
                let launcher = format!("{bin}/omarchy-launch-floating-terminal-with-presentation");
                let toggle = format!("{bin}/omarchy-toggle-hybrid-gpu");
                self.svc.spawn(&[&launcher, &toggle], &bin).await.context("launch Omarchy's GPU toggle")?;
                Ok((true, format!("Omarchy's GPU toggle opened in a terminal; confirm there to switch to {} (it reboots)", gpu_name(*to))))
            }
            GpuStep::Supergfx { to } => {
                // Not retried: a timed-out call may still complete inside supergfxd.
                let action = match tokio::time::timeout(crate::hw::CALL_TIMEOUT * 2, self.gfx.set_mode(to.code())).await {
                    Ok(Ok(a)) => a,
                    failed => {
                        let why = match failed { Ok(Err(e)) => format!("{e:#}"), _ => "timed out".into() };
                        match with_retry(|| self.gfx.pending_mode()).await {
                            Ok(p) if p == to.code() => 1, // it was applied after all
                            _ => anyhow::bail!("SetMode {why}; the switch may or may not have been applied — run `armoury gpu plan {}`", gpu_name(*to)),
                        }
                    }
                };
                self.write_gpu_record(*to);
                Ok(match action {
                    0 => (false, format!("log out and back in to finish switching to {}", gpu_name(*to))),
                    4 => (false, format!("switched to {}", gpu_name(*to))),
                    _ => (true, format!("reboot to finish switching to {}", gpu_name(*to))),
                })
            }
        }
    }

    /// Boot-keyed record of a supergfxd switch made this boot (supergfxd forgets on restart).
    fn gpu_record(&self) -> Option<GpuMode> {
        let text = std::fs::read_to_string(self.state_dir.join("gpu-pending")).ok()?;
        let (boot, code) = text.trim_end().split_once(' ')?;
        let current = self.sys.read(crate::hw::sysfs::BOOT_ID).unwrap_or_default();
        (boot == current).then(|| code.parse().ok().and_then(GpuMode::from_supergfx)).flatten()
    }

    fn write_gpu_record(&self, to: GpuMode) {
        let boot = self.sys.read(crate::hw::sysfs::BOOT_ID).unwrap_or_default();
        let _ = std::fs::create_dir_all(&self.state_dir);
        if let Err(e) = std::fs::write(self.state_dir.join("gpu-pending"), format!("{boot} {}", to.code())) {
            eprintln!("armouryd: record GPU switch: {e}");
        }
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
        s.apply_error = self.apply_error.lock().unwrap().clone();
        s.gpu.armoury_pending = self.gpu_record();
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
                if req == Request::Handback && self.mode.get() == ControlMode::Active { self.reset_to_stock().await; }
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
            Request::Lighting | Request::SetBrightness { .. } | Request::SetEffect { .. } | Request::SetZonePower { .. }
            | Request::KbdIdle | Request::KbdResume => self.lighting_request(req).await,
            Request::PlanGpuMode { mode } => match plan_switch(&self.refresh().await.gpu, mode) {
                Ok(step) => Response::ok(serde_json::to_value(step).unwrap()),
                Err(e) => Response::err(e),
            },
            Request::SetGpuMode { mode } => {
                let _gpu = self.gpu_lock.lock().await; // plan + execute as one step
                if let Err(r) = self.write_guard().await { return r; }
                let step = match plan_switch(&self.refresh().await.gpu, mode) { Ok(s) => s, Err(e) => return Response::err(e) };
                if self.mode.get() != ControlMode::Active { return Response::err("observe mode: run 'armoury takeover' first"); }
                match self.run_gpu_step(&step).await {
                    Ok((reboot_required, message)) => {
                        let message = match &step {
                            GpuStep::FirstOfTwo { then, .. } => format!("Step 1 of 2: {message}. After rebooting, run `armoury gpu set {}` again.", gpu_name(*then)),
                            _ => message,
                        };
                        self.refresh().await;
                        Response::ok(serde_json::to_value(GpuSwitchResult { step, reboot_required, message }).unwrap())
                    }
                    Err(e) => Response::err(format!("{e:#}")),
                }
            }
            Request::ProbeUndervolt => {
                if let Err(r) = self.write_guard().await { return r; }
                // The probe leaves the offset at 0 mV: re-apply the mode under the same
                // lock, which also keeps two root processes off the MSR mailbox at once.
                let mut st = self.apply.lock().await;
                let Some(unlocked) = self.probe_undervolt().await else { return Response::err("undervolt probe failed; see journalctl --user -u armouryd") };
                st.applied = None;
                st.uv_at_stock = true;
                let errs = self.tick_locked(&mut st, true).await;
                if !errs.is_empty() { return Response::err(errs.join("; ")); }
                Response::ok(serde_json::json!({"unlocked": unlocked}))
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
                    let ctx = self.hw_ctx(dgpu_active, &st);
                    let errs = apply_mode(profile, &merged, ctx, &*self.asusd, &*self.svc).await;
                    self.note_apply(profile, &errs);
                    if !errs.is_empty() { return Response::err(errs.join("; ")); }
                    *st = ApplyState {
                        applied: Some(profile),
                        dgpu_was_active: dgpu_active,
                        uv_at_stock: if ctx.uv_unlocked { merged.uv_mv.unwrap_or(0) == 0 } else { st.uv_at_stock },
                        nv_at_stock: if dgpu_active { nv_is_stock(&merged) } else { st.nv_at_stock },
                        probe_retry_at: st.probe_retry_at,
                        ..Default::default()
                    };
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

/// Keyboard lighting bookkeeping (active mode).
#[derive(Default)]
struct LightState {
    /// Power source seen at the last tick.
    on_ac: Option<bool>,
    /// Brightness before KbdIdle dimmed it.
    idle_saved: Option<u8>,
}

/// Placeholder until main wires the asusd Aura client.
struct NoAura;

#[async_trait::async_trait]
impl Aura for NoAura {
    async fn info(&self) -> anyhow::Result<(crate::features::lighting::RawMode, crate::features::lighting::RawPower, Vec<u32>, Vec<u32>)> {
        anyhow::bail!("asusd has no Aura keyboard device")
    }
    async fn set_mode_data(&self, _: crate::features::lighting::RawMode) -> anyhow::Result<()> { anyhow::bail!("asusd has no Aura keyboard device") }
    async fn set_power(&self, _: crate::features::lighting::RawPower) -> anyhow::Result<()> { anyhow::bail!("asusd has no Aura keyboard device") }
    async fn set_brightness(&self, _: u32) -> anyhow::Result<()> { anyhow::bail!("asusd has no Aura keyboard device") }
}

const RETRY_BACKOFF: Duration = Duration::from_secs(30);
const PROBE_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Default)]
struct ApplyState {
    /// Profile whose settings are in force.
    applied: Option<Profile>,
    /// Profile whose last apply failed, and when to try it again.
    failed: Option<Profile>,
    retry_at: Option<tokio::time::Instant>,
    /// dGPU state at the last tick, to catch it waking up.
    dgpu_was_active: bool,
    /// Last undervolt / NVIDIA values sent were stock, so stock need not be re-sent.
    uv_at_stock: bool,
    nv_at_stock: bool,
    probe_retry_at: Option<tokio::time::Instant>,
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

/// User-facing GPU mode name (AsusMuxDgpu is "Ultimate" in G-Helper and the UI).
fn gpu_name(m: GpuMode) -> &'static str {
    match m { GpuMode::Integrated => "integrated", GpuMode::Hybrid => "hybrid", GpuMode::AsusMuxDgpu => "ultimate", _ => "other" }
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
    use crate::hw::fake::{FakeAsusd, FakeAura, FakeGfx, FakeNvidia, FakeServices, FakeSysfs};
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

    fn calls(r: &NvRig) -> Vec<String> { r.svc.calls.lock().unwrap().clone() }

    #[tokio::test]
    async fn probe_request_reapplies_undervolt() {
        let r = rig_with_nv("[modes.balanced]\nuv_mv = -30\n", "unlocked\n");
        r.d.tick().await;
        r.svc.calls.lock().unwrap().clear();
        assert!(r.d.handle(Request::ProbeUndervolt).await.ok);
        let c = calls(&r);
        assert!(c.iter().any(|c| c.ends_with("undervolt probe")));
        assert!(c.last().unwrap().ends_with("undervolt set -30"), "{c:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn reapply_timer_touches_only_limits() {
        let r = rig_with_nv("reapply_power_secs = 5\n[modes.balanced]\npl1 = 45\npl2 = 65\nuv_mv = -30\ngpu_core_offset = 50\n", "unlocked\n");
        *r.nv.active.lock().unwrap() = Some(true);
        r.d.tick().await;
        r.svc.calls.lock().unwrap().clear();
        tokio::time::advance(Duration::from_secs(6)).await;
        r.d.tick().await;
        let c = calls(&r);
        assert!(c.iter().any(|c| c.contains("set-limits pl1=45")), "{c:?}");
        assert!(!c.iter().any(|c| c.contains("nv-clocks") || c.contains("undervolt")), "{c:?}");
    }

    #[tokio::test]
    async fn handback_resets_undervolt_and_gpu_clocks() {
        let r = rig_with_nv("[modes.balanced]\nuv_mv = -30\ngpu_core_offset = 50\n", "unlocked\n");
        *r.nv.active.lock().unwrap() = Some(true);
        r.d.tick().await;
        r.svc.calls.lock().unwrap().clear();
        r.d.handle(Request::Handback).await;
        let c = calls(&r);
        let pos = |s: &str| c.iter().position(|x| x.ends_with(s)).unwrap_or_else(|| panic!("{s} missing: {c:?}"));
        assert!(pos("undervolt set 0") < pos("armoury-root handback"));
        assert!(pos("nv-clocks 0 0 off off") < pos("armoury-root handback"));
    }

    #[tokio::test]
    async fn wake_apply_error_is_reported() {
        let r = rig_with_nv("[modes.balanced]\ngpu_core_offset = 400\n", "locked\n");
        r.d.tick().await;
        *r.svc.fail_on.lock().unwrap() = Some("nv-clocks".into());
        *r.nv.active.lock().unwrap() = Some(true);
        r.d.tick().await;
        assert!(r.d.refresh().await.apply_error.unwrap().contains("NVIDIA"));
    }

    #[tokio::test]
    async fn no_probe_unless_a_mode_uses_undervolt() {
        let r = rig_with_nv("[modes.balanced]\npl1 = 45\npl2 = 65\n", "unlocked\n");
        r.d.tick().await;
        assert!(!calls(&r).iter().any(|c| c.contains("undervolt")), "{:?}", calls(&r));
    }

    #[tokio::test(start_paused = true)]
    async fn failed_probe_is_retried_not_cached() {
        let r = rig_with_nv("[modes.balanced]\nuv_mv = -30\n", "unlocked\n");
        *r.svc.fail_on.lock().unwrap() = Some("undervolt probe".into());
        r.d.tick().await;
        assert_eq!(r.d.refresh().await.perf.undervolt, None);
        *r.svc.fail_on.lock().unwrap() = None;
        tokio::time::advance(Duration::from_secs(61)).await;
        r.d.tick().await;
        assert!(calls(&r).iter().any(|c| c.ends_with("undervolt set -30")), "{:?}", calls(&r));
    }

    #[tokio::test]
    async fn stock_gpu_clocks_not_resent_on_every_wake() {
        let r = rig_with_nv("[modes.balanced]\npl1 = 45\npl2 = 65\n", "locked\n");
        for _ in 0..3 {
            *r.nv.active.lock().unwrap() = Some(true);
            r.d.tick().await;
            *r.nv.active.lock().unwrap() = Some(false);
            r.d.tick().await;
        }
        assert_eq!(calls(&r).iter().filter(|c| c.contains("nv-clocks")).count(), 1, "{:?}", calls(&r));
    }

    struct GpuRig { d: Arc<Daemon>, svc: Arc<FakeServices>, gfx: Arc<FakeGfx> }

    fn gpu_rig(mode: u32, active: bool) -> GpuRig {
        let dir = tempfile::tempdir().unwrap();
        let sys = Arc::new(FakeSysfs::with(&[
            ("sys/devices/platform/asus-nb-wmi/gpu_mux_mode", if mode == 5 { "0" } else { "1" }),
            ("sys/devices/platform/asus-nb-wmi/dgpu_disable", if mode == 1 { "1" } else { "0" }),
        ]));
        let svc = Arc::new(FakeServices::default());
        let gfx = Arc::new(FakeGfx { mode, ..Default::default() });
        let mut ctl = Control::load(dir.path());
        if active { ctl.set(ControlMode::Active).unwrap(); }
        let d = Daemon::new(Box::new(sys), Box::new(gfx.clone()), Box::new(svc.clone()),
            Box::new(FakeAsusd::default()), ctl, dir.path().join("config.toml"), Box::new(FakeNvidia::default()));
        std::mem::forget(dir);
        GpuRig { d, svc, gfx }
    }

    fn gpu_req(cmd: &str, mode: &str) -> Request {
        serde_json::from_value(serde_json::json!({"cmd": cmd, "mode": mode})).unwrap()
    }

    #[tokio::test]
    async fn gpu_switch_requires_active() {
        let r = gpu_rig(0, false);
        assert!(r.d.handle(gpu_req("set_gpu_mode", "AsusMuxDgpu")).await.error.unwrap().contains("observe"));
        assert!(r.gfx.set_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn hybrid_to_ultimate_calls_supergfx() {
        let r = gpu_rig(0, true);
        let resp = r.d.handle(gpu_req("set_gpu_mode", "AsusMuxDgpu")).await;
        assert!(resp.ok, "{resp:?}");
        assert_eq!(*r.gfx.set_calls.lock().unwrap(), [5]);
        assert_eq!(resp.data.unwrap()["reboot_required"], true);
    }

    #[tokio::test]
    async fn hybrid_to_integrated_launches_omarchy_toggle() {
        let r = gpu_rig(0, true);
        assert!(r.d.handle(gpu_req("set_gpu_mode", "Integrated")).await.ok);
        let calls = r.svc.calls.lock().unwrap().clone();
        // absolute paths, exactly the Omarchy menu's command
        assert!(calls.iter().any(|c| c.starts_with("spawn /") && c.contains("/bin/omarchy-launch-floating-terminal-with-presentation /")
            && c.ends_with("/bin/omarchy-toggle-hybrid-gpu")), "{calls:?}");
        assert!(r.gfx.set_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn integrated_to_ultimate_runs_first_step_only() {
        let r = gpu_rig(1, true);
        let resp = r.d.handle(gpu_req("set_gpu_mode", "AsusMuxDgpu")).await;
        assert!(resp.data.unwrap()["message"].as_str().unwrap().contains("Step 1 of 2"));
        assert!(r.gfx.set_calls.lock().unwrap().is_empty());
        assert!(r.svc.calls.lock().unwrap().iter().any(|c| c.contains("omarchy-toggle-hybrid-gpu")));
    }

    #[tokio::test]
    async fn plan_does_not_execute() {
        let r = gpu_rig(0, false);
        let resp = r.d.handle(gpu_req("plan_gpu_mode", "AsusMuxDgpu")).await;
        assert_eq!(resp.data.unwrap()["kind"], "supergfx");
        assert!(r.gfx.set_calls.lock().unwrap().is_empty());
        assert!(!r.svc.calls.lock().unwrap().iter().any(|c| c.contains("omarchy-toggle")));
    }

    #[tokio::test]
    async fn supergfx_switch_is_recorded_as_pending() {
        let r = gpu_rig(0, true);
        assert!(r.d.handle(gpu_req("set_gpu_mode", "AsusMuxDgpu")).await.ok);
        // supergfxd restarted and forgot its pending mode; our boot-keyed record still blocks
        let e = r.d.handle(gpu_req("set_gpu_mode", "Integrated")).await.error.unwrap();
        assert!(e.contains("reboot"), "{e}");
    }

    #[tokio::test]
    async fn toggle_launch_failure_is_reported() {
        let r = gpu_rig(0, true);
        *r.svc.fail_on.lock().unwrap() = Some("omarchy-launch-floating-terminal-with-presentation".into());
        let resp = r.d.handle(gpu_req("set_gpu_mode", "Integrated")).await;
        assert!(!resp.ok && resp.error.unwrap().contains("launch"));
    }

    #[tokio::test]
    async fn setmode_failure_rechecks_pending() {
        let r = gpu_rig(0, true);
        *r.gfx.set_fails.lock().unwrap() = true; // write errors, but supergfxd did apply it
        *r.gfx.pending_after_fail.lock().unwrap() = Some(5);
        let resp = r.d.handle(gpu_req("set_gpu_mode", "AsusMuxDgpu")).await;
        assert!(resp.ok, "{resp:?}");
        assert_eq!(r.gfx.set_calls.lock().unwrap().len(), 1, "never blind-retried");
    }

    struct LightRig { d: Arc<Daemon>, sys: Arc<FakeSysfs>, aura: Arc<FakeAura>, dir: tempfile::TempDir }

    fn light_rig(active: bool, aura: FakeAura, toml: &str) -> LightRig {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), toml).unwrap();
        let sys = Arc::new(FakeSysfs::with(&[
            ("sys/class/leds/asus::kbd_backlight/brightness", "3"),
            ("sys/class/power_supply/ADP0/type", "Mains"),
            ("sys/class/power_supply/ADP0/online", "1"),
        ]));
        let aura = Arc::new(aura);
        let mut ctl = Control::load(dir.path());
        if active { ctl.set(ControlMode::Active).unwrap(); }
        let d = Daemon::new(Box::new(sys.clone()), Box::new(FakeGfx::default()), Box::new(FakeServices::default()),
            Box::new(FakeAsusd::default()), ctl, dir.path().join("config.toml"), Box::new(FakeNvidia::default()))
            .with_aura(Box::new(aura.clone()));
        LightRig { d, sys, aura, dir }
    }

    fn req(v: serde_json::Value) -> Request { serde_json::from_value(v).unwrap() }
    fn acalls(r: &LightRig) -> Vec<String> { r.aura.calls.lock().unwrap().clone() }
    fn set_brightness_sysfs(r: &LightRig, v: &str) { r.sys.files.lock().unwrap().insert("sys/class/leds/asus::kbd_backlight/brightness".into(), v.into()); }

    #[tokio::test]
    async fn lighting_info_and_effect() {
        let r = light_rig(true, FakeAura::default(), "");
        let info = r.d.handle(Request::Lighting).await.data.unwrap();
        assert_eq!(info["effect"]["mode"], "static");
        assert_eq!(info["power_zones"].as_array().unwrap().len(), 3);
        let resp = r.d.handle(req(serde_json::json!({"cmd":"set_effect","effect":{"mode":"rainbow_wave","colour1":[255,0,0],"colour2":[0,0,0],"speed":"high","direction":"left"}}))).await;
        assert!(resp.ok, "{resp:?}");
        assert_eq!(acalls(&r), ["set_mode_data 3"]);
    }

    #[tokio::test]
    async fn unsupported_mode_refused() {
        let r = light_rig(true, FakeAura { modes: vec![0], ..Default::default() }, "");
        let resp = r.d.handle(req(serde_json::json!({"cmd":"set_effect","effect":{"mode":"comet","colour1":[0,0,0],"colour2":[0,0,0],"speed":"med","direction":"right"}}))).await;
        assert!(resp.error.unwrap().contains("not supported"));
        assert!(acalls(&r).is_empty());
    }

    #[tokio::test]
    async fn zone_power_keeps_other_zones() {
        let r = light_rig(true, FakeAura::default(), "");
        assert!(r.d.handle(req(serde_json::json!({"cmd":"set_zone_power","zone":{"zone":"logo","boot":false,"awake":false,"sleep":false,"shutdown":false}}))).await.ok);
        let p = r.aura.power.lock().unwrap().clone();
        assert_eq!(p.0, vec![(1, true, true, false, false), (2, true, true, false, false), (0, false, false, false, false)]);
        let resp = r.d.handle(req(serde_json::json!({"cmd":"set_zone_power","zone":{"zone":"rear_glow","boot":true,"awake":true,"sleep":false,"shutdown":false}}))).await;
        assert!(resp.error.unwrap().contains("not"), "unsupported zone refused");
    }

    #[tokio::test]
    async fn brightness_stored_per_power_source() {
        let r = light_rig(true, FakeAura::default(), "");
        assert!(r.d.handle(req(serde_json::json!({"cmd":"set_brightness","level":1}))).await.ok);
        assert_eq!(acalls(&r), ["set_brightness 1"]);
        let text = std::fs::read_to_string(r.dir.path().join("config.toml")).unwrap();
        assert!(text.contains("brightness_ac = 1"), "{text}");
        assert!(r.d.handle(req(serde_json::json!({"cmd":"set_brightness","level":7}))).await.error.is_some());
    }

    #[tokio::test]
    async fn brightness_follows_power_source() {
        let r = light_rig(true, FakeAura::default(), "[lighting]\nbrightness_ac = 3\nbrightness_battery = 1\n");
        r.d.tick().await; // on AC, sysfs already 3 → no write
        assert!(!acalls(&r).iter().any(|c| c.starts_with("set_brightness")), "{:?}", acalls(&r));
        r.sys.files.lock().unwrap().insert("sys/class/power_supply/ADP0/online".into(), "0".into());
        r.d.tick().await;
        assert_eq!(acalls(&r).last().unwrap(), "set_brightness 1");
        set_brightness_sysfs(&r, "1");
        r.d.tick().await; // still on battery: no repeat
        assert_eq!(acalls(&r).iter().filter(|c| c.starts_with("set_brightness")).count(), 1);
        r.sys.files.lock().unwrap().insert("sys/class/power_supply/ADP0/online".into(), "1".into());
        r.d.tick().await;
        assert_eq!(acalls(&r).last().unwrap(), "set_brightness 3");
    }

    #[tokio::test]
    async fn idle_twice_restores_original() {
        let r = light_rig(true, FakeAura::default(), "");
        set_brightness_sysfs(&r, "2");
        assert!(r.d.handle(Request::KbdIdle).await.ok);
        set_brightness_sysfs(&r, "0");
        assert!(r.d.handle(Request::KbdIdle).await.ok);
        assert!(r.d.handle(Request::KbdResume).await.ok);
        assert_eq!(acalls(&r), ["set_brightness 0", "set_brightness 2"]);
        assert!(r.d.handle(Request::KbdResume).await.ok, "resume when not idle is a no-op");
        assert_eq!(acalls(&r).len(), 2);
    }

    #[tokio::test]
    async fn keep_on_disables_idle() {
        let r = light_rig(true, FakeAura::default(), "[lighting]\nkeep_on = true\n");
        assert!(r.d.handle(Request::KbdIdle).await.ok);
        assert!(acalls(&r).is_empty());
    }

    #[tokio::test]
    async fn no_aura_device() {
        let r = light_rig(true, FakeAura { missing: true, ..Default::default() }, "");
        assert!(r.d.handle(Request::Lighting).await.error.unwrap().contains("no Aura"));
    }

    #[tokio::test]
    async fn lighting_writes_refused_in_observe() {
        let r = light_rig(false, FakeAura::default(), "");
        assert!(r.d.handle(req(serde_json::json!({"cmd":"set_brightness","level":1}))).await.error.unwrap().contains("observe"));
        assert!(r.d.handle(Request::KbdIdle).await.error.unwrap().contains("observe"));
        assert_eq!(r.d.refresh().await.lighting.brightness, Some(3), "reads still work");
    }
}
