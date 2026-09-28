use super::Nvidia;
use armoury_proto::{GpuUser, NvStatus};
use std::path::PathBuf;

/// Runs a blocking call on its own thread; None if it does not finish in time
/// (a wedged NVIDIA driver must not hang armouryd). A stuck thread is abandoned.
pub fn with_thread_timeout<T: Send + 'static>(d: std::time::Duration, f: impl FnOnce() -> Option<T> + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let _ = tx.send(f()); });
    rx.recv_timeout(d).ok().flatten()
}

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
        with_thread_timeout(crate::hw::CALL_TIMEOUT, read_nvml)
    }

    fn users(&self) -> Vec<GpuUser> { self.scan_users() }
}

fn read_nvml() -> Option<NvStatus> {
    {
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
}

impl RealNvidia {
    fn scan_users(&self) -> Vec<GpuUser> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_dgpu_and_runtime_status() {
        let d = tempfile::tempdir().unwrap();
        let igpu = d.path().join("sys/bus/pci/devices/0000:00:02.0");
        let dgpu = d.path().join("sys/bus/pci/devices/0000:01:00.0");
        for (p, v, c) in [(&igpu, "0x8086", "0x030000"), (&dgpu, "0x10de", "0x030000")] {
            std::fs::create_dir_all(p.join("power")).unwrap();
            std::fs::write(p.join("vendor"), v).unwrap();
            std::fs::write(p.join("class"), c).unwrap();
        }
        std::fs::write(dgpu.join("power/runtime_status"), "suspended\n").unwrap();
        let nv = RealNvidia::new(d.path());
        assert_eq!(nv.dgpu_active(), Some(false));
        std::fs::write(dgpu.join("power/runtime_status"), "active\n").unwrap();
        assert_eq!(nv.dgpu_active(), Some(true));
        assert_eq!(RealNvidia::new(d.path().join("nothing")).dgpu_active(), None);
    }

    #[test]
    fn users_found_from_proc_fds() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("proc/4242");
        std::fs::create_dir_all(p.join("fd")).unwrap();
        std::fs::write(p.join("comm"), "game\n").unwrap();
        std::os::unix::fs::symlink("/dev/nvidia0", p.join("fd/7")).unwrap();
        let q = d.path().join("proc/99");
        std::fs::create_dir_all(q.join("fd")).unwrap();
        std::os::unix::fs::symlink("/dev/null", q.join("fd/1")).unwrap();
        let users = RealNvidia::new(d.path()).users();
        assert_eq!(users, vec![GpuUser { pid: 4242, name: "game".into() }]);
    }

    #[test]
    fn blocking_call_times_out() {
        let started = std::time::Instant::now();
        let r: Option<u32> = with_thread_timeout(std::time::Duration::from_millis(100), || { std::thread::sleep(std::time::Duration::from_secs(5)); Some(1) });
        assert_eq!(r, None);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(with_thread_timeout(std::time::Duration::from_secs(1), || Some(7)), Some(7));
    }
}
