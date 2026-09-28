use super::Nvidia;
use armoury_proto::{GpuUser, NvStatus};
use std::path::PathBuf;

pub struct RealNvidia {
    root: PathBuf,
    dev: Option<PathBuf>,
}

impl RealNvidia {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let dev = std::fs::read_dir(root.join("sys/bus/pci/devices")).ok().and_then(|rd| {
            rd.flatten().map(|e| e.path()).find(|p| {
                let r = |f: &str| std::fs::read_to_string(p.join(f)).unwrap_or_default();
                r("vendor").trim() == "0x10de" && r("class").trim().starts_with("0x03")
            })
        });
        Self { root, dev }
    }
}

impl Nvidia for RealNvidia {
    fn dgpu_active(&self) -> Option<bool> {
        let s = std::fs::read_to_string(self.dev.as_ref()?.join("power/runtime_status")).ok()?;
        Some(s.trim() == "active")
    }

    fn status(&self) -> Option<NvStatus> {
        use nvml_wrapper::enum_wrappers::device::{Clock, PerformanceState, TemperatureSensor};
        // Initialised per read and dropped, so armouryd never holds the device open.
        let nvml = nvml_wrapper::Nvml::init().ok()?;
        let d = nvml.device_by_index(0).ok()?;
        let mem = d.memory_info().ok()?;
        Some(NvStatus {
            core_mhz: d.clock_info(Clock::Graphics).unwrap_or(0),
            mem_mhz: d.clock_info(Clock::Memory).unwrap_or(0),
            temp_c: d.temperature(TemperatureSensor::Gpu).unwrap_or(0),
            power_w: d.power_usage().map(|mw| mw as f32 / 1000.0).unwrap_or(0.0),
            util_pct: d.utilization_rates().map(|u| u.gpu).unwrap_or(0),
            vram_used_mb: mem.used / 1_048_576,
            vram_total_mb: mem.total / 1_048_576,
            pstate: d.performance_state().map(|p| format!("{p:?}")).unwrap_or_default(),
            core_offset: d.clock_offset(Clock::Graphics, PerformanceState::Zero).map(|o| o.clock_offset_mhz).unwrap_or(0),
            mem_offset: d.clock_offset(Clock::Memory, PerformanceState::Zero).map(|o| o.clock_offset_mhz).unwrap_or(0),
        })
    }

    fn users(&self) -> Vec<GpuUser> {
        let me = std::process::id();
        let mut users: Vec<GpuUser> = std::fs::read_dir(self.root.join("proc")).into_iter().flatten().flatten()
            .filter_map(|e| {
                let pid: u32 = e.file_name().to_str()?.parse().ok()?;
                if pid == me { return None; }
                let holds = std::fs::read_dir(e.path().join("fd")).ok()?.flatten()
                    .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|l| l.to_string_lossy().starts_with("/dev/nvidia")));
                holds.then(|| GpuUser { pid, name: std::fs::read_to_string(e.path().join("comm")).unwrap_or_default().trim().to_string() })
            })
            .collect();
        users.sort_by_key(|u| u.pid);
        users
    }
}

