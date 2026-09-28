use crate::hw::{Gfx, Nvidia, Services, Sysfs, sysfs, with_retry};
use armoury_proto::{BatteryInfo, SleepMode, SystemState, BatteryState, ControlMode, GpuMode, GpuPower, GpuState, LightingState, PerfState, Profile, Snapshot};

/// `gpu_detail` reads NVIDIA status and dGPU users. Only for explicit requests: polling
/// NVML every 2 s kept the dGPU from ever runtime-suspending (measured on the G533ZW).
pub async fn collect(sys: &dyn Sysfs, gfx: &dyn Gfx, svc: &dyn Services, nv: &dyn Nvidia, control: ControlMode, gpu_detail: bool) -> Snapshot {
    Snapshot {
        model: sys.read(sysfs::PRODUCT_NAME),
        control,
        keystone: sys.read(sysfs::KEYSTONE).map(|v| v == "1"),
        platform_profile: sys.read(sysfs::PLATFORM_PROFILE),
        gpu: gpu_state(sys, gfx, nv, gpu_detail).await,
        battery: battery_state(sys),
        perf: perf_state(sys),
        battery_info: battery_info(sys),
        display: Vec::new(),
        system: system_state(sys),
        lighting: LightingState {
            brightness: sys.read(sysfs::KBD_BRIGHTNESS).and_then(|v| v.parse().ok()),
            on_ac: on_ac(sys),
        },
        config_error: None,
        apply_error: None,
        asusd_running: svc.unit_active("asusd.service", false).await,
        ghelper_running: svc.is_running("ghelper").await,
    }
}

async fn gpu_state(sys: &dyn Sysfs, gfx: &dyn Gfx, nv: &dyn Nvidia, detail: bool) -> GpuState {
    let mut g = GpuState {
        mux: sys.read(sysfs::GPU_MUX).and_then(|v| v.parse().ok()),
        dgpu_disable: sys.read(sysfs::DGPU_DISABLE).and_then(|v| v.parse().ok()),
        dgpu_active: nv.dgpu_active(),
        ..Default::default()
    };
    // Never wake a sleeping dGPU just to report on it.
    if detail && g.dgpu_active == Some(true) {
        g.nvidia = nv.status();
        g.users = nv.users();
    }
    // A wedged supergfxd costs one retry budget here, not four.
    g.pending_reboot = sys.read(sysfs::PENDING_REBOOT).map(|v| v == "1");
    g.conf_mode = sys.read(sysfs::SUPERGFXD_CONF)
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| serde_json::from_value(v["mode"].clone()).ok());
    g.toggle_running = sys.list("proc").iter().filter(|p| p.bytes().all(|b| b.is_ascii_digit()))
        .any(|p| sys.read(&format!("proc/{p}/cmdline")).is_some_and(|c| c.contains("omarchy-toggle-hybrid-gpu")));
    let Ok(mode) = with_retry(|| gfx.mode()).await else { return g };
    g.mode = GpuMode::from_supergfx(mode);
    g.supported = with_retry(|| gfx.supported()).await.unwrap_or_default()
        .into_iter().filter_map(GpuMode::from_supergfx).collect();
    match with_retry(|| gfx.pending_mode()).await {
        Ok(p) => g.pending = GpuMode::from_supergfx(p).filter(|m| *m != GpuMode::None),
        Err(_) => g.pending_unknown = true,
    }
    g.power = with_retry(|| gfx.power()).await.ok().and_then(GpuPower::from_supergfx);
    g
}

fn battery_state(sys: &dyn Sysfs) -> BatteryState {
    let Some(bat) = sys.list(sysfs::POWER_SUPPLY_DIR).into_iter()
        .find(|n| sys.read(&format!("{}/{n}/type", sysfs::POWER_SUPPLY_DIR)).as_deref() == Some("Battery"))
    else { return BatteryState::default() };
    let attr = |a: &str| sys.read(&format!("{}/{bat}/{a}", sysfs::POWER_SUPPLY_DIR));
    BatteryState {
        capacity: attr("capacity").and_then(|v| v.parse().ok()),
        status: attr("status"),
        charge_limit: attr("charge_control_end_threshold").and_then(|v| v.parse().ok()),
    }
}

pub fn battery_info(sys: &dyn Sysfs) -> BatteryInfo {
    let Some(bat) = sys.list(sysfs::POWER_SUPPLY_DIR).into_iter()
        .find(|n| sys.read(&format!("{}/{n}/type", sysfs::POWER_SUPPLY_DIR)).as_deref() == Some("Battery"))
    else { return BatteryInfo::default() };
    let num = |a: &str| sys.read(&format!("{}/{bat}/{a}", sysfs::POWER_SUPPLY_DIR)).and_then(|v| v.parse::<f64>().ok());
    let volts = num("voltage_now").map(|uv| uv / 1e6);
    let design_v = num("voltage_min_design").map(|uv| uv / 1e6).or(volts);
    // energy_* is µWh; charge_* (µAh) needs a voltage
    let wh = |e: &str, c: &str| num(e).map(|v| v / 1e6).or_else(|| Some(num(c)? / 1e6 * design_v?));
    let full = wh("energy_full", "charge_full");
    let design = wh("energy_full_design", "charge_full_design");
    let now = wh("energy_now", "charge_now");
    let draw = num("power_now").map(|uw| uw / 1e6).or_else(|| Some(num("current_now")? / 1e6 * volts?));
    let status = sys.read(&format!("{}/{bat}/status", sysfs::POWER_SUPPLY_DIR));
    let time_left = match (status.as_deref(), now, draw) {
        (Some("Discharging"), Some(n), Some(d)) if d > 0.5 => Some((n / d * 60.0).round() as u32),
        _ => None,
    };
    BatteryInfo {
        capacity: num("capacity").map(|v| v as u8),
        status,
        health_pct: full.zip(design).filter(|(_, d)| *d > 0.0).map(|(f, d)| (f / d * 100.0) as f32),
        full_wh: full.map(|v| v as f32),
        design_wh: design.map(|v| v as f32),
        cycles: num("cycle_count").map(|v| v as u32).filter(|c| *c > 0),
        voltage_v: volts.map(|v| v as f32),
        draw_w: draw.map(|v| v as f32),
        time_left_min: time_left,
        charge_limit: num("charge_control_end_threshold").map(|v| v as u8),
    }
}

pub fn system_state(sys: &dyn Sysfs) -> SystemState {
    let flag = |p: &str| sys.read(p).map(|v| v == "1");
    let sleep = sys.read(sysfs::MEM_SLEEP).unwrap_or_default();
    SystemState {
        boot_sound: flag(sysfs::BOOT_SOUND),
        panel_od: flag(sysfs::PANEL_OD),
        mem_sleep: sleep.split_whitespace().find(|w| w.starts_with('[')).and_then(|w| SleepMode::from_kernel(w.trim_matches(['[', ']']))),
        sleep_modes: sleep.split_whitespace().filter_map(|w| SleepMode::from_kernel(w.trim_matches(['[', ']']))).collect(),
        // no webcam on this model; any bound UVC device counts
        camera_present: sys.list(sysfs::UVC_DRIVER).iter().any(|e| e.chars().next().is_some_and(|c| c.is_ascii_digit())),
        ..Default::default()
    }
}

/// Some(true) if any mains supply is online; None if the machine reports none.
pub fn on_ac(sys: &dyn Sysfs) -> Option<bool> {
    let mains: Vec<String> = sys.list(sysfs::POWER_SUPPLY_DIR).into_iter()
        .filter(|n| sys.read(&format!("{}/{n}/type", sysfs::POWER_SUPPLY_DIR)).as_deref() == Some("Mains"))
        .collect();
    if mains.is_empty() { return None; }
    Some(mains.iter().any(|n| sys.read(&format!("{}/{n}/online", sysfs::POWER_SUPPLY_DIR)).as_deref() == Some("1")))
}

pub fn perf_state(sys: &dyn Sysfs) -> PerfState {
    let num = |p: String| sys.read(&p).and_then(|v| v.parse::<i64>().ok());
    let coretemp = sysfs::find_hwmon(sys, "coretemp");
    let asus = sysfs::find_hwmon(sys, "asus");
    let battery = sys.list(sysfs::POWER_SUPPLY_DIR).into_iter()
        .find(|n| sys.read(&format!("{}/{n}/type", sysfs::POWER_SUPPLY_DIR)).as_deref() == Some("Battery"));
    PerfState {
        profile: sys.read(sysfs::PLATFORM_PROFILE).as_deref().and_then(Profile::from_sysfs),
        choices: sys.read(sysfs::PROFILE_CHOICES).unwrap_or_default().split_whitespace().filter_map(Profile::from_sysfs).collect(),
        cpu_temp_c: coretemp.and_then(|h| num(format!("{h}/temp1_input"))).map(|m| m as f32 / 1000.0),
        cpu_fan_rpm: asus.as_ref().and_then(|h| num(format!("{h}/fan1_input"))).map(|v| v as u32),
        gpu_fan_rpm: asus.as_ref().and_then(|h| num(format!("{h}/fan2_input"))).map(|v| v as u32),
        power_draw_w: battery.and_then(|b| num(format!("{}/{b}/power_now", sysfs::POWER_SUPPLY_DIR))).map(|uw| uw as f32 / 1_000_000.0),
        cpu_boost: sys.read(sysfs::NO_TURBO).map(|v| v == "0"),
        undervolt: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::fake::{FakeGfx, FakeNvidia, FakeServices, FakeSysfs};
    use armoury_proto::{GpuMode, Profile};

    fn machine() -> FakeSysfs {
        FakeSysfs::with(&[
            ("sys/class/dmi/id/product_name", "ROG Strix G533ZW_G533ZW"),
            ("sys/devices/platform/asus-nb-wmi/keystone", "1"),
            ("sys/devices/platform/asus-nb-wmi/gpu_mux_mode", "1"),
            ("sys/devices/platform/asus-nb-wmi/dgpu_disable", "0"),
            ("sys/firmware/acpi/platform_profile", "performance"),
            ("sys/class/power_supply/ADP0/type", "Mains"),
            ("sys/class/power_supply/BAT0/type", "Battery"),
            ("sys/class/power_supply/BAT0/capacity", "80"),
            ("sys/class/power_supply/BAT0/status", "Not charging"),
            ("sys/class/power_supply/BAT0/charge_control_end_threshold", "80"),
            ("sys/firmware/acpi/platform_profile_choices", "quiet balanced performance"),
            ("sys/class/hwmon/hwmon8/name", "coretemp"),
            ("sys/class/hwmon/hwmon8/temp1_input", "72000"),
            ("sys/class/hwmon/hwmon9/name", "asus"),
            ("sys/class/hwmon/hwmon9/fan1_input", "3300"),
            ("sys/class/hwmon/hwmon9/fan2_input", "5200"),
            ("sys/class/power_supply/BAT0/power_now", "18250000"),
            ("sys/devices/system/cpu/intel_pstate/no_turbo", "0"),
        ])
    }

    #[tokio::test]
    async fn collect_reads_everything() {
        let svc = FakeServices::default();
        svc.running.lock().unwrap().insert("ghelper".into());
        let s = collect(&machine(), &FakeGfx::default(), &svc, &FakeNvidia::default(), ControlMode::Observe, true).await;
        assert_eq!(s.model.as_deref(), Some("ROG Strix G533ZW_G533ZW"));
        assert_eq!(s.keystone, Some(true));
        assert_eq!(s.platform_profile.as_deref(), Some("performance"));
        assert_eq!(s.gpu.mode, Some(GpuMode::Hybrid));
        assert_eq!(s.gpu.supported, vec![GpuMode::Integrated, GpuMode::Hybrid, GpuMode::Vfio, GpuMode::AsusMuxDgpu]);
        assert_eq!(s.gpu.pending, None);
        assert_eq!(s.gpu.power, Some(GpuPower::Suspended));
        assert_eq!((s.gpu.mux, s.gpu.dgpu_disable), (Some(1), Some(0)));
        assert_eq!(s.battery.capacity, Some(80));
        assert_eq!(s.battery.charge_limit, Some(80));
        assert_eq!(s.battery.status.as_deref(), Some("Not charging"));
        assert!(s.ghelper_running);
        assert!(!s.asusd_running);
    }

    #[tokio::test]
    async fn pending_mode_reported_when_set() {
        let gfx = FakeGfx { pending: 5, ..Default::default() };
        let s = collect(&machine(), &gfx, &FakeServices::default(), &FakeNvidia::default(), ControlMode::Active, true).await;
        assert_eq!(s.gpu.pending, Some(GpuMode::AsusMuxDgpu));
        assert_eq!(s.control, ControlMode::Active);
    }

    #[tokio::test(start_paused = true)]
    async fn collect_survives_wedged_gfx() {
        let gfx = FakeGfx { wedged: true, ..Default::default() };
        let started = tokio::time::Instant::now();
        let s = collect(&machine(), &gfx, &FakeServices::default(), &FakeNvidia::default(), ControlMode::Observe, true).await;
        assert_eq!(s.gpu.mode, None);
        assert!(s.gpu.supported.is_empty());
        assert_eq!(s.gpu.mux, Some(1), "sysfs still read");
        assert!(started.elapsed() <= crate::hw::CALL_TIMEOUT * 2);
    }

    #[tokio::test]
    async fn missing_nodes_are_none() {
        let s = collect(&FakeSysfs::default(), &FakeGfx::default(), &FakeServices::default(), &FakeNvidia::default(), ControlMode::Observe, true).await;
        assert_eq!(s.keystone, None);
        assert_eq!(s.battery, BatteryState::default());
    }

    #[tokio::test]
    async fn perf_readings() {
        let s = collect(&machine(), &FakeGfx::default(), &FakeServices::default(), &FakeNvidia::default(), ControlMode::Observe, true).await;
        assert_eq!(s.perf.profile, Some(Profile::Performance));
        assert_eq!(s.perf.choices, vec![Profile::Quiet, Profile::Balanced, Profile::Performance]);
        assert_eq!(s.perf.cpu_temp_c, Some(72.0));
        assert_eq!((s.perf.cpu_fan_rpm, s.perf.gpu_fan_rpm), (Some(3300), Some(5200)));
        assert_eq!(s.perf.power_draw_w, Some(18.25));
        assert_eq!(s.perf.cpu_boost, Some(true));
    }

    #[tokio::test]
    async fn no_nvml_while_suspended() {
        let nv = FakeNvidia::with_active(Some(false));
        let s = collect(&machine(), &FakeGfx::default(), &FakeServices::default(), &nv, ControlMode::Observe, true).await;
        assert_eq!(s.gpu.dgpu_active, Some(false));
        assert!(s.gpu.nvidia.is_none() && s.gpu.users.is_empty());
        assert_eq!(nv.status_reads.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn nvml_read_when_active() {
        let nv = FakeNvidia::with_active(Some(true));
        let s = collect(&machine(), &FakeGfx::default(), &FakeServices::default(), &nv, ControlMode::Observe, true).await;
        assert_eq!(s.gpu.nvidia.unwrap().core_mhz, 1500);
        assert_eq!(s.gpu.users[0].name, "game");
    }

    #[tokio::test]
    async fn gpu_pending_signals_read() {
        let sys = machine();
        {
            let mut f = sys.files.lock().unwrap();
            f.insert("sys/class/firmware-attributes/asus-armoury/attributes/pending_reboot".into(), "1".into());
            f.insert("etc/supergfxd.conf".into(), r#"{"mode": "Integrated", "vfio_enable": true}"#.into());
            f.insert("proc/777/cmdline".into(), "/bin/bash\0/usr/share/omarchy/bin/omarchy-toggle-hybrid-gpu\0".into());
            f.insert("proc/12/cmdline".into(), "bash\0".into());
        }
        let s = collect(&sys, &FakeGfx::default(), &FakeServices::default(), &FakeNvidia::default(), ControlMode::Observe, false).await;
        assert_eq!(s.gpu.pending_reboot, Some(true));
        assert_eq!(s.gpu.conf_mode, Some(GpuMode::Integrated));
        assert!(s.gpu.toggle_running);
        assert!(!s.gpu.pending_unknown);
    }

    #[tokio::test]
    async fn lighting_readings() {
        let sys = machine();
        {
            let mut f = sys.files.lock().unwrap();
            f.insert("sys/class/leds/asus::kbd_backlight/brightness".into(), "2".into());
            f.insert("sys/class/power_supply/ADP0/online".into(), "1".into());
        }
        let s = collect(&sys, &FakeGfx::default(), &FakeServices::default(), &FakeNvidia::default(), ControlMode::Observe, false).await;
        assert_eq!(s.lighting.brightness, Some(2));
        assert_eq!(s.lighting.on_ac, Some(true));
        sys.files.lock().unwrap().insert("sys/class/power_supply/ADP0/online".into(), "0".into());
        let s = collect(&sys, &FakeGfx::default(), &FakeServices::default(), &FakeNvidia::default(), ControlMode::Observe, false).await;
        assert_eq!(s.lighting.on_ac, Some(false));
    }

    #[tokio::test]
    async fn battery_and_system_readings() {
        let sys = machine();
        {
            let mut f = sys.files.lock().unwrap();
            // this machine's BAT0 (energy_* in µWh)
            f.insert("sys/class/power_supply/BAT0/energy_full".into(), "64137000".into());
            f.insert("sys/class/power_supply/BAT0/energy_full_design".into(), "90005000".into());
            f.insert("sys/class/power_supply/BAT0/energy_now".into(), "32000000".into());
            f.insert("sys/class/power_supply/BAT0/cycle_count".into(), "0".into());
            f.insert("sys/class/power_supply/BAT0/voltage_now".into(), "15920000".into());
            f.insert("sys/class/power_supply/BAT0/status".into(), "Discharging".into());
            f.insert("sys/class/power_supply/BAT0/power_now".into(), "16000000".into());
            f.insert("sys/power/mem_sleep".into(), "[s2idle] deep".into());
            f.insert("sys/devices/platform/asus-nb-wmi/boot_sound".into(), "0".into());
            f.insert("sys/devices/platform/asus-nb-wmi/panel_od".into(), "1".into());
        }
        let s = collect(&sys, &FakeGfx::default(), &FakeServices::default(), &FakeNvidia::default(), ControlMode::Observe, false).await;
        let b = s.battery_info;
        assert_eq!(b.health_pct.map(|h| h.round()), Some(71.0));
        assert_eq!(b.design_wh.map(|w| w.round()), Some(90.0));
        assert_eq!(b.cycles, None, "firmware reports 0");
        assert_eq!(b.voltage_v.map(|v| (v * 10.0).round()), Some(159.0));
        assert_eq!(b.time_left_min, Some(120), "32 Wh at 16 W");
        assert_eq!(b.charge_limit, Some(80));
        assert_eq!(s.system.mem_sleep, Some(armoury_proto::SleepMode::S2idle));
        assert_eq!(s.system.sleep_modes, vec![armoury_proto::SleepMode::S2idle, armoury_proto::SleepMode::Deep]);
        assert_eq!((s.system.boot_sound, s.system.panel_od), (Some(false), Some(true)));
        assert!(!s.system.camera_present);
    }
}
