use crate::config::Config;
use crate::control::{Control, ModeHandle, handback, takeover};
use crate::features::{fan, manual};
use crate::features::limits::{self, Bounds, Limit};
use crate::features::perf::{HwCtx, apply_mode, apply_nv, nv_is_stock};
use crate::features::lighting::{effect_from_raw, effect_to_raw, power_from_raw, set_zone, validate_effect};
use crate::features::clamshell::Clamshell;
use crate::hw::hypr::{Hypr, RealHypr, lua_monitor};
use crate::hw::{Asusd, Aura, Gfx, Nvidia, Services, Sysfs, with_retry};
use crate::state::collect;
use anyhow::{Context, bail};
use crate::features::gpu::plan_switch;
use armoury_proto::{HotKey, KeyAction, Toggle, ControlMode, Event, GpuMode, GpuStep, GpuSwitchResult, ManualProfile, ManualView, ModeChoice, ModeSettings, Profile, Request, Response, Snapshot, UndervoltState};
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
    hypr: Box<dyn Hypr>,
    clamshell: Mutex<Clamshell>,
    /// Home directory (Omarchy's toggle state and ~/.local/bin tools live under it).
    home: PathBuf,
    /// Last clamshell state reported by Omarchy's lid toggle.
    lid_awake: std::sync::Mutex<Option<bool>>,
    sys_src: std::sync::Mutex<SysSource>,
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
            hypr: Box::new(RealHypr),
            clamshell: Mutex::new(Clamshell::systemd_inhibit()),
            home: std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default(),
            lid_awake: std::sync::Mutex::new(None),
            sys_src: std::sync::Mutex::new(SysSource::default()),
            state_dir, last_reapply: std::sync::Mutex::new(tokio::time::Instant::now()),
        })
    }

    /// Sets the asusd Aura client (keyboard lighting); call before the daemon is shared.
    pub fn with_aura(self: Arc<Self>, aura: Box<dyn Aura>) -> Arc<Self> {
        let mut d = Arc::try_unwrap(self).ok().expect("with_aura before the daemon is shared");
        d.aura = aura;
        Arc::new(d)
    }

    pub fn with_hypr(self: Arc<Self>, hypr: Box<dyn Hypr>) -> Arc<Self> {
        let mut d = Arc::try_unwrap(self).ok().expect("with_hypr before the daemon is shared");
        d.hypr = hypr;
        Arc::new(d)
    }

    pub fn with_clamshell(self: Arc<Self>, c: Clamshell) -> Arc<Self> {
        let mut d = Arc::try_unwrap(self).ok().expect("with_clamshell before the daemon is shared");
        d.clamshell = Mutex::new(c);
        Arc::new(d)
    }

    pub fn with_home(self: Arc<Self>, home: PathBuf) -> Arc<Self> {
        let mut d = Arc::try_unwrap(self).ok().expect("with_home before the daemon is shared");
        d.home = home;
        Arc::new(d)
    }

    pub async fn clamshell_holding(&self) -> bool { self.clamshell.lock().await.holding() }

    /// Omarchy's on-screen display.
    /// `icon` is an omarchy-osd icon name or a literal glyph; without one it shows a muted speaker.
    async fn osd(&self, icon: &str, msg: &str) {
        let osd = self.omarchy_bin().join("omarchy-osd");
        if let Err(e) = self.svc.run(&[&osd.to_string_lossy(), "-i", icon, "-m", msg]).await { eprintln!("armouryd: osd: {e:#}"); }
    }

    /// The firmware changed the keyboard backlight (Fn+F2/F3): show the new level.
    pub async fn on_kbd_brightness(&self, level: u8) {
        if self.mode.get() != ControlMode::Active { return; }
        let name = ["off", "low", "medium", "high"].get(level as usize).copied().unwrap_or("?");
        let pct = (level as u32 * 100 + 1) / 3;
        let osd = self.omarchy_bin().join("omarchy-osd");
        let (msg, p) = (format!("Keyboard {name}"), pct.to_string());
        if let Err(e) = self.svc.run(&[&osd.to_string_lossy(), "-i", "keyboard", "-m", &msg, "-p", &p]).await {
            eprintln!("armouryd: osd: {e:#}");
        }
    }

    /// ROG key / Fn+F4 / Fn+F5. Ignored in observe mode (G-Helper owns the keys then).
    pub async fn on_hotkey(&self, key: HotKey) {
        if self.mode.get() != ControlMode::Active { return; }
        let (action, command) = {
            let k = self.config.lock().await.keys.clone();
            match key { HotKey::Rog => (k.rog, k.rog_command), HotKey::Fan => (k.fan, k.fan_command), HotKey::Aura => (k.aura, k.aura_command) }
        };
        match action {
            KeyAction::None => {}
            KeyAction::CycleMode => {
                // Keys are handled one at a time and switch_profile holds the apply lock,
                // so presses are serialized: each switch finishes before the next starts
                // (overlapping switches hung the EC on the G533ZW).
                let next = next_choice(self.refresh().await.perf.mode);
                let icon = mode_look(next);
                let label = match next {
                    ModeChoice::Manual => {
                        self.ensure_manual_profile().await;
                        let name = self.config.lock().await.manual.active_profile().map(|p| p.name.clone()).unwrap_or_default();
                        format!("Manual ({name})")
                    }
                    m => m.label().to_string(),
                };
                // omarchy-osd shows a muted speaker when no icon is given
                self.osd(icon, &format!("{label} mode")).await;
                if !self.choose_mode(next).await.ok {
                    self.osd(icon, &format!("{label} mode (settings failed)")).await;
                }
            }
            KeyAction::CycleBrightness => {
                let next = (self.refresh().await.lighting.brightness.unwrap_or(0) + 1) % 4;
                if self.handle(Request::SetBrightness { level: next }).await.ok {
                    self.osd("keyboard", &format!("Keyboard {}", ["off", "low", "medium", "high"][next as usize])).await;
                }
            }
            KeyAction::CycleEffect => {
                if let Ok((mode, _, modes, _)) = with_retry(|| self.aura.info()).await {
                    let codes: Vec<u32> = modes.into_iter().filter(|c| armoury_proto::AuraMode::from_code(*c).is_some()).collect();
                    if let Some(pos) = codes.iter().position(|c| *c == mode.0).or(Some(0)) {
                        let next = codes.get((pos + 1) % codes.len().max(1)).copied().unwrap_or(0);
                        let mut raw = mode.clone();
                        raw.0 = next;
                        if with_retry(|| self.aura.set_mode_data(raw.clone())).await.is_ok() {
                            let name = armoury_proto::AuraMode::from_code(next).map(|m| format!("{m:?}")).unwrap_or_default();
                            self.osd("keyboard", &format!("Lighting: {name}")).await;
                        }
                    }
                }
            }
            KeyAction::OpenWindow => {
                let shell = self.omarchy_bin().join("omarchy-shell");
                let sh = shell.to_string_lossy();
                // the key toggles: the window answers isOpen() only while it is loaded
                let open = self.svc.output(&[&sh, "shell", "call", "asus.armoury", "isOpen", ""]).await
                    .is_ok_and(|o| o.trim() == "open");
                if open {
                    if let Err(e) = self.svc.run(&[&sh, "shell", "hide", "asus.armoury"]).await { eprintln!("armouryd: hide window: {e:#}"); }
                    return;
                }
                // summon exits 0 either way; it prints "ok" only when the panel exists
                let opened = self.svc.output(&[&shell.to_string_lossy(), "shell", "summon", "asus.armoury", "{}"]).await
                    .is_ok_and(|o| o.trim() == "ok");
                if !opened {
                    self.osd("󰢮", "Armoury window: coming with the UI").await;
                }
            }
            KeyAction::Command => match command {
                // Detached in its own systemd scope: never waited on (a GUI app would block every
                // hotkey) and not in armouryd's cgroup, so a daemon restart does not kill it.
                // A failing command only shows as its own exit status.
                Some(cmd) => if let Err(e) = self.svc.spawn(&["/usr/bin/systemd-run", "--user", "--scope", "--quiet", "--collect", "/bin/sh", "-c", &cmd], "").await {
                    eprintln!("armouryd: key command: {e:#}");
                },
                None => eprintln!("armouryd: key action 'command' has no command configured"),
            },
        }
    }

    fn omarchy_bin(&self) -> PathBuf {
        PathBuf::from(std::env::var("OMARCHY_PATH").unwrap_or_else(|_| "/usr/share/omarchy".into())).join("bin")
    }

    /// Omarchy's "lid closed stays awake on AC" toggle, when installed. It owns the
    /// inhibitor (and suspends on unplug with the lid closed), so armouryd defers to it.
    fn lid_tool(&self) -> Option<PathBuf> {
        [self.home.join(".local/bin/omarchy-toggle-lid-ac"), self.omarchy_bin().join("omarchy-toggle-lid-ac")]
            .into_iter().find(|p| p.exists())
    }

    /// Omarchy persists a disabled touchpad here and re-applies it on every Hyprland reload.
    fn touchpad_enabled(&self) -> bool {
        !self.home.join(".local/state/omarchy/toggles/hypr/touchpad-disabled-name").exists()
    }

    /// The built-in panel. Never another output: with the lid closed Omarchy disables eDP
    /// and the first listed monitor would be an external one.
    async fn panel(&self) -> anyhow::Result<armoury_proto::DisplayInfo> {
        let mons = self.hypr.monitors().await?;
        mons.into_iter().find(|m| m.output.starts_with("eDP")).context("the built-in panel is not active (lid closed?)")
    }

    async fn set_refresh(&self, hz: f32) -> anyhow::Result<()> {
        let panel = self.panel().await?;
        if (panel.refresh_hz - hz).abs() < 0.5 { return Ok(()); }
        let lua = lua_monitor(&panel, hz).map_err(anyhow::Error::msg)?;
        self.hypr.eval(&lua).await
    }

    /// Per-source refresh rate, clamshell inhibitor and the once-per-start sleep mode.
    async fn follow_system(&self, snap: &Snapshot) {
        let cfg = self.config.lock().await.system.clone();
        let on_ac = snap.lighting.on_ac;
        let own_clamshell = cfg.clamshell && self.lid_tool().is_none();
        self.clamshell.lock().await.update(own_clamshell, on_ac == Some(true)).await;
        let now = tokio::time::Instant::now();
        let (prev, sleep_applied, retry_at, last_check) = {
            let s = self.sys_src.lock().unwrap();
            (s.on_ac, s.sleep_applied, s.retry_at, s.last_check)
        };
        if !sleep_applied {
            if let Some(m) = cfg.sleep_mode.filter(|m| snap.system.mem_sleep != Some(*m)) {
                if let Err(e) = self.svc.run(&[PKEXEC, ROOT_HELPER, "mem-sleep", m.kernel()]).await { eprintln!("armouryd: sleep mode: {e:#}"); }
            }
            self.sys_src.lock().unwrap().sleep_applied = true;
        }
        let Some(ac) = on_ac else { return };
        let desired = if ac { cfg.refresh_ac } else { cfg.refresh_battery };
        if retry_at.is_some_and(|t| now < t) { return; }
        if prev == Some(ac) {
            // a Hyprland config reload resets the monitor rule: re-check once a minute
            let due = last_check.is_none_or(|t| now.duration_since(t) >= REFRESH_RECHECK);
            if !due { return; }
            self.sys_src.lock().unwrap().last_check = Some(now);
            let Some(hz) = desired else { return };
            if let Err(e) = self.set_refresh(hz).await {
                eprintln!("armouryd: refresh rate: {e:#}");
                self.sys_src.lock().unwrap().retry_at = Some(now + REFRESH_RECHECK);
            }
            return;
        }
        if let Some(hz) = desired {
            if let Err(e) = self.set_refresh(hz).await {
                eprintln!("armouryd: refresh rate: {e:#}");
                self.sys_src.lock().unwrap().retry_at = Some(now + REFRESH_RECHECK);
                return; // flip stays unrecorded, retried after the backoff
            }
        }
        let mut s = self.sys_src.lock().unwrap();
        s.on_ac = Some(ac);
        s.retry_at = None;
        s.last_check = Some(now);
    }

    async fn system_request(&self, req: Request) -> Response {
        if let Err(r) = self.write_guard().await { return r; }
        let result: anyhow::Result<serde_json::Value> = async {
            match req {
                Request::SetChargeLimit { percent } => {
                    anyhow::ensure!((20..=100).contains(&percent), "charge limit must be 20–100 %");
                    with_retry(|| self.asusd.set_charge_limit(percent)).await?;
                    Ok(serde_json::json!({"percent": percent}))
                }
                Request::OneShotCharge => { with_retry(|| self.asusd.one_shot_charge()).await?; Ok(serde_json::json!({})) }
                Request::SetRefresh { hz } => { self.set_refresh(hz).await?; Ok(serde_json::json!({"hz": hz})) }
                Request::SetGamma { percent } => {
                    anyhow::ensure!((20..=100).contains(&percent), "gamma must be 20–100 %");
                    self.hypr.gamma(percent).await?;
                    Ok(serde_json::json!({"percent": percent}))
                }
                Request::SetToggle { toggle, on } => {
                    match toggle {
                        Toggle::Touchpad => {
                            // Omarchy's tool escapes the device name and persists the state across reloads
                            let tool = self.omarchy_bin().join("omarchy-toggle-input-device");
                            self.svc.run(&[&tool.to_string_lossy(), "touchpad", if on { "on" } else { "off" }]).await?;
                        }
                        Toggle::BootSound => with_retry(|| self.asusd.armoury_set_value("boot_sound", on as i32)).await?,
                        Toggle::PanelOd => with_retry(|| self.asusd.armoury_set_value("panel_overdrive", on as i32)).await?,
                        Toggle::Clamshell if self.lid_tool().is_some() => {
                            let tool = self.lid_tool().unwrap();
                            self.svc.run(&[&tool.to_string_lossy(), if on { "on" } else { "off" }]).await?;
                            *self.lid_awake.lock().unwrap() = Some(on);
                        }
                        Toggle::Clamshell => {
                            let mut cfg = self.config.lock().await;
                            cfg.system.clamshell = on;
                            cfg.save(&self.config_path)?;
                            drop(cfg);
                            let on_ac = self.refresh().await.lighting.on_ac == Some(true);
                            self.clamshell.lock().await.update(on, on_ac).await;
                        }
                    }
                    Ok(serde_json::json!({"toggle": toggle, "on": on}))
                }
                Request::SetSleepMode { mode } => {
                    anyhow::ensure!(self.refresh().await.system.sleep_modes.contains(&mode), "{} sleep is not supported here", mode.kernel());
                    self.svc.run(&[PKEXEC, ROOT_HELPER, "mem-sleep", mode.kernel()]).await?;
                    let mut cfg = self.config.lock().await;
                    cfg.system.sleep_mode = Some(mode);
                    cfg.save(&self.config_path)?;
                    Ok(serde_json::json!({"mode": mode}))
                }
                Request::SetSourceProfile { ac, battery } => {
                    with_retry(|| self.asusd.disable_source_switching()).await?;
                    let mut cfg = self.config.lock().await;
                    if ac.is_some() { cfg.system.profile_ac = ac; }
                    if battery.is_some() { cfg.system.profile_battery = battery; }
                    cfg.save(&self.config_path)?;
                    Ok(serde_json::json!({"ac": ac, "battery": battery}))
                }
                Request::SetSourceRefresh { ac, battery } => {
                    let panel = self.panel().await?;
                    for hz in [ac, battery].into_iter().flatten() { lua_monitor(&panel, hz).map_err(anyhow::Error::msg)?; }
                    let mut cfg = self.config.lock().await;
                    if ac.is_some() { cfg.system.refresh_ac = ac; }
                    if battery.is_some() { cfg.system.refresh_battery = battery; }
                    cfg.save(&self.config_path)?;
                    drop(cfg);
                    self.sys_src.lock().unwrap().on_ac = None; // apply for the current source on the next tick
                    Ok(serde_json::json!({"ac": ac, "battery": battery}))
                }
                _ => unreachable!(),
            }
        }.await;
        match result { Ok(v) => Response::ok(v), Err(e) => Response::err(format!("{e:#}")) }
    }

    /// Keeps keyboard brightness per power source. Between flips it tracks the level the
    /// user actually has (Fn keys included); on a flip it stores that level for the old
    /// source and applies the new source's. The flip only counts once handled, so a failed
    /// write (asusd still starting after resume) is retried on the next tick.
    async fn follow_power_source(&self, snap: &Snapshot) {
        let Some(on_ac) = snap.lighting.on_ac else { return };
        let current = snap.lighting.brightness;
        let (prev, idle, last) = {
            let mut l = self.light.lock().unwrap();
            if l.on_ac == Some(on_ac) {
                if l.idle_saved.is_none() { l.last_brightness = current; }
                return;
            }
            (l.on_ac, l.idle_saved, l.last_brightness)
        };
        let target = {
            let mut cfg = self.config.lock().await;
            if let (Some(old), None, Some(level)) = (prev, idle, last) {
                let slot = if old { &mut cfg.lighting.brightness_ac } else { &mut cfg.lighting.brightness_battery };
                if *slot != Some(level) {
                    *slot = Some(level);
                    if let Err(e) = cfg.save(&self.config_path) { eprintln!("armouryd: save keyboard brightness: {e}"); }
                }
            }
            if on_ac { cfg.lighting.brightness_ac } else { cfg.lighting.brightness_battery }
        };
        if idle.is_some() {
            // dimmed: resume should bring back the new source's level
            let mut l = self.light.lock().unwrap();
            if target.is_some() { l.idle_saved = target; }
            l.on_ac = Some(on_ac);
            return;
        }
        if let Some(t) = target.filter(|t| current != Some(*t)) {
            if let Err(e) = with_retry(|| self.aura.set_brightness(t as u32)).await {
                eprintln!("armouryd: keyboard brightness: {e:#}");
                return; // retry next tick
            }
        }
        let mut l = self.light.lock().unwrap();
        l.on_ac = Some(on_ac);
        l.last_brightness = target.or(current);
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
                    if current == 0 { return Ok(serde_json::json!({"dimmed": true})); }
                    // save only the first level; dim whenever the backlight is on
                    let saved_now = {
                        let mut l = self.light.lock().unwrap();
                        let first = l.idle_saved.is_none();
                        if first { l.idle_saved = Some(current); }
                        first
                    };
                    if let Err(e) = with_retry(|| self.aura.set_brightness(0)).await {
                        if saved_now { self.light.lock().unwrap().idle_saved = None; }
                        return Err(e);
                    }
                    Ok(serde_json::json!({"dimmed": true}))
                }
                Request::KbdResume => {
                    let saved = self.light.lock().unwrap().idle_saved.take();
                    let current = self.refresh().await.lighting.brightness.unwrap_or(0);
                    // a level set while dimmed (Fn key) wins over the saved one
                    if let Some(level) = saved.filter(|l| *l > 0 && current == 0) {
                        with_retry(|| self.aura.set_brightness(level as u32)).await?;
                    }
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
            *self.sys_src.lock().unwrap() = SysSource::default();
            self.clamshell.lock().await.update(false, false).await;
            return Vec::new();
        }
        self.follow_power_source(&snap).await;
        self.follow_system(&snap).await;
        let snap = if self.follow_source_mode(&snap, st).await { self.refresh().await } else { snap };
        let now = tokio::time::Instant::now();
        if self.uv.lock().unwrap().is_none() && self.config.lock().await.manual.profiles.iter().any(|p| p.settings.uv_mv.is_some())
            && st.probe_retry_at.is_none_or(|t| now >= t)
        {
            match self.probe_undervolt().await {
                None => st.probe_retry_at = Some(now + PROBE_BACKOFF),
                // the mode may already be applied without undervolt: apply it again
                Some(true) => { st.applied = None; st.uv_at_stock = true; }
                Some(false) => {}
            }
        }
        let Some(fw) = snap.perf.profile else { return Vec::new() };
        let dgpu_active = snap.gpu.dgpu_active == Some(true);
        let woke = dgpu_active && !st.dgpu_was_active;
        st.dgpu_was_active = dgpu_active;
        let (target, manual) = self.resolve_target(fw, st).await;
        let settings = manual.as_ref().map(|p| p.settings).unwrap_or_default();
        let base = manual.as_ref().map_or(fw, |p| p.base);
        let secs = self.config.lock().await.reapply_power_secs;
        let due = secs > 0 && now.duration_since(*self.last_reapply.lock().unwrap()) >= Duration::from_secs(secs as u64);
        if st.applied.as_ref() == Some(&target) {
            st.mode_just_set = false;
            let mut errs = Vec::new();
            if woke && !(nv_is_stock(&settings) && st.nv_at_stock) {
                // NVIDIA settings could not be sent while the dGPU slept; send them now it is awake.
                errs = apply_nv(&settings, &*self.svc).await;
                st.nv_at_stock = errs.is_empty() && nv_is_stock(&settings);
            }
            if due && manual.is_some() {
                // only Manual carries limits; stock modes are the firmware's own
                errs.extend(apply_mode(base, &settings, HwCtx { limits_only: true, curves_on: true, ..self.hw_ctx(dgpu_active, st) }, &*self.asusd, &*self.svc).await);
                *self.last_reapply.lock().unwrap() = now;
            }
            self.note_apply(&target, &errs);
            return errs;
        }
        if !force && st.failed.as_ref() == Some(&target) && st.retry_at.is_some_and(|t| now < t) { return Vec::new(); }
        let ctx = HwCtx { cpu_boost_now: snap.perf.cpu_boost, ..self.hw_ctx(dgpu_active, st) };
        // one-time cleanup only when leaving Manual (its base slot) or on the first
        // apply after start/takeover (G-Helper may have left the current slot's curves on)
        let cleanup = match &st.applied {
            Some(Target::Manual { base, .. }) => Some(*base),
            None => Some(fw),
            Some(Target::Stock(_)) => None,
        };
        let errs = match &manual {
            Some(p) => manual::apply_manual(p, ctx, &*self.asusd, &*self.svc).await,
            None => manual::apply_stock(fw, cleanup, ctx, &*self.asusd, &*self.svc).await,
        };
        self.note_apply(&target, &errs);
        if errs.is_empty() {
            *st = ApplyState {
                applied: Some(target),
                dgpu_was_active: dgpu_active,
                uv_at_stock: if ctx.uv_unlocked { settings.uv_mv.unwrap_or(0) == 0 } else { st.uv_at_stock },
                nv_at_stock: if dgpu_active { nv_is_stock(&settings) } else { st.nv_at_stock },
                probe_retry_at: st.probe_retry_at,
                ..Default::default()
            };
            *self.last_reapply.lock().unwrap() = now;
        } else {
            *st = ApplyState { failed: Some(target), retry_at: Some(now + RETRY_BACKOFF), dgpu_was_active: dgpu_active, probe_retry_at: st.probe_retry_at, ..Default::default() };
        }
        errs
    }

    /// Records the outcome for `Snapshot.apply_error` and the journal.
    fn note_apply(&self, target: &Target, errs: &[String]) {
        let label = match target { Target::Stock(p) => p.sysfs().to_string(), Target::Manual { name, .. } => format!("manual ({name})") };
        for e in errs { eprintln!("armouryd: apply {label}: {e}"); }
        *self.apply_error.lock().unwrap() = (!errs.is_empty()).then(|| format!("{label}: {}", errs.join("; ")));
    }

    fn hw_ctx(&self, dgpu_active: bool, st: &ApplyState) -> HwCtx {
        HwCtx {
            uv_unlocked: self.uv.lock().unwrap().unwrap_or(false),
            dgpu_active,
            limits_only: false,
            uv_at_stock: st.uv_at_stock,
            nv_at_stock: st.nv_at_stock,
            mode_just_set: st.mode_just_set,
            cpu_boost_now: None,
            curves_on: false,
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

    /// Firmware ranges where reported, fallbacks otherwise.
    async fn bounds(&self) -> impl Fn(Limit) -> Bounds + use<> {
        let mut found = Vec::new();
        for l in Limit::ALL {
            found.push((l, limits::bounds(l, with_retry(|| self.asusd.armoury_range(l.attr())).await.ok())));
        }
        move |l| found.iter().find(|(k, _)| *k == l).map(|(_, b)| *b).unwrap()
    }

    /// Changes the thermal policy and applies the new mode as one step under the apply
    /// lock, so a second mode change never starts while the previous one is still writing.
    async fn switch_profile(&self, p: Profile) -> Response {
        let mut st = self.apply.lock().await;
        if let Err(e) = with_retry(|| self.asusd.set_profile(p.to_asusd())).await { return Response::err(format!("{e:#}")); }
        st.mode_just_set = true;
        let errs = self.tick_locked(&mut st, true).await;
        st.mode_just_set = false;
        drop(st);
        if !errs.is_empty() {
            return Response::err(format!("mode changed, but its settings failed: {}", errs.join("; ")));
        }
        Response::ok(serde_json::to_value(self.refresh().await).unwrap())
    }

    /// On a real AC↔battery flip, set up the mode configured for the new source; the rest of
    /// this tick applies it. Runs inside tick_locked (apply lock held), so it changes the
    /// policy directly instead of calling choose_mode, which would deadlock.
    /// The first tick after start/takeover only records the source.
    async fn follow_source_mode(&self, snap: &Snapshot, st: &mut ApplyState) -> bool {
        let Some(ac) = snap.lighting.on_ac else { return false };
        let prev = self.sys_src.lock().unwrap().mode_on_ac.replace(ac);
        if prev != Some(!ac) { return false; }
        let want = { let c = self.config.lock().await; if ac { c.system.profile_ac } else { c.system.profile_battery } };
        let Some(choice) = want else { return false };
        if choice == ModeChoice::Manual { self.ensure_manual_profile().await; }
        {
            let mut cfg = self.config.lock().await;
            cfg.manual.enabled = choice == ModeChoice::Manual;
            if let Err(e) = cfg.save(&self.config_path) { eprintln!("armouryd: save config: {e}"); }
        }
        if let Some(p) = choice.stock() {
            if let Err(e) = with_retry(|| self.asusd.set_profile(p.to_asusd())).await {
                eprintln!("armouryd: power-source mode: {e:#}");
                return false;
            }
            st.mode_just_set = true;
        }
        true
    }

    /// Manual when it's on, unless the firmware mode was changed away from the applied
    /// manual base by something else: then Manual is switched off and that stock mode is kept.
    async fn resolve_target(&self, fw: Profile, st: &ApplyState) -> (Target, Option<ManualProfile>) {
        // one lock per statement: two guards in one expression would deadlock
        let needs_profile = { let c = self.config.lock().await; c.manual.enabled && c.manual.profiles.is_empty() };
        if needs_profile { self.ensure_manual_profile().await; }
        let mut cfg = self.config.lock().await;
        if !cfg.manual.enabled { return (Target::Stock(fw), None); }
        if let Some(Target::Manual { base, .. }) = &st.applied {
            if *base != fw {
                cfg.manual.enabled = false;
                if let Err(e) = cfg.save(&self.config_path) { eprintln!("armouryd: save config: {e}"); }
                eprintln!("armouryd: mode changed to {} outside armouryd; leaving Manual", fw.sysfs());
                return (Target::Stock(fw), None);
            }
        }
        match cfg.manual.active_profile().cloned() {
            Some(p) => (Target::Manual { name: p.name.clone(), base: p.base }, Some(p)),
            None => (Target::Stock(fw), None),
        }
    }

    /// Creates "Manual 1" from asusd's current Turbo curves when no profile exists yet.
    async fn ensure_manual_profile(&self) {
        if !self.config.lock().await.manual.profiles.is_empty() { return; }
        let raw = with_retry(|| self.asusd.fan_curves(Profile::Performance.to_asusd())).await.unwrap_or_default();
        let mut cfg = self.config.lock().await;
        if cfg.manual.profiles.is_empty() {
            let _ = cfg.manual.save(manual::default_profile("Manual 1", &raw), None);
            if let Err(e) = cfg.save(&self.config_path) { eprintln!("armouryd: save config: {e}"); }
        }
    }

    /// The user picked a mode (UI, CLI, mode key). Takes the apply lock (via switch_profile /
    /// reapply_manual), so it must never be called from inside tick_locked.
    async fn choose_mode(&self, choice: ModeChoice) -> Response {
        match choice.stock() {
            Some(p) => {
                {
                    let mut cfg = self.config.lock().await;
                    if cfg.manual.enabled {
                        cfg.manual.enabled = false;
                        if let Err(e) = cfg.save(&self.config_path) { return Response::err(format!("save config: {e}")); }
                    }
                }
                self.switch_profile(p).await
            }
            None => {
                self.ensure_manual_profile().await;
                {
                    let mut cfg = self.config.lock().await;
                    cfg.manual.enabled = true;
                    if let Err(e) = cfg.save(&self.config_path) { return Response::err(format!("save config: {e}")); }
                }
                let errs = self.reapply_manual().await;
                if !errs.is_empty() { return Response::err(format!("Manual on, but applying failed: {}", errs.join("; "))); }
                Response::ok(serde_json::to_value(self.refresh().await).unwrap())
            }
        }
    }

    /// Re-applies Manual from scratch (profile edited, activated or deleted).
    async fn reapply_manual(&self) -> Vec<String> {
        let mut st = self.apply.lock().await;
        st.applied = None;
        st.failed = None;
        self.tick_locked(&mut st, true).await
    }

    async fn manual_view(&self) -> ManualView {
        let m = self.config.lock().await.manual.clone();
        let active = m.active_profile().map(|p| p.name.clone());
        ManualView { enabled: m.enabled, active, profiles: m.profiles }
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
        s.system.touchpad = Some(self.touchpad_enabled());
        {
            let cfg = self.config.lock().await;
            s.perf.mode = if mode == ControlMode::Active && cfg.manual.enabled { Some(ModeChoice::Manual) } else { s.perf.profile.map(ModeChoice::from) };
            s.perf.manual_profile = cfg.manual.active_profile().map(|p| p.name.clone());
        }
        s.system.clamshell = match self.lid_tool() {
            Some(tool) => {
                if gpu_detail {
                    let awake = self.svc.output(&[&tool.to_string_lossy(), "status"]).await.is_ok_and(|o| o.trim() == "awake");
                    *self.lid_awake.lock().unwrap() = Some(awake);
                }
                self.lid_awake.lock().unwrap().unwrap_or(false)
            }
            None => self.config.lock().await.system.clamshell,
        };
        if gpu_detail { s.display = self.hypr.monitors().await.unwrap_or_default(); }
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
                self.choose_mode(profile).await
            }
            Request::NextProfile => {
                if let Err(r) = self.write_guard().await { return r; }
                let cur = self.refresh().await.perf.mode;
                self.choose_mode(next_choice(cur)).await
            }
            Request::LimitBounds => {
                let b = self.bounds().await;
                let bounds: serde_json::Map<String, serde_json::Value> =
                    Limit::ALL.iter().map(|l| (l.key().to_string(), serde_json::json!([b(*l).min, b(*l).max]))).collect();
                Response::ok(serde_json::Value::Object(bounds))
            }
            Request::ManualProfiles => Response::ok(serde_json::to_value(self.manual_view().await).unwrap()),
            Request::DefaultCurves { base } => {
                if let Err(r) = self.write_guard().await { return r; }
                if let Err(e) = with_retry(|| self.asusd.reset_fan_curves(base.to_asusd())).await { return Response::err(format!("{e:#}")); }
                let raw = match with_retry(|| self.asusd.fan_curves(base.to_asusd())).await { Ok(r) => r, Err(e) => return Response::err(format!("{e:#}")) };
                // the reset also cleared Manual's curves if they live in that slot
                let in_slot = { let c = self.config.lock().await; c.manual.enabled && c.manual.active_profile().is_some_and(|p| p.base == base) };
                if in_slot { self.reapply_manual().await; }
                Response::ok(serde_json::to_value(raw.iter().filter_map(fan::from_raw).collect::<Vec<_>>()).unwrap())
            }
            Request::SaveManualProfile { mut profile, original_name } => {
                if let Err(r) = self.write_guard().await { return r; }
                if profile.curves.is_empty() {
                    // a brand-new profile starts from its base mode's current curves (read only)
                    let raw = with_retry(|| self.asusd.fan_curves(profile.base.to_asusd())).await.unwrap_or_default();
                    profile.curves = manual::default_profile(&profile.name, &raw).curves;
                }
                if let Err(e) = manual::validate_profile(&profile, self.bounds().await) { return Response::err(e); }
                let reapply = {
                    let mut cfg = self.config.lock().await;
                    let was_active = cfg.manual.active_profile().map(|p| p.name.clone());
                    if let Err(e) = cfg.manual.save(profile.clone(), original_name.as_deref()) { return Response::err(e); }
                    if let Err(e) = cfg.save(&self.config_path) { return Response::err(format!("save config: {e}")); }
                    cfg.manual.enabled && (was_active == original_name.or(Some(profile.name.clone())))
                };
                if reapply {
                    let errs = self.reapply_manual().await;
                    if !errs.is_empty() { return Response::err(format!("saved, but applying failed: {}", errs.join("; "))); }
                }
                Response::ok(serde_json::to_value(self.manual_view().await).unwrap())
            }
            Request::ActivateManualProfile { name } => {
                if let Err(r) = self.write_guard().await { return r; }
                {
                    let mut cfg = self.config.lock().await;
                    if let Err(e) = cfg.manual.activate(&name) { return Response::err(e); }
                    if let Err(e) = cfg.save(&self.config_path) { return Response::err(format!("save config: {e}")); }
                }
                let errs = self.reapply_manual().await;
                if !errs.is_empty() { return Response::err(format!("activated, but applying failed: {}", errs.join("; "))); }
                Response::ok(serde_json::to_value(self.manual_view().await).unwrap())
            }
            Request::DeleteManualProfile { name } => {
                if let Err(r) = self.write_guard().await { return r; }
                let reapply = {
                    let mut cfg = self.config.lock().await;
                    let was_active = cfg.manual.active_profile().is_some_and(|p| p.name == name);
                    if let Err(e) = cfg.manual.delete(&name) { return Response::err(e); }
                    if let Err(e) = cfg.save(&self.config_path) { return Response::err(format!("save config: {e}")); }
                    was_active && cfg.manual.enabled
                };
                if reapply { self.reapply_manual().await; }
                Response::ok(serde_json::to_value(self.manual_view().await).unwrap())
            }
            Request::Lighting | Request::SetBrightness { .. } | Request::SetEffect { .. } | Request::SetZonePower { .. }
            | Request::KbdIdle | Request::KbdResume => self.lighting_request(req).await,
            Request::SetChargeLimit { .. } | Request::OneShotCharge | Request::SetRefresh { .. } | Request::SetGamma { .. }
            | Request::SetToggle { .. } | Request::SetSleepMode { .. } | Request::SetSourceProfile { .. }
            | Request::SetSourceRefresh { .. } => self.system_request(req).await,
            Request::SetKeepOn { on } => {
                if let Err(r) = self.write_guard().await { return r; }
                let mut cfg = self.config.lock().await;
                cfg.lighting.keep_on = on;
                match cfg.save(&self.config_path) {
                    Ok(()) => Response::ok(serde_json::json!({"keep_on": on})),
                    Err(e) => Response::err(format!("save config: {e}")),
                }
            }
            Request::Config => Response::ok(serde_json::to_value(&*self.config.lock().await).unwrap()),
            Request::Keys => Response::ok(serde_json::to_value(self.config.lock().await.keys.clone()).unwrap()),
            Request::SetKeyBinding { key, action, command } => {
                if let Err(r) = self.write_guard().await { return r; }
                if action == KeyAction::Command && command.as_deref().is_none_or(|c| c.trim().is_empty()) {
                    return Response::err("action 'command' needs --command \"…\"");
                }
                let mut cfg = self.config.lock().await;
                match key {
                    HotKey::Rog => { cfg.keys.rog = action; if command.is_some() { cfg.keys.rog_command = command; } }
                    HotKey::Fan => { cfg.keys.fan = action; if command.is_some() { cfg.keys.fan_command = command; } }
                    HotKey::Aura => { cfg.keys.aura = action; if command.is_some() { cfg.keys.aura_command = command; } }
                }
                match cfg.save(&self.config_path) {
                    Ok(()) => Response::ok(serde_json::to_value(cfg.keys.clone()).unwrap()),
                    Err(e) => Response::err(format!("save config: {e}")),
                }
            }
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

/// System bookkeeping (active mode).
#[derive(Default)]
struct SysSource {
    /// Power source whose refresh rate is applied.
    on_ac: Option<bool>,
    /// Sleep mode re-applied this active run.
    sleep_applied: bool,
    /// Refresh-rate backoff after a failure (Hyprland unreachable, panel off).
    retry_at: Option<tokio::time::Instant>,
    /// Last time the applied refresh rate was re-checked.
    last_check: Option<tokio::time::Instant>,
    /// Power source seen by the mode switch (None until the first active tick).
    mode_on_ac: Option<bool>,
}

const REFRESH_RECHECK: Duration = Duration::from_secs(60);

/// Keyboard lighting bookkeeping (active mode).
#[derive(Default)]
struct LightState {
    /// Power source seen at the last tick.
    on_ac: Option<bool>,
    /// Brightness before KbdIdle dimmed it.
    idle_saved: Option<u8>,
    /// Level the user had at the last tick on the current source (not while dimmed).
    last_brightness: Option<u8>,
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
    /// What is in force.
    applied: Option<Target>,
    /// What last failed to apply, and when to try it again.
    failed: Option<Target>,
    retry_at: Option<tokio::time::Instant>,
    /// dGPU state at the last tick, to catch it waking up.
    dgpu_was_active: bool,
    /// Last undervolt / NVIDIA values sent were stock, so stock need not be re-sent.
    uv_at_stock: bool,
    nv_at_stock: bool,
    probe_retry_at: Option<tokio::time::Instant>,
    /// Set by switch_profile for the apply that follows its own policy change.
    mode_just_set: bool,
}

/// What the apply loop keeps in force.
#[derive(Debug, Clone, PartialEq)]
enum Target { Stock(Profile), Manual { name: String, base: Profile } }

/// Mode key order: Silent → Balanced → Turbo → Manual → Silent.
fn next_choice(current: Option<ModeChoice>) -> ModeChoice {
    let i = current.and_then(|c| ModeChoice::CYCLE.iter().position(|m| *m == c)).map_or(0, |i| (i + 1) % 4);
    ModeChoice::CYCLE[i]
}

fn mode_look(m: ModeChoice) -> &'static str {
    match m { ModeChoice::Quiet => "󰾆", ModeChoice::Balanced => "󰾅", ModeChoice::Performance => "󰓅", ModeChoice::Manual => "󰈐" }
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
    config.manual.profiles.retain(|p| match limits::validate(&p.settings, |l| limits::bounds(l, None)) {
        Ok(()) => true,
        Err(e) => { dropped.push(format!("manual profile {} ignored: {e}", p.name)); false }
    });
    (config, (!dropped.is_empty()).then(|| format!("config.toml: {}", dropped.join("; "))))
}

/// User-facing GPU mode name (AsusMuxDgpu is "Ultimate" in G-Helper and the UI).
fn gpu_name(m: GpuMode) -> &'static str {
    match m { GpuMode::Integrated => "integrated", GpuMode::Hybrid => "hybrid", GpuMode::AsusMuxDgpu => "ultimate", _ => "other" }
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
    use crate::hw::fake::{FakeAsusd, FakeAura, FakeGfx, FakeHypr, FakeNvidia, FakeServices, FakeSysfs};
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

    /// A config whose Manual is on, with one profile on Balanced (the rig's current mode).
    fn manual_on(pre: &str, settings: &str) -> String {
        format!("{pre}[manual]\nenabled = true\nactive = \"T\"\n[[manual.profiles]]\nname = \"T\"\nbase = \"balanced\"\nsettings = {{ {settings} }}\n")
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

    #[tokio::test]
    async fn observe_mode_never_applies() {
        let r = rig(false);
        let p: ManualProfile = serde_json::from_value(serde_json::json!({"name":"T","settings":{"pl1":60,"pl2":80}})).unwrap();
        let resp = r.d.handle(Request::SaveManualProfile { profile: p, original_name: None }).await;
        assert!(!resp.ok && resp.error.unwrap().contains("observe"));
        r.sys.files.lock().unwrap().insert("sys/firmware/acpi/platform_profile".into(), "performance".into());
        r.d.tick().await;
        assert!(r.asusd.calls.lock().unwrap().is_empty() && r.svc.calls.lock().unwrap().is_empty());
    }

    fn limit_calls(r: &Rig) -> Vec<String> {
        r.svc.calls.lock().unwrap().iter().filter(|c| c.contains("set-limits")).cloned().collect()
    }


    #[tokio::test]
    async fn set_mode_settings_for_other_mode_only_saves() {
        // saving a profile that isn't in use only saves it
        let r = rig(true);
        r.d.tick().await;
        let p: ManualProfile = serde_json::from_value(serde_json::json!({"name":"Later","settings":{"pl1":120,"pl2":150}})).unwrap();
        assert!(r.d.handle(Request::SaveManualProfile { profile: p, original_name: None }).await.ok);
        assert!(!limit_calls(&r).iter().any(|c| c.contains("pl1=")), "{:?}", limit_calls(&r));
        assert_eq!(r.d.config.lock().await.manual.profiles.len(), 1);
    }


    #[tokio::test]
    async fn invalid_settings_rejected_and_not_saved() {
        let r = rig(true);
        let bad: ManualProfile = serde_json::from_value(serde_json::json!({"name":"X","settings":{"pl1":140,"pl2":100}})).unwrap();
        let resp = r.d.handle(Request::SaveManualProfile { profile: bad, original_name: None }).await;
        assert!(resp.error.unwrap().contains("PL1 must not exceed PL2"));
        assert!(!r.dir.path().join("config.toml").exists());
        assert!(limit_calls(&r).is_empty());
    }

    #[tokio::test]
    async fn failed_set_does_not_save() {
        // renamed in spirit: a failed apply keeps the saved profile and reports the failure
        let r = rig_with_config(&manual_on("", "pl1 = 45, pl2 = 65"), true);
        r.d.tick().await;
        *r.svc.fail_on.lock().unwrap() = Some("set-limits".into());
        let p: ManualProfile = serde_json::from_value(serde_json::json!({"name":"T","base":"balanced","settings":{"pl1":60,"pl2":80}})).unwrap();
        let resp = r.d.handle(Request::SaveManualProfile { profile: p, original_name: Some("T".into()) }).await;
        assert!(!resp.ok && resp.error.unwrap().contains("power limits"));
        assert_eq!(r.d.config.lock().await.manual.profiles[0].settings.pl1, Some(60), "the edit is kept");
    }

    #[tokio::test]
    async fn fan_curve_validation_and_set() {
        let r = rig(true);
        let with_curve = |percent: serde_json::Value| -> ManualProfile { serde_json::from_value(serde_json::json!(
            {"name":"C","curves":[{"fan":"cpu","temps":[40,50,60,70,80,90,95,97],"percent":percent}]})).unwrap() };
        let resp = r.d.handle(Request::SaveManualProfile { profile: with_curve(serde_json::json!([50,40,50,60,70,80,90,100])), original_name: None }).await;
        assert!(resp.error.unwrap().contains("fan speed"));
        let resp = r.d.handle(Request::SaveManualProfile { profile: with_curve(serde_json::json!([0,10,20,30,40,60,80,100])), original_name: None }).await;
        assert!(resp.ok, "{resp:?}");
        assert_eq!(resp.data.unwrap()["profiles"][0]["curves"][0]["percent"][7], 100);
    }

    #[tokio::test(start_paused = true)]
    async fn reapply_timer() {
        let r = rig_with_config(&manual_on("", "pl1 = 60, pl2 = 80"), true);
        r.d.tick().await;
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
        std::fs::write(dir.path().join("config.toml"), manual_on("", "pl1 = 45, pl2 = 65").replace("enabled = true", "enabled = false")).unwrap();
        let sys = Arc::new(FakeSysfs::with(&[("sys/firmware/acpi/platform_profile", "balanced")]));
        let svc = FakeServices::default();
        *svc.fail_on.lock().unwrap() = Some("set-limits".into());
        let mut ctl = Control::load(dir.path());
        ctl.set(ControlMode::Active).unwrap();
        let d = Daemon::new(Box::new(sys), Box::new(FakeGfx::default()), Box::new(svc),
            Box::new(FakeAsusd::default()), ctl, dir.path().join("config.toml"), Box::new(FakeNvidia::default()));
        let resp = d.handle(Request::SetProfile { profile: ModeChoice::Manual }).await;
        let err = resp.error.expect("apply failure must be reported");
        assert!(err.contains("applying failed") && err.contains("power limits"), "{err}");
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
    async fn invalid_config_values_dropped_and_reported() {
        let r = rig_with_config("[[manual.profiles]]\nname = \"Hot\"\nsettings = { pl1 = 200, pl2 = 150 }\n[[manual.profiles]]\nname = \"Ok\"\nsettings = { pl1 = 45, pl2 = 65 }\n", true);
        let names: Vec<String> = r.d.config.lock().await.manual.profiles.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, ["Ok"]);
        assert!(r.d.refresh().await.config_error.unwrap().contains("Hot"));
    }

    #[tokio::test]
    async fn corrupt_config_moved_aside_and_reported() {
        let r = rig_with_config("reapply_power_secs = \"often\"\n", true);
        assert!(r.dir.path().join("config.toml.bad").exists());
        assert!(r.d.refresh().await.config_error.unwrap().contains("config.toml.bad"));
        assert!(r.d.handle(sreq(serde_json::json!({"cmd":"set_profile","profile":"quiet"}))).await.ok);
        assert_eq!(std::fs::read_to_string(r.dir.path().join("config.toml.bad")).unwrap(), "reapply_power_secs = \"often\"\n");
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
        let r = rig_with_nv(&manual_on("", "gpu_core_offset = 50"), "locked\n");
        r.d.tick().await;
        assert!(!r.svc.calls.lock().unwrap().iter().any(|c| c.contains("nv-clocks")));
        *r.nv.active.lock().unwrap() = Some(true);
        r.d.tick().await;
        assert!(r.svc.calls.lock().unwrap().iter().any(|c| c.ends_with("nv-clocks 50 0 off off")), "{:?}", r.svc.calls.lock().unwrap());
    }

    #[tokio::test]
    async fn probe_result_reported_and_used() {
        let r = rig_with_nv(&manual_on("", "uv_mv = -30"), "unlocked\n");
        r.d.tick().await;
        let calls = r.svc.calls.lock().unwrap().clone();
        assert!(calls.iter().any(|c| c.ends_with("undervolt probe")));
        assert!(calls.iter().any(|c| c.ends_with("undervolt set -30")), "{calls:?}");
        assert_eq!(r.d.refresh().await.perf.undervolt, Some(armoury_proto::UndervoltState { unlocked: true }));
    }

    #[tokio::test]
    async fn locked_probe_never_sends_undervolt() {
        let r = rig_with_nv(&manual_on("", "uv_mv = -30"), "locked\n");
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
        let r = rig_with_nv(&manual_on("", "uv_mv = -30"), "unlocked\n");
        r.d.tick().await;
        r.svc.calls.lock().unwrap().clear();
        assert!(r.d.handle(Request::ProbeUndervolt).await.ok);
        let c = calls(&r);
        assert!(c.iter().any(|c| c.ends_with("undervolt probe")));
        assert!(c.last().unwrap().ends_with("undervolt set -30"), "{c:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn reapply_timer_touches_only_limits() {
        let r = rig_with_nv(&manual_on("reapply_power_secs = 5
", "pl1 = 45, pl2 = 65, uv_mv = -30, gpu_core_offset = 50"), "unlocked\n");
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
        let r = rig_with_nv(&manual_on("", "uv_mv = -30, gpu_core_offset = 50"), "unlocked\n");
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
        let r = rig_with_nv(&manual_on("", "uv_mv = -30"), "unlocked\n");
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

    struct LightRig { d: Arc<Daemon>, sys: Arc<FakeSysfs>, aura: Arc<FakeAura>, svc: Arc<FakeServices>, dir: tempfile::TempDir }

    fn light_rig(active: bool, aura: FakeAura, toml: &str) -> LightRig {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), toml).unwrap();
        let sys = Arc::new(FakeSysfs::with(&[
            ("sys/class/leds/asus::kbd_backlight/brightness", "3"),
            ("sys/class/power_supply/ADP0/type", "Mains"),
            ("sys/class/power_supply/ADP0/online", "1"),
        ]));
        let aura = Arc::new(aura);
        let svc = Arc::new(FakeServices::default());
        let mut ctl = Control::load(dir.path());
        if active { ctl.set(ControlMode::Active).unwrap(); }
        let d = Daemon::new(Box::new(sys.clone()), Box::new(FakeGfx::default()), Box::new(svc.clone()),
            Box::new(FakeAsusd::default()), ctl, dir.path().join("config.toml"), Box::new(FakeNvidia::default()))
            .with_aura(Box::new(aura.clone()));
        LightRig { d, sys, aura, svc, dir }
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

    fn unplug(r: &LightRig, on: bool) { r.sys.files.lock().unwrap().insert("sys/class/power_supply/ADP0/online".into(), if on { "1" } else { "0" }.into()); }
    fn stored(r: &LightRig) -> String { std::fs::read_to_string(r.dir.path().join("config.toml")).unwrap_or_default() }

    #[tokio::test]
    async fn flip_retried_after_failed_write() {
        let r = light_rig(true, FakeAura::default(), "[lighting]\nbrightness_ac = 3\nbrightness_battery = 1\n");
        r.d.tick().await;
        *r.aura.fail.lock().unwrap() = true; // asusd still coming back after resume
        unplug(&r, false);
        r.d.tick().await;
        *r.aura.fail.lock().unwrap() = false;
        r.d.tick().await;
        assert_eq!(acalls(&r).last().unwrap(), "set_brightness 1", "{:?}", acalls(&r));
    }

    #[tokio::test]
    async fn idle_flip_resume_restores_new_source_level() {
        let r = light_rig(true, FakeAura::default(), "[lighting]\nbrightness_ac = 3\nbrightness_battery = 1\n");
        r.d.tick().await;
        assert!(r.d.handle(Request::KbdIdle).await.ok);
        set_brightness_sysfs(&r, "0");
        unplug(&r, false);
        r.d.tick().await;
        assert!(r.d.handle(Request::KbdResume).await.ok);
        assert_eq!(acalls(&r).last().unwrap(), "set_brightness 1", "{:?}", acalls(&r));
    }

    #[tokio::test]
    async fn fn_key_level_remembered_per_source() {
        let r = light_rig(true, FakeAura::default(), "[lighting]\nbrightness_ac = 3\n");
        r.d.tick().await;
        set_brightness_sysfs(&r, "1"); // Fn key on AC
        r.d.tick().await;
        unplug(&r, false);
        r.d.tick().await;
        assert!(stored(&r).contains("brightness_ac = 1"), "{}", stored(&r));
        unplug(&r, true);
        r.d.tick().await;
        assert!(!acalls(&r).contains(&"set_brightness 3".to_string()), "must not jump back to 3: {:?}", acalls(&r));
    }

    #[tokio::test]
    async fn failed_dim_does_not_stick() {
        let r = light_rig(true, FakeAura::default(), "");
        *r.aura.fail.lock().unwrap() = true;
        assert!(!r.d.handle(Request::KbdIdle).await.ok);
        *r.aura.fail.lock().unwrap() = false;
        assert!(r.d.handle(Request::KbdIdle).await.ok);
        assert_eq!(acalls(&r).last().unwrap(), "set_brightness 0");
    }

    #[tokio::test]
    async fn resume_keeps_level_changed_while_dimmed() {
        let r = light_rig(true, FakeAura::default(), "");
        assert!(r.d.handle(Request::KbdIdle).await.ok);
        set_brightness_sysfs(&r, "2"); // Fn press woke it
        assert!(r.d.handle(Request::KbdResume).await.ok);
        assert_eq!(acalls(&r), ["set_brightness 0"], "resume must not overwrite the user's choice");
    }

    struct SysRig { d: Arc<Daemon>, sys: Arc<FakeSysfs>, asusd: Arc<FakeAsusd>, svc: Arc<FakeServices>, hypr: Arc<FakeHypr>, dir: tempfile::TempDir }

    fn sys_rig(active: bool, toml: &str) -> SysRig {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), toml).unwrap();
        let sys = Arc::new(FakeSysfs::with(&[
            ("sys/class/power_supply/ADP0/type", "Mains"),
            ("sys/class/power_supply/ADP0/online", "1"),
            ("sys/power/mem_sleep", "[s2idle] deep"),
        ]));
        let (asusd, svc, hypr) = (Arc::new(FakeAsusd::default()), Arc::new(FakeServices::default()), Arc::new(FakeHypr::default()));
        let mut ctl = Control::load(dir.path());
        if active { ctl.set(ControlMode::Active).unwrap(); }
        let d = Daemon::new(Box::new(sys.clone()), Box::new(FakeGfx::default()), Box::new(svc.clone()),
            Box::new(asusd.clone()), ctl, dir.path().join("config.toml"), Box::new(FakeNvidia::default()))
            .with_hypr(Box::new(hypr.clone()))
            .with_clamshell(crate::features::clamshell::Clamshell::new(vec!["sleep".into(), "30".into()]))
            .with_home(dir.path().join("home"));
        SysRig { d, sys, asusd, svc, hypr, dir }
    }

    fn sreq(v: serde_json::Value) -> Request { serde_json::from_value(v).unwrap() }
    fn plug(r: &SysRig, on: bool) { r.sys.files.lock().unwrap().insert("sys/class/power_supply/ADP0/online".into(), if on { "1" } else { "0" }.into()); }

    #[tokio::test]
    async fn charge_limit_bounds() {
        let r = sys_rig(true, "");
        assert!(r.d.handle(Request::SetChargeLimit { percent: 80 }).await.ok);
        assert!(r.d.handle(Request::SetChargeLimit { percent: 10 }).await.error.unwrap().contains("20"));
        assert!(r.d.handle(Request::SetChargeLimit { percent: 101 }).await.error.is_some());
        assert!(r.d.handle(Request::OneShotCharge).await.ok);
        assert_eq!(*r.asusd.calls.lock().unwrap(), ["set_charge_limit 80", "one_shot_charge"]);
    }

    #[tokio::test]
    async fn refresh_validated_against_panel() {
        let r = sys_rig(true, "");
        assert!(r.d.handle(Request::SetRefresh { hz: 60.0 }).await.ok);
        assert!(r.hypr.evals.lock().unwrap()[0].contains("2560x1440@60"));
        assert!(r.d.handle(Request::SetRefresh { hz: 144.0 }).await.error.unwrap().contains("not available"));
        assert_eq!(r.hypr.evals.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn source_refresh_follows_power_once_per_flip() {
        let r = sys_rig(true, "[system]\nrefresh_ac = 240.0\nrefresh_battery = 60.0\n");
        r.d.tick().await; // on AC at 240 already → nothing
        assert!(r.hypr.evals.lock().unwrap().is_empty(), "{:?}", r.hypr.evals.lock().unwrap());
        plug(&r, false);
        r.d.tick().await;
        r.d.tick().await;
        assert_eq!(r.hypr.evals.lock().unwrap().len(), 1);
        assert!(r.hypr.evals.lock().unwrap()[0].contains("@60"));
        plug(&r, true);
        r.d.tick().await;
        assert!(r.hypr.evals.lock().unwrap().last().unwrap().contains("@240"));
    }

    #[tokio::test]
    async fn clamshell_follows_power() {
        let r = sys_rig(true, "");
        assert!(r.d.handle(Request::SetToggle { toggle: armoury_proto::Toggle::Clamshell, on: true }).await.ok);
        r.d.tick().await;
        assert!(r.d.clamshell_holding().await);
        plug(&r, false);
        r.d.tick().await;
        assert!(!r.d.clamshell_holding().await, "released on battery");
        plug(&r, true);
        r.d.tick().await;
        assert!(r.d.clamshell_holding().await);
        assert!(std::fs::read_to_string(r.dir.path().join("config.toml")).unwrap().contains("clamshell = true"));
    }

    #[tokio::test]
    async fn toggles_route_to_the_right_backend() {
        let r = sys_rig(true, "");
        assert!(r.d.handle(sreq(serde_json::json!({"cmd":"set_toggle","toggle":"touchpad","on":false}))).await.ok);
        assert!(r.svc.calls.lock().unwrap().iter().any(|c| c.ends_with("omarchy-toggle-input-device touchpad off")));
        assert!(r.d.handle(sreq(serde_json::json!({"cmd":"set_toggle","toggle":"panel_od","on":false}))).await.ok);
        assert!(r.d.handle(sreq(serde_json::json!({"cmd":"set_toggle","toggle":"boot_sound","on":true}))).await.ok);
        assert_eq!(*r.asusd.calls.lock().unwrap(), ["armoury_set_value panel_overdrive 0", "armoury_set_value boot_sound 1"]);
    }

    #[tokio::test]
    async fn sleep_mode_set_saved_and_reapplied_at_start() {
        let r = sys_rig(true, "");
        assert!(r.d.handle(Request::SetSleepMode { mode: armoury_proto::SleepMode::Deep }).await.ok);
        assert!(r.svc.calls.lock().unwrap().iter().any(|c| c.ends_with("mem-sleep deep")));
        let r2 = sys_rig(true, "[system]\nsleep_mode = \"deep\"\n");
        r2.d.tick().await;
        r2.d.tick().await;
        assert_eq!(r2.svc.calls.lock().unwrap().iter().filter(|c| c.ends_with("mem-sleep deep")).count(), 1, "once per start");
    }

    #[tokio::test]
    async fn source_profiles_saved_and_asusd_switching_disabled() {
        let r = sys_rig(true, "");
        assert!(r.d.handle(sreq(serde_json::json!({"cmd":"set_source_profile","ac":"manual","battery":"quiet"}))).await.ok);
        assert_eq!(*r.asusd.calls.lock().unwrap(), ["disable_source_switching"]);
        let cfg = r.d.config.lock().await.system.clone();
        assert_eq!((cfg.profile_ac, cfg.profile_battery), (Some(ModeChoice::Manual), Some(ModeChoice::Quiet)));
    }

    #[tokio::test]
    async fn power_source_flip_switches_mode() {
        let r = sys_rig(true, &format!("[system]\nprofile_ac = \"manual\"\nprofile_battery = \"quiet\"\n{GAMING}"));
        *r.asusd.mirror.lock().unwrap() = Some(r.sys.clone());
        r.sys.files.lock().unwrap().insert("sys/firmware/acpi/platform_profile".into(), "balanced".into());
        r.d.tick().await; // first tick on AC: no switch
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Balanced));
        plug(&r, false);
        r.d.tick().await;
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Quiet));
        plug(&r, true);
        r.d.tick().await;
        let s = r.d.refresh().await;
        assert_eq!(s.perf.mode, Some(ModeChoice::Manual));
        assert!(r.asusd.calls.lock().unwrap().iter().any(|c| c == "set_fan_curve 1 CPU"), "manual applied in the same tick");
    }

    #[tokio::test]
    async fn system_writes_refused_in_observe() {
        let r = sys_rig(false, "");
        assert!(r.d.handle(Request::SetChargeLimit { percent: 80 }).await.error.unwrap().contains("observe"));
        assert!(r.d.handle(Request::SetRefresh { hz: 60.0 }).await.error.unwrap().contains("observe"));
    }

    fn omarchy_home(r: &SysRig) -> std::path::PathBuf { r.dir.path().join("home") }

    #[tokio::test]
    async fn refresh_only_targets_the_builtin_panel() {
        let r = sys_rig(true, "");
        r.hypr.monitors.lock().unwrap()[0].output = "HDMI-A-1".into(); // lid closed: eDP disabled
        let e = r.d.handle(Request::SetRefresh { hz: 60.0 }).await.error.unwrap();
        assert!(e.contains("built-in"), "{e}");
        assert!(r.hypr.evals.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_reasserted_after_hyprland_reload() {
        let r = sys_rig(true, "[system]\nrefresh_ac = 60.0\n");
        r.d.tick().await;
        assert_eq!(r.hypr.evals.lock().unwrap().len(), 1);
        r.hypr.monitors.lock().unwrap()[0].refresh_hz = 240.0; // config reload put it back
        tokio::time::advance(Duration::from_secs(61)).await;
        r.d.tick().await;
        assert_eq!(r.hypr.evals.lock().unwrap().len(), 2, "{:?}", r.hypr.evals.lock().unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_failure_backs_off() {
        let r = sys_rig(true, "[system]\nrefresh_ac = 60.0\n");
        *r.hypr.fail.lock().unwrap() = true;
        for _ in 0..5 { r.d.tick().await; }
        assert_eq!(*r.hypr.attempts.lock().unwrap(), 1, "no retry inside the backoff");
        *r.hypr.fail.lock().unwrap() = false;
        tokio::time::advance(Duration::from_secs(61)).await;
        r.d.tick().await;
        assert_eq!(r.hypr.evals.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn touchpad_goes_through_omarchy_and_reads_its_state() {
        let r = sys_rig(true, "");
        assert!(r.d.handle(Request::SetToggle { toggle: armoury_proto::Toggle::Touchpad, on: false }).await.ok);
        assert!(r.svc.calls.lock().unwrap().iter().any(|c| c.ends_with("/bin/omarchy-toggle-input-device touchpad off")), "{:?}", r.svc.calls.lock().unwrap());
        assert!(r.hypr.evals.lock().unwrap().is_empty(), "no hand-built Lua for the touchpad");
        let state = omarchy_home(&r).join(".local/state/omarchy/toggles/hypr");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("touchpad-disabled-name"), "asue1403:00-04f3:319a-touchpad\n").unwrap();
        assert_eq!(r.d.refresh().await.system.touchpad, Some(false));
        std::fs::remove_file(state.join("touchpad-disabled-name")).unwrap();
        assert_eq!(r.d.refresh().await.system.touchpad, Some(true));
    }

    #[tokio::test]
    async fn clamshell_delegates_to_omarchy_lid_toggle() {
        let r = sys_rig(true, "");
        let bin = omarchy_home(&r).join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("omarchy-toggle-lid-ac"), "#!/bin/sh\n").unwrap();
        r.svc.outputs.lock().unwrap().insert("omarchy-toggle-lid-ac status".into(), "awake\n".into());
        assert!(r.d.handle(Request::SetToggle { toggle: armoury_proto::Toggle::Clamshell, on: false }).await.ok);
        assert!(r.svc.calls.lock().unwrap().iter().any(|c| c.ends_with("omarchy-toggle-lid-ac off")));
        r.d.tick().await;
        assert!(!r.d.clamshell_holding().await, "no second inhibitor of our own");
        assert!(r.d.handle(Request::Status).await.data.unwrap()["system"]["clamshell"] == true, "state from Omarchy's status");
    }

    fn svc_calls(r: &SysRig) -> Vec<String> { r.svc.calls.lock().unwrap().clone() }

    #[tokio::test]
    async fn keys_ignored_in_observe() {
        let r = sys_rig(false, "");
        r.d.on_hotkey(armoury_proto::HotKey::Fan).await;
        assert!(r.asusd.calls.lock().unwrap().is_empty() && svc_calls(&r).is_empty());
    }

    fn profile_rig(p: &str) -> SysRig {
        let r = sys_rig(true, "");
        r.sys.files.lock().unwrap().insert("sys/firmware/acpi/platform_profile".into(), p.into());
        r
    }
    fn profile_sets(r: &SysRig) -> Vec<String> {
        r.asusd.calls.lock().unwrap().iter().filter(|c| c.starts_with("set_profile") || *c == "next_profile").cloned().collect()
    }

    #[tokio::test]
    async fn fan_key_cycles_silent_balanced_turbo_with_osd() {
        // Balanced → Turbo → Manual (created on Turbo) → Silent → Balanced
        let r = profile_rig("balanced");
        *r.asusd.mirror.lock().unwrap() = Some(r.sys.clone());
        r.d.on_hotkey(armoury_proto::HotKey::Fan).await;
        assert!(svc_calls(&r).iter().any(|c| c.ends_with("-i 󰓅 -m Turbo mode")), "{:?}", svc_calls(&r));
        r.d.on_hotkey(armoury_proto::HotKey::Fan).await;
        r.d.on_hotkey(armoury_proto::HotKey::Fan).await;
        r.d.on_hotkey(armoury_proto::HotKey::Fan).await;
        // every press is its own switch, in order, each finished before the next starts
        assert_eq!(profile_sets(&r), ["set_profile 1", "set_profile 1", "set_profile 2", "set_profile 0"]);
    }

    #[tokio::test]
    async fn stock_switch_skips_cpu_boost_call_when_already_on() {
        let r = profile_rig("balanced");
        *r.asusd.mirror.lock().unwrap() = Some(r.sys.clone());
        r.sys.files.lock().unwrap().insert("sys/devices/system/cpu/intel_pstate/no_turbo".into(), "0".into());
        r.d.tick().await;
        r.svc.calls.lock().unwrap().clear();
        assert!(r.d.handle(sreq(serde_json::json!({"cmd":"set_profile","profile":"quiet"}))).await.ok);
        assert!(!svc_calls(&r).iter().any(|c| c.contains("set-limits")), "boost already on: no root call {:?}", svc_calls(&r));
        r.sys.files.lock().unwrap().insert("sys/devices/system/cpu/intel_pstate/no_turbo".into(), "1".into());
        assert!(r.d.handle(sreq(serde_json::json!({"cmd":"set_profile","profile":"balanced"}))).await.ok);
        assert!(svc_calls(&r).iter().any(|c| c.ends_with("set-limits cpu_boost=on")), "boost was off: restore it {:?}", svc_calls(&r));
    }

    #[tokio::test]
    async fn own_switch_changes_the_policy_once() {
        let r = sys_rig(true, "");
        r.sys.files.lock().unwrap().insert("sys/firmware/acpi/platform_profile".into(), "balanced".into());
        *r.asusd.mirror.lock().unwrap() = Some(r.sys.clone());
        let resp = r.d.handle(sreq(serde_json::json!({"cmd":"set_profile","profile":"performance"}))).await;
        assert!(resp.ok, "{resp:?}");
        assert_eq!(profile_sets(&r), ["set_profile 1"], "no re-assert right after our own switch");
    }

    #[tokio::test]
    async fn next_profile_request_uses_our_order() {
        let r = profile_rig("performance");
        assert!(r.d.handle(Request::NextProfile).await.ok);
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Manual), "Turbo → Manual, not asusd's own order");
        assert!(!r.asusd.calls.lock().unwrap().contains(&"next_profile".to_string()));
    }

    #[tokio::test]
    async fn firmware_brightness_change_shows_osd_when_active() {
        let r = sys_rig(true, "");
        r.d.on_kbd_brightness(2).await;
        assert!(svc_calls(&r).iter().any(|c| c.ends_with("omarchy-osd -i keyboard -m Keyboard medium -p 67")), "{:?}", svc_calls(&r));
        let o = sys_rig(false, "");
        o.d.on_kbd_brightness(2).await;
        assert!(!svc_calls(&o).iter().any(|c| c.contains("omarchy-osd")), "G-Helper owns the keys in observe mode");
    }

    #[tokio::test]
    async fn aura_key_cycles_lighting_effect() {
        let r = light_rig(true, FakeAura::default(), "");
        r.d.on_hotkey(armoury_proto::HotKey::Aura).await;
        assert!(acalls(&r).iter().any(|c| c.starts_with("set_mode_data")), "{:?}", acalls(&r));
        assert!(r.svc.calls.lock().unwrap().iter().any(|c| c.contains("-i keyboard -m Lighting")));
    }

    #[tokio::test]
    async fn osd_messages_carry_their_own_icon() {
        // omarchy-osd shows a muted-speaker glyph when no icon is given
        let r = profile_rig("balanced");
        r.d.on_hotkey(armoury_proto::HotKey::Fan).await;
        let osd: Vec<String> = svc_calls(&r).into_iter().filter(|c| c.contains("omarchy-osd")).collect();
        assert!(osd.iter().any(|c| c.ends_with("omarchy-osd -i 󰓅 -m Turbo mode")), "{osd:?}");
        let l = light_rig(true, FakeAura::default(), "[keys]\nrog = \"cycle_brightness\"\n");
        l.d.on_hotkey(armoury_proto::HotKey::Rog).await;
        let osd: Vec<String> = l.svc.calls.lock().unwrap().iter().filter(|c| c.contains("omarchy-osd")).cloned().collect();
        assert!(osd.iter().any(|c| c.contains("omarchy-osd -i keyboard -m Keyboard")), "{osd:?}");
    }

    #[tokio::test]
    async fn rog_key_falls_back_to_osd_until_the_ui_exists() {
        let r = sys_rig(true, "");
        *r.svc.fail_on.lock().unwrap() = Some("summon".into());
        r.d.on_hotkey(armoury_proto::HotKey::Rog).await;
        assert!(svc_calls(&r).iter().any(|c| c.contains("omarchy-osd") && c.contains("coming with the UI")), "{:?}", svc_calls(&r));
    }

    #[tokio::test]
    async fn failing_key_command_does_not_break_the_daemon() {
        let r = sys_rig(true, "[keys]\nrog = \"command\"\nrog_command = \"false\"\n");
        *r.svc.fail_on.lock().unwrap() = Some("sh -c false".into());
        r.d.on_hotkey(armoury_proto::HotKey::Rog).await;
        // launched detached in its own scope (never waited on, survives armouryd restarts)
        assert!(svc_calls(&r).iter().any(|c| c.starts_with("spawn ") && c.contains("systemd-run --user --scope") && c.ends_with("/bin/sh -c false")), "{:?}", svc_calls(&r));
        assert!(r.d.handle(Request::Ping).await.ok);
    }

    #[tokio::test]
    async fn key_binding_saved() {
        let r = sys_rig(true, "");
        let req: Request = serde_json::from_value(serde_json::json!({"cmd":"set_key_binding","key":"rog","action":"cycle_mode"})).unwrap();
        assert!(r.d.handle(req).await.ok);
        assert!(std::fs::read_to_string(r.dir.path().join("config.toml")).unwrap().contains("rog = \"cycle_mode\""));
        let v = r.d.handle(Request::Keys).await.data.unwrap();
        assert_eq!(v["rog"], "cycle_mode");
        let bad: Request = serde_json::from_value(serde_json::json!({"cmd":"set_key_binding","key":"fan","action":"command"})).unwrap();
        assert!(r.d.handle(bad).await.error.unwrap().contains("command"));
    }

    #[tokio::test]
    async fn brightness_key_cycles() {
        let r = light_rig(true, FakeAura::default(), "[keys]\nrog = \"cycle_brightness\"\n");
        r.d.on_hotkey(armoury_proto::HotKey::Rog).await; // at 3 → 0
        assert_eq!(acalls(&r).last().unwrap(), "set_brightness 0");
    }

    #[tokio::test]
    async fn rog_key_osd_when_shell_reports_unknown_plugin() {
        // measured: `omarchy-shell shell summon <missing>` prints "unknown" and exits 0
        let r = sys_rig(true, "");
        r.svc.outputs.lock().unwrap().insert("summon".into(), "unknown\n".into());
        r.d.on_hotkey(armoury_proto::HotKey::Rog).await;
        assert!(svc_calls(&r).iter().any(|c| c.contains("coming with the UI")), "{:?}", svc_calls(&r));
    }

    #[tokio::test]
    async fn rog_key_summons_the_plugin_panel() {
        let r = sys_rig(true, "");
        r.svc.outputs.lock().unwrap().insert("summon".into(), "ok\n".into());
        r.d.on_hotkey(armoury_proto::HotKey::Rog).await;
        assert!(svc_calls(&r).iter().any(|c| c.ends_with("shell summon asus.armoury {}")), "{:?}", svc_calls(&r));
        assert!(!svc_calls(&r).iter().any(|c| c.contains("omarchy-osd")));
    }

    #[tokio::test]
    async fn rog_key_closes_an_open_window() {
        let r = sys_rig(true, "");
        r.svc.outputs.lock().unwrap().insert("isOpen".into(), "open\n".into());
        r.d.on_hotkey(armoury_proto::HotKey::Rog).await;
        let calls = svc_calls(&r);
        assert!(calls.iter().any(|c| c.ends_with("shell hide asus.armoury")), "{calls:?}");
        assert!(!calls.iter().any(|c| c.contains("summon")), "{calls:?}");
    }

    #[tokio::test]
    async fn keep_on_request_saved() {
        let r = sys_rig(true, "");
        assert!(r.d.handle(Request::SetKeepOn { on: true }).await.ok);
        assert!(std::fs::read_to_string(r.dir.path().join("config.toml")).unwrap().contains("keep_on = true"));
    }

    #[tokio::test]
    async fn config_request_returns_current_settings() {
        let r = sys_rig(false, "[system]\nrefresh_ac = 240.0\n[lighting]\nkeep_on = true\n");
        let v = r.d.handle(Request::Config).await.data.unwrap();
        assert_eq!(v["system"]["refresh_ac"], 240.0);
        assert_eq!(v["lighting"]["keep_on"], true);
        assert_eq!(v["keys"]["fan"], "cycle_mode");
    }

    fn mrig(active: bool, toml: &str) -> Rig {
        let r = rig(active);
        std::fs::write(r.dir.path().join("config.toml"), toml).unwrap();
        let (cfg, _) = crate::config::Config::load(&r.dir.path().join("config.toml"));
        *r.d.config.try_lock().unwrap() = cfg;
        *r.asusd.mirror.lock().unwrap() = Some(r.sys.clone());
        r
    }
    fn fw(r: &Rig, p: &str) { r.sys.files.lock().unwrap().insert("sys/firmware/acpi/platform_profile".into(), p.into()); }
    fn acalls_of(r: &Rig) -> Vec<String> { r.asusd.calls.lock().unwrap().clone() }
    fn clear(r: &Rig) { r.asusd.calls.lock().unwrap().clear(); r.svc.calls.lock().unwrap().clear(); }

    const GAMING: &str = r#"
[manual]
enabled = false
active = "Gaming"
[[manual.profiles]]
name = "Gaming"
base = "performance"
settings = { pl1 = 20, pl2 = 40 }
[[manual.profiles.curves]]
fan = "cpu"
temps = [30, 40, 50, 60, 70, 80, 90, 100]
percent = [0, 10, 20, 35, 55, 75, 90, 100]
"#;

    fn set(p: &str) -> Request { sreq(serde_json::json!({"cmd":"set_profile","profile":p})) }
    #[tokio::test]
    async fn stock_mode_turns_curves_off() {
        let r = mrig(true, "");
        r.d.tick().await; // firmware balanced
        assert!(acalls_of(&r).contains(&"set_fan_curves_enabled 0 false".to_string()), "{:?}", acalls_of(&r));
        clear(&r);
        r.d.tick().await;
        assert!(acalls_of(&r).is_empty(), "applied once, not every tick: {:?}", acalls_of(&r));
    }

    #[tokio::test]
    async fn entering_manual_applies_the_profile_and_stays() {
        let r = mrig(true, GAMING);
        r.d.tick().await;
        clear(&r);
        let resp = r.d.handle(set("manual")).await;
        assert!(resp.ok, "{resp:?}");
        let calls = acalls_of(&r);
        assert_eq!(&calls[..3], ["set_profile 1", "set_fan_curve 1 CPU", "set_fan_curves_enabled 1 true"], "{calls:?}");
        assert!(limit_calls(&r).last().unwrap().ends_with("set-limits pl1=20 pl2=40 cpu_boost=on"));
        clear(&r);
        r.d.tick().await; // our own switch to the base (balanced → performance) is not an outside change
        assert!(acalls_of(&r).is_empty(), "{:?}", acalls_of(&r));
        let s = r.d.refresh().await;
        assert_eq!((s.perf.mode, s.perf.manual_profile.as_deref()), (Some(ModeChoice::Manual), Some("Gaming")));
        assert!(r.d.config.lock().await.manual.enabled);
    }

    #[tokio::test]
    async fn outside_mode_change_leaves_manual() {
        let r = mrig(true, GAMING);
        r.d.tick().await;
        assert!(r.d.handle(set("manual")).await.ok);
        clear(&r);
        fw(&r, "quiet"); // e.g. asusd's own switch
        r.d.tick().await;
        assert!(acalls_of(&r).contains(&"set_fan_curves_enabled 1 false".to_string()), "Manual's slot cleaned up: {:?}", acalls_of(&r));
        assert!(!r.d.config.lock().await.manual.enabled);
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Quiet));
    }

    #[tokio::test]
    async fn takeover_re_enters_manual() {
        let r = mrig(true, &GAMING.replace("enabled = false", "enabled = true"));
        r.d.tick().await; // first active tick (firmware still balanced)
        assert_eq!(acalls_of(&r)[0], "set_profile 1", "{:?}", acalls_of(&r));
        assert!(r.d.config.lock().await.manual.enabled, "re-entering is not an outside change");
    }

    #[tokio::test]
    async fn stock_choice_leaves_manual() {
        let r = mrig(true, GAMING);
        r.d.tick().await;
        assert!(r.d.handle(set("manual")).await.ok);
        clear(&r);
        assert!(r.d.handle(set("performance")).await.ok); // same firmware mode as the base
        assert!(acalls_of(&r).contains(&"set_fan_curves_enabled 1 false".to_string()), "{:?}", acalls_of(&r));
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Performance));
    }

    #[tokio::test]
    async fn next_profile_cycles_through_manual() {
        let r = mrig(true, GAMING);
        fw(&r, "performance");
        r.d.tick().await;
        assert!(r.d.handle(Request::NextProfile).await.ok);
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Manual));
        assert!(r.d.handle(Request::NextProfile).await.ok);
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Quiet));
    }

    #[tokio::test]
    async fn first_manual_entry_creates_a_profile_from_turbo_curves() {
        let r = mrig(true, "");
        r.asusd.curves.lock().unwrap().insert(1, vec![("CPU".into(), [0, 25, 51, 76, 102, 153, 204, 255], [30, 40, 50, 60, 70, 80, 90, 100], true)]);
        r.d.tick().await;
        assert!(r.d.handle(set("manual")).await.ok);
        let m = r.d.config.lock().await.manual.clone();
        assert_eq!((m.active.as_deref(), m.profiles.len()), (Some("Manual 1"), 1));
    }

    #[tokio::test]
    async fn saving_the_active_profile_reapplies_it() {
        let r = mrig(true, GAMING);
        r.d.tick().await;
        assert!(r.d.handle(set("manual")).await.ok);
        clear(&r);
        let mut p = r.d.config.lock().await.manual.profiles[0].clone();
        p.settings.pl1 = Some(30);
        let resp = r.d.handle(Request::SaveManualProfile { profile: p, original_name: Some("Gaming".into()) }).await;
        assert!(resp.ok, "{resp:?}");
        assert!(limit_calls(&r).last().unwrap().contains("pl1=30"), "{:?}", limit_calls(&r));
        let bad = { let mut q = r.d.config.lock().await.manual.profiles[0].clone(); q.settings.pl1 = Some(90); q.settings.pl2 = Some(50); q };
        assert!(!r.d.handle(Request::SaveManualProfile { profile: bad, original_name: None }).await.ok);
    }

    #[tokio::test]
    async fn activate_other_profile_switches_base() {
        let r = mrig(true, &format!("{GAMING}\n[[manual.profiles]]\nname = \"Quiet work\"\nbase = \"quiet\"\n"));
        r.d.tick().await;
        assert!(r.d.handle(set("manual")).await.ok);
        clear(&r);
        assert!(r.d.handle(Request::ActivateManualProfile { name: "Quiet work".into() }).await.ok);
        assert_eq!(acalls_of(&r)[0], "set_profile 2", "{:?}", acalls_of(&r));
        r.d.tick().await;
        assert!(r.d.config.lock().await.manual.enabled, "switching base for a new profile is ours");
    }

    #[tokio::test]
    async fn manual_writes_refused_in_observe_but_listing_works() {
        let r = mrig(false, GAMING);
        assert!(!r.d.handle(set("manual")).await.ok);
        assert!(!r.d.handle(Request::ActivateManualProfile { name: "Gaming".into() }).await.ok);
        let v = r.d.handle(Request::ManualProfiles).await;
        assert!(v.ok && v.data.unwrap()["profiles"][0]["name"] == "Gaming");
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Balanced), "Manual shows only while active");
    }

    #[tokio::test]
    async fn new_profile_without_curves_gets_the_base_modes_curves() {
        let r = mrig(true, "");
        r.asusd.curves.lock().unwrap().insert(2, vec![("CPU".into(), [0, 25, 51, 76, 102, 153, 204, 255], [30, 40, 50, 60, 70, 80, 90, 100], true)]);
        let p: ManualProfile = serde_json::from_value(serde_json::json!({"name":"Quiet work","base":"quiet"})).unwrap();
        assert!(r.d.handle(Request::SaveManualProfile { profile: p, original_name: None }).await.ok);
        let saved = r.d.config.lock().await.manual.profiles[0].clone();
        assert_eq!(saved.curves.len(), 1, "copied from asusd's Silent slot");
        assert!(!acalls_of(&r).iter().any(|c| c.starts_with("reset_fan_curves") || c.starts_with("set_fan_curve")), "read only: {:?}", acalls_of(&r));
    }
}
