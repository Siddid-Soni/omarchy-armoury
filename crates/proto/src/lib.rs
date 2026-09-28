use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const SOCKET_NAME: &str = "armoury.sock";

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| "/tmp".into());
    dir.join(SOCKET_NAME)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
    #[default]
    Observe,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuMode { Hybrid, Integrated, NvidiaNoModeset, Vfio, AsusEgpu, AsusMuxDgpu, None }

impl GpuMode {
    pub fn from_supergfx(v: u32) -> Option<Self> {
        Some(match v {
            0 => Self::Hybrid,
            1 => Self::Integrated,
            2 => Self::NvidiaNoModeset,
            3 => Self::Vfio,
            4 => Self::AsusEgpu,
            5 => Self::AsusMuxDgpu,
            6 => Self::None,
            _ => return Option::None,
        })
    }

    /// supergfxd mode code (inverse of from_supergfx).
    pub fn code(self) -> u32 {
        match self {
            Self::Hybrid => 0,
            Self::Integrated => 1,
            Self::NvidiaNoModeset => 2,
            Self::Vfio => 3,
            Self::AsusEgpu => 4,
            Self::AsusMuxDgpu => 5,
            Self::None => 6,
        }
    }
}

/// One GPU switch action. Integrated↔Ultimate is two manual steps via Hybrid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GpuStep {
    /// Omarchy's own omarchy-toggle-hybrid-gpu (Hybrid ↔ Integrated; it reboots).
    OmarchyToggle { to: GpuMode },
    /// supergfxd SetMode (Hybrid ↔ Ultimate; reboot to finish).
    Supergfx { to: GpuMode },
    FirstOfTwo { first: Box<GpuStep>, then: GpuMode },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuSwitchResult {
    pub step: GpuStep,
    pub reboot_required: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuPower { Active, Suspended, Off, AsusDisabled, AsusMuxDiscreet, Unknown }

impl GpuPower {
    pub fn from_supergfx(v: u32) -> Option<Self> {
        Some(match v {
            0 => Self::Active,
            1 => Self::Suspended,
            2 => Self::Off,
            3 => Self::AsusDisabled,
            4 => Self::AsusMuxDiscreet,
            5 => Self::Unknown,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile { Quiet, Balanced, Performance }

impl Profile {
    pub const ALL: [Profile; 3] = [Profile::Quiet, Profile::Balanced, Profile::Performance];
    pub fn from_asusd(v: u32) -> Option<Self> {
        match v { 0 => Some(Self::Balanced), 1 => Some(Self::Performance), 2 => Some(Self::Quiet), _ => None }
    }
    pub fn to_asusd(self) -> u32 {
        match self { Self::Balanced => 0, Self::Performance => 1, Self::Quiet => 2 }
    }
    pub fn from_sysfs(s: &str) -> Option<Self> {
        match s { "quiet" => Some(Self::Quiet), "balanced" => Some(Self::Balanced), "performance" => Some(Self::Performance), _ => None }
    }
    pub fn sysfs(self) -> &'static str {
        match self { Self::Quiet => "quiet", Self::Balanced => "balanced", Self::Performance => "performance" }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Epp { Default, Performance, BalancePerformance, BalancePower, Power }

impl Epp {
    pub fn to_asusd(self) -> u32 { self as u32 }
    pub fn from_asusd(v: u32) -> Option<Self> {
        [Self::Default, Self::Performance, Self::BalancePerformance, Self::BalancePower, Self::Power].get(v as usize).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fan { Cpu, Gpu, Mid }

impl Fan {
    pub fn asusd_name(self) -> &'static str {
        match self { Self::Cpu => "CPU", Self::Gpu => "GPU", Self::Mid => "MID" }
    }
    pub fn from_asusd(s: &str) -> Option<Self> {
        match s { "CPU" => Some(Self::Cpu), "GPU" => Some(Self::Gpu), "MID" => Some(Self::Mid), _ => None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FanCurve {
    pub fan: Fan,
    pub temps: [u8; 8],
    pub percent: [u8; 8],
    pub enabled: bool,
}

/// Per-mode settings; `None` leaves that setting untouched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pl1: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pl2: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nv_boost: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nv_temp: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epp: Option<Epp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_boost: Option<bool>,
    /// Intel core+cache voltage offset, mV (negative = undervolt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uv_mv: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_core_offset: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_mem_offset: Option<i32>,
    /// Max GPU core clock, MHz; 0 = no lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_core_lock: Option<u32>,
    /// Max VRAM clock, MHz; 0 = no lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_mem_lock: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NvStatus {
    pub core_mhz: u32,
    pub mem_mhz: u32,
    pub temp_c: u32,
    pub power_w: f32,
    pub util_pct: u32,
    pub vram_used_mb: u64,
    pub vram_total_mb: u64,
    pub pstate: String,
    pub core_offset: i32,
    pub mem_offset: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuUser {
    pub pid: u32,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndervoltState {
    pub unlocked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PerfState {
    pub profile: Option<Profile>,
    pub choices: Vec<Profile>,
    pub cpu_temp_c: Option<f32>,
    pub cpu_fan_rpm: Option<u32>,
    pub gpu_fan_rpm: Option<u32>,
    pub power_draw_w: Option<f32>,
    pub cpu_boost: Option<bool>,
    /// None until the undervolt probe has run (active mode only).
    pub undervolt: Option<UndervoltState>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuState {
    pub mode: Option<GpuMode>,
    pub supported: Vec<GpuMode>,
    pub pending: Option<GpuMode>,
    pub power: Option<GpuPower>,
    pub mux: Option<u8>,
    pub dgpu_disable: Option<u8>,
    /// dGPU PCI runtime PM state is "active" (None = no NVIDIA dGPU found).
    pub dgpu_active: Option<bool>,
    /// Read only while the dGPU is already awake.
    pub nvidia: Option<NvStatus>,
    /// Processes holding /dev/nvidia* (keep the dGPU awake).
    pub users: Vec<GpuUser>,
    /// Firmware reports a GPU change waiting for reboot (asus-armoury pending_reboot).
    pub pending_reboot: Option<bool>,
    /// "mode" in /etc/supergfxd.conf; differs from `mode` after an Omarchy toggle rewrite.
    pub conf_mode: Option<GpuMode>,
    /// omarchy-toggle-hybrid-gpu is running (waiting for confirmation or rebooting).
    pub toggle_running: bool,
    /// supergfxd PendingMode could not be read.
    pub pending_unknown: bool,
    /// Switch armouryd made this boot (survives a supergfxd restart).
    pub armoury_pending: Option<GpuMode>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BatteryState {
    pub capacity: Option<u8>,
    pub status: Option<String>,
    pub charge_limit: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub model: Option<String>,
    pub control: ControlMode,
    pub keystone: Option<bool>,
    pub platform_profile: Option<String>,
    pub gpu: GpuState,
    pub battery: BatteryState,
    pub perf: PerfState,
    /// Why config.toml (or part of it) was ignored, if it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_error: Option<String>,
    /// Last failure applying a mode's settings (cleared by the next success).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apply_error: Option<String>,
    pub asusd_running: bool,
    pub ghelper_running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Status,
    Subscribe,
    Takeover,
    Handback,
    SetProfile { profile: Profile },
    NextProfile,
    FanCurves { profile: Profile },
    SetFanCurve { profile: Profile, curve: FanCurve },
    ResetFanCurves { profile: Profile },
    ModeSettings { profile: Profile },
    SetModeSettings { profile: Profile, settings: ModeSettings },
    ProbeUndervolt,
    SetGpuMode { mode: GpuMode },
    PlanGpuMode { mode: GpuMode },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(data: serde_json::Value) -> Self { Self { ok: true, data: Some(data), error: None } }
    pub fn err(msg: impl Into<String>) -> Self { Self { ok: false, data: None, error: Some(msg.into()) } }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum Event { Snapshot(Snapshot) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_wire_format() {
        let r: Request = serde_json::from_str(r#"{"cmd":"status"}"#).unwrap();
        assert_eq!(r, Request::Status);
        assert_eq!(serde_json::to_string(&Request::Takeover).unwrap(), r#"{"cmd":"takeover"}"#);
    }

    #[test]
    fn supergfx_codes_map() {
        assert_eq!(GpuMode::from_supergfx(0), Some(GpuMode::Hybrid));
        assert_eq!(GpuMode::from_supergfx(1), Some(GpuMode::Integrated));
        assert_eq!(GpuMode::from_supergfx(5), Some(GpuMode::AsusMuxDgpu));
        assert_eq!(GpuMode::from_supergfx(6), Some(GpuMode::None));
        assert_eq!(GpuMode::from_supergfx(99), None);
        assert_eq!(GpuPower::from_supergfx(1), Some(GpuPower::Suspended));
        assert_eq!(GpuPower::from_supergfx(9), None);
    }

    #[test]
    fn event_wire_format() {
        let e = Event::Snapshot(Snapshot::default());
        let v: serde_json::Value = serde_json::to_value(&e).unwrap();
        assert_eq!(v["event"], "snapshot");
        assert_eq!(v["data"]["control"], "observe");
    }

    #[test]
    fn response_helpers() {
        let v = serde_json::to_value(Response::err("boom")).unwrap();
        assert_eq!(v, serde_json::json!({"ok": false, "error": "boom"}));
        let v = serde_json::to_value(Response::ok(serde_json::json!(1))).unwrap();
        assert_eq!(v, serde_json::json!({"ok": true, "data": 1}));
    }

    #[test]
    fn profile_codes() {
        assert_eq!(Profile::from_asusd(2), Some(Profile::Quiet));
        assert_eq!(Profile::from_asusd(3), None);
        assert_eq!(Profile::Performance.to_asusd(), 1);
        assert_eq!(Profile::from_sysfs("balanced"), Some(Profile::Balanced));
        assert_eq!(Profile::Quiet.sysfs(), "quiet");
        assert_eq!(Epp::BalancePower.to_asusd(), 3);
        assert_eq!(Fan::from_asusd("GPU"), Some(Fan::Gpu));
        assert_eq!(Fan::Cpu.asusd_name(), "CPU");
    }

    #[test]
    fn new_requests_wire_format() {
        let r: Request = serde_json::from_str(r#"{"cmd":"set_profile","profile":"quiet"}"#).unwrap();
        assert_eq!(r, Request::SetProfile { profile: Profile::Quiet });
        let r: Request = serde_json::from_str(
            r#"{"cmd":"set_mode_settings","profile":"performance","settings":{"pl1":120,"epp":"balance_power"}}"#,
        ).unwrap();
        assert_eq!(r, Request::SetModeSettings {
            profile: Profile::Performance,
            settings: ModeSettings { pl1: Some(120), epp: Some(Epp::BalancePower), ..Default::default() },
        });
        let v = serde_json::to_value(ModeSettings { pl2: Some(150), ..Default::default() }).unwrap();
        assert_eq!(v, serde_json::json!({"pl2": 150}));
    }

    #[test]
    fn tuning_fields_wire_format() {
        let r: Request = serde_json::from_str(r#"{"cmd":"set_mode_settings","profile":"performance","settings":{"uv_mv":-40,"gpu_core_offset":100,"gpu_core_lock":0}}"#).unwrap();
        assert_eq!(r, Request::SetModeSettings { profile: Profile::Performance, settings: ModeSettings {
            uv_mv: Some(-40), gpu_core_offset: Some(100), gpu_core_lock: Some(0), ..Default::default() } });
        assert_eq!(serde_json::to_string(&Request::ProbeUndervolt).unwrap(), r#"{"cmd":"probe_undervolt"}"#);
    }

    #[test]
    fn gpu_switch_types() {
        assert_eq!(GpuMode::AsusMuxDgpu.code(), 5);
        for c in 0..=6 { assert_eq!(GpuMode::from_supergfx(c).unwrap().code(), c); }
        let v = serde_json::to_value(GpuStep::FirstOfTwo { first: Box::new(GpuStep::OmarchyToggle { to: GpuMode::Hybrid }), then: GpuMode::AsusMuxDgpu }).unwrap();
        assert_eq!(v["kind"], "first_of_two");
        assert_eq!(v["first"]["kind"], "omarchy_toggle");
        let r: Request = serde_json::from_str(r#"{"cmd":"set_gpu_mode","mode":"AsusMuxDgpu"}"#).unwrap();
        assert_eq!(r, Request::SetGpuMode { mode: GpuMode::AsusMuxDgpu });
    }
}
