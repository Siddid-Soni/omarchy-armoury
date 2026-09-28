use crate::hw::{Gfx, Services, Sysfs, sysfs, with_retry};
use armoury_proto::{BatteryState, ControlMode, GpuMode, GpuPower, GpuState, PerfState, Profile, Snapshot};

pub async fn collect(sys: &dyn Sysfs, gfx: &dyn Gfx, svc: &dyn Services, control: ControlMode) -> Snapshot {
    Snapshot {
        model: sys.read(sysfs::PRODUCT_NAME),
        control,
        keystone: sys.read(sysfs::KEYSTONE).map(|v| v == "1"),
        platform_profile: sys.read(sysfs::PLATFORM_PROFILE),
        gpu: gpu_state(sys, gfx).await,
        battery: battery_state(sys),
        perf: perf_state(sys),
        config_error: None,
        asusd_running: svc.unit_active("asusd.service", false).await,
        ghelper_running: svc.is_running("ghelper").await,
    }
}

async fn gpu_state(sys: &dyn Sysfs, gfx: &dyn Gfx) -> GpuState {
    let mut g = GpuState {
        mux: sys.read(sysfs::GPU_MUX).and_then(|v| v.parse().ok()),
        dgpu_disable: sys.read(sysfs::DGPU_DISABLE).and_then(|v| v.parse().ok()),
        ..Default::default()
    };
    // A wedged supergfxd costs one retry budget here, not four.
    let Ok(mode) = with_retry(|| gfx.mode()).await else { return g };
    g.mode = GpuMode::from_supergfx(mode);
    g.supported = with_retry(|| gfx.supported()).await.unwrap_or_default()
        .into_iter().filter_map(GpuMode::from_supergfx).collect();
    g.pending = with_retry(|| gfx.pending_mode()).await.ok()
        .and_then(GpuMode::from_supergfx).filter(|m| *m != GpuMode::None);
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
    use crate::hw::fake::{FakeGfx, FakeServices, FakeSysfs};
    use armoury_proto::Profile;

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
        let s = collect(&machine(), &FakeGfx::default(), &svc, ControlMode::Observe).await;
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
        let s = collect(&machine(), &gfx, &FakeServices::default(), ControlMode::Active).await;
        assert_eq!(s.gpu.pending, Some(GpuMode::AsusMuxDgpu));
        assert_eq!(s.control, ControlMode::Active);
    }

    #[tokio::test(start_paused = true)]
    async fn collect_survives_wedged_gfx() {
        let gfx = FakeGfx { wedged: true, ..Default::default() };
        let started = tokio::time::Instant::now();
        let s = collect(&machine(), &gfx, &FakeServices::default(), ControlMode::Observe).await;
        assert_eq!(s.gpu.mode, None);
        assert!(s.gpu.supported.is_empty());
        assert_eq!(s.gpu.mux, Some(1), "sysfs still read");
        assert!(started.elapsed() <= crate::hw::CALL_TIMEOUT * 2);
    }

    #[tokio::test]
    async fn missing_nodes_are_none() {
        let s = collect(&FakeSysfs::default(), &FakeGfx::default(), &FakeServices::default(), ControlMode::Observe).await;
        assert_eq!(s.keystone, None);
        assert_eq!(s.battery, BatteryState::default());
    }

    #[tokio::test]
    async fn perf_readings() {
        let s = collect(&machine(), &FakeGfx::default(), &FakeServices::default(), ControlMode::Observe).await;
        assert_eq!(s.perf.profile, Some(Profile::Performance));
        assert_eq!(s.perf.choices, vec![Profile::Quiet, Profile::Balanced, Profile::Performance]);
        assert_eq!(s.perf.cpu_temp_c, Some(72.0));
        assert_eq!((s.perf.cpu_fan_rpm, s.perf.gpu_fan_rpm), (Some(3300), Some(5200)));
        assert_eq!(s.perf.power_draw_w, Some(18.25));
        assert_eq!(s.perf.cpu_boost, Some(true));
    }
}
