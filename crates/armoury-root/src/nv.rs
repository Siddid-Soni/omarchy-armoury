use anyhow::{Context, bail};
use armoury_root::NvArgs;
use nvml_wrapper::enum_wrappers::device::{Clock, PerformanceState};
use nvml_wrapper::enums::device::GpuLockedClocksSetting;

/// Applies offsets (checked against the card's own P0 range) and clock locks;
/// returns the core and memory offsets read back from NVML.
pub fn apply(a: NvArgs) -> anyhow::Result<(i32, i32)> {
    let nvml = nvml_wrapper::Nvml::init().context("NVML init")?;
    let mut dev = nvml.device_by_index(0).context("NVIDIA device 0")?;
    for (clock, v, name) in [(Clock::Graphics, a.core_offset, "core"), (Clock::Memory, a.mem_offset, "memory")] {
        let r = dev.clock_offset(clock.clone(), PerformanceState::Zero).with_context(|| format!("{name} offset range"))?;
        if v < r.min_clock_offset_mhz || v > r.max_clock_offset_mhz {
            bail!("{name} offset {v} is outside the card's range {}–{}", r.min_clock_offset_mhz, r.max_clock_offset_mhz);
        }
        dev.set_clock_offset(clock, PerformanceState::Zero, v).with_context(|| format!("set {name} offset"))?;
    }
    match a.core_lock {
        Some(max) => dev.set_gpu_locked_clocks(GpuLockedClocksSetting::Numeric { min_clock_mhz: 0, max_clock_mhz: max }).context("lock core clock")?,
        None => dev.reset_gpu_locked_clocks().context("unlock core clock")?,
    }
    match a.mem_lock {
        Some(max) => dev.set_mem_locked_clocks(0, max).context("lock memory clock")?,
        None => dev.reset_mem_locked_clocks().context("unlock memory clock")?,
    }
    let core = dev.clock_offset(Clock::Graphics, PerformanceState::Zero)?.clock_offset_mhz;
    let mem = dev.clock_offset(Clock::Memory, PerformanceState::Zero)?.clock_offset_mhz;
    Ok((core, mem))
}
