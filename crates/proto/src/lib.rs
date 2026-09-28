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
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuState {
    pub mode: Option<GpuMode>,
    pub supported: Vec<GpuMode>,
    pub pending: Option<GpuMode>,
    pub power: Option<GpuPower>,
    pub mux: Option<u8>,
    pub dgpu_disable: Option<u8>,
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
}
