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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuraMode { Static, Breathe, RainbowCycle, RainbowWave, Star, Rain, Highlight, Laser, Ripple, Pulse, Comet, Flash }

impl AuraMode {
    const CODES: [(AuraMode, u32); 12] = [
        (Self::Static, 0), (Self::Breathe, 1), (Self::RainbowCycle, 2), (Self::RainbowWave, 3), (Self::Star, 4), (Self::Rain, 5),
        (Self::Highlight, 6), (Self::Laser, 7), (Self::Ripple, 8), (Self::Pulse, 10), (Self::Comet, 11), (Self::Flash, 12),
    ];
    /// asusd AuraModeNum.
    pub fn code(self) -> u32 { Self::CODES.iter().find(|(m, _)| *m == self).unwrap().1 }
    pub fn from_code(c: u32) -> Option<Self> { Self::CODES.iter().find(|(_, v)| *v == c).map(|(m, _)| *m) }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speed { Low, #[default] Med, High }

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction { #[default] Right, Left, Up, Down }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuraZone { Logo, Keyboard, Lightbar, Lid, RearGlow }

impl AuraZone {
    /// asusd PowerZones.
    pub fn code(self) -> u32 {
        match self { Self::Logo => 0, Self::Keyboard => 1, Self::Lightbar => 2, Self::Lid => 3, Self::RearGlow => 4 }
    }
    pub fn from_code(c: u32) -> Option<Self> {
        [Self::Logo, Self::Keyboard, Self::Lightbar, Self::Lid, Self::RearGlow].get(c as usize).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuraEffect {
    pub mode: AuraMode,
    pub colour1: [u8; 3],
    pub colour2: [u8; 3],
    pub speed: Speed,
    pub direction: Direction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZonePower {
    pub zone: AuraZone,
    pub boot: bool,
    pub awake: bool,
    pub sleep: bool,
    pub shutdown: bool,
}

/// Read from asusd on request (active mode).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightingInfo {
    pub effect: Option<AuraEffect>,
    pub zones: Vec<ZonePower>,
    pub modes: Vec<AuraMode>,
    pub power_zones: Vec<AuraZone>,
}

/// Observe-safe lighting readings (sysfs).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightingState {
    pub brightness: Option<u8>,
    pub on_ac: Option<bool>,
    /// The keyboard's current effect (asusd; active mode only).
    #[serde(default)]
    pub effect: Option<AuraEffect>,
    #[serde(default)]
    pub music: MusicState,
    /// Why music lighting stopped by itself (capture or keyboard failures).
    #[serde(default)]
    pub music_error: Option<String>,
}

/// Music-reactive lighting (armouryd's, active mode only).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicState {
    /// Observe mode, or no per-key keyboard.
    #[default]
    Unavailable,
    Off,
    On,
    /// Turned itself off after repeated failures (see `music_error`).
    Failed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicStyle {
    /// Bass → treble left → right, bars rise with each band's level.
    #[default]
    Spectrum,
    /// Everything follows the loudness.
    Pulse,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicScheme {
    /// Colour 1 → colour 2 (by bar height, or by loudness in Pulse).
    #[default]
    Gradient,
    /// Hue by key column.
    Rainbow,
    /// Colour 1 only.
    Single,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BatteryInfo {
    pub capacity: Option<u8>,
    pub status: Option<String>,
    pub health_pct: Option<f32>,
    pub full_wh: Option<f32>,
    pub design_wh: Option<f32>,
    /// None when the firmware reports 0 (it does not count cycles).
    pub cycles: Option<u32>,
    pub voltage_v: Option<f32>,
    pub draw_w: Option<f32>,
    pub time_left_min: Option<u32>,
    pub charge_limit: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub output: String,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f32,
    pub rates: Vec<f32>,
    pub scale: f32,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default)]
    pub transform: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SleepMode { S2idle, Deep }

impl SleepMode {
    pub fn kernel(self) -> &'static str { match self { Self::S2idle => "s2idle", Self::Deep => "deep" } }
    pub fn from_kernel(s: &str) -> Option<Self> { match s { "s2idle" => Some(Self::S2idle), "deep" => Some(Self::Deep), _ => None } }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Toggle { Touchpad, BootSound, PanelOd, Clamshell }

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemState {
    #[serde(default)]
    pub numpad: NumpadState,
    pub boot_sound: Option<bool>,
    pub panel_od: Option<bool>,
    pub touchpad: Option<bool>,
    pub clamshell: bool,
    /// Active kernel sleep mode (the bracketed one in /sys/power/mem_sleep).
    pub mem_sleep: Option<SleepMode>,
    pub sleep_modes: Vec<SleepMode>,
    pub camera_present: bool,
}

/// Illuminated NumberPad on the touchpad (armouryd's, active mode only).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumpadState {
    /// No supported touchpad, observe mode, or the worker gave up.
    #[default]
    Unavailable,
    Off,
    On,
}

/// Lighting change when the Keystone goes in or out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeystoneLight {
    #[default]
    Unchanged,
    Music,
    /// Back to what was on before the last insert changed it (remove only).
    Previous,
    /// This effect, with the current colours and speed.
    #[serde(untagged)]
    Effect(AuraMode),
}

/// What armouryd does when the Keystone is inserted or removed (active mode).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeystoneAction {
    pub mode: Option<ModeChoice>,
    pub light: KeystoneLight,
    /// Shell command, run detached.
    pub command: Option<String>,
    /// Lock the screen (remove only).
    pub lock: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeystoneEvent { Insert, Remove }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotKey {
    /// ROG / Armoury key (KEY_PROG1)
    Rog,
    /// Fn+F5 fan key (KEY_PROG4)
    Fan,
    /// Fn+F4 Aura key (KEY_PROG3)
    Aura,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    #[default]
    None,
    OpenWindow,
    CycleMode,
    CycleBrightness,
    CycleEffect,
    /// Turns the NumberPad on or off.
    ToggleNumpad,
    /// Turns music lighting on or off.
    ToggleMusic,
    /// Runs the configured shell command.
    Command,
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

/// A mode the user picks: the three firmware modes or Manual (a saved profile on a base mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeChoice { Quiet, Balanced, Performance, Manual }

impl ModeChoice {
    /// Order of the mode key (Fn+F5) cycle.
    pub const CYCLE: [ModeChoice; 4] = [Self::Quiet, Self::Balanced, Self::Performance, Self::Manual];
    pub fn stock(self) -> Option<Profile> {
        match self { Self::Quiet => Some(Profile::Quiet), Self::Balanced => Some(Profile::Balanced), Self::Performance => Some(Profile::Performance), Self::Manual => None }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Quiet => "Silent", Self::Balanced => "Balanced", Self::Performance => "Turbo", Self::Manual => "Manual" }
    }
}

impl From<Profile> for ModeChoice {
    fn from(p: Profile) -> Self {
        match p { Profile::Quiet => Self::Quiet, Profile::Balanced => Self::Balanced, Profile::Performance => Self::Performance }
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
    #[serde(default)]
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

fn default_base() -> Profile { Profile::Performance }

/// A saved Manual-mode profile: which firmware mode it runs on, its fan curves and its tuning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualProfile {
    pub name: String,
    #[serde(default = "default_base")]
    pub base: Profile,
    #[serde(default)]
    pub curves: Vec<FanCurve>,
    #[serde(default)]
    pub settings: ModeSettings,
}

/// What the UI sees of Manual mode.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ManualView {
    pub enabled: bool,
    pub active: Option<String>,
    pub profiles: Vec<ManualProfile>,
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
    /// The mode the user is in (Manual only while armouryd is active).
    #[serde(default)]
    pub mode: Option<ModeChoice>,
    /// Active manual profile name (even while a stock mode is on).
    #[serde(default)]
    pub manual_profile: Option<String>,
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
    /// armouryd's version (the plugin offers a daemon update when it differs from its own).
    #[serde(default)]
    pub version: String,
    pub model: Option<String>,
    pub control: ControlMode,
    pub keystone: Option<bool>,
    pub platform_profile: Option<String>,
    pub gpu: GpuState,
    pub battery: BatteryState,
    pub perf: PerfState,
    pub lighting: LightingState,
    #[serde(default)]
    pub battery_info: BatteryInfo,
    /// Hyprland outputs (empty outside a Hyprland session).
    #[serde(default)]
    pub display: Vec<DisplayInfo>,
    #[serde(default)]
    pub system: SystemState,
    /// Why config.toml (or part of it) was ignored, if it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_error: Option<String>,
    /// Last failure applying a mode's settings (cleared by the next success).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apply_error: Option<String>,
    pub asusd_running: bool,
    pub ghelper_running: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Status,
    Subscribe,
    Takeover,
    Handback,
    /// Silent / Balanced / Turbo, or Manual (the active manual profile).
    SetProfile { profile: ModeChoice },
    /// Next mode in ModeChoice::CYCLE.
    NextProfile,
    /// Firmware bounds for the power-limit settings: {key: [min, max]}.
    LimitBounds,
    /// Resets asusd's curves for `base` to the firmware defaults and returns them (for the editor).
    DefaultCurves { base: Profile },
    ManualProfiles,
    /// Creates or overwrites a profile; `original_name` set to another name renames it.
    SaveManualProfile { profile: ManualProfile, #[serde(default)] original_name: Option<String> },
    /// Makes a profile active and switches to Manual.
    ActivateManualProfile { name: String },
    DeleteManualProfile { name: String },
    ProbeUndervolt,
    SetGpuMode { mode: GpuMode },
    PlanGpuMode { mode: GpuMode },
    Lighting,
    SetBrightness { level: u8 },
    SetEffect { effect: AuraEffect },
    SetZonePower { zone: ZonePower },
    KbdIdle,
    KbdResume,
    SetChargeLimit { percent: u8 },
    OneShotCharge,
    SetRefresh { hz: f32 },
    SetGamma { percent: u8 },
    SetToggle { toggle: Toggle, on: bool },
    SetSleepMode { mode: SleepMode },
    SetSourceProfile { ac: Option<ModeChoice>, battery: Option<ModeChoice> },
    SetSourceRefresh { ac: Option<f32>, battery: Option<f32> },
    Keys,
    /// armouryd's saved settings (read-only view for the UI).
    Config,
    /// Never dim the keyboard backlight when idle.
    SetKeepOn { on: bool },
    SetKeyBinding { key: HotKey, action: KeyAction, #[serde(default)] command: Option<String> },
    /// NumberPad on/off (active mode; refused while the touchpad is off unless allowed).
    SetNumpad { on: bool },
    SetNumpadConfig {
        #[serde(default)] start_brightness: Option<u8>,
        #[serde(default)] allow_when_touchpad_off: Option<bool>,
        #[serde(default)] idle_dim_secs: Option<u32>,
        #[serde(default)] hold_ms: Option<u32>,
        #[serde(default)] key_repeat: Option<bool>,
        #[serde(default)] repeat_delay_ms: Option<u32>,
        #[serde(default)] repeat_rate_hz: Option<u32>,
    },
    /// Music lighting on/off (active mode; remembered).
    SetMusic { on: bool },
    /// Replaces the actions for one Keystone event.
    SetKeystoneAction { event: KeystoneEvent, action: KeystoneAction },
    /// Flash the Keystone LED when it is inserted.
    SetKeystoneFlash { on: bool },
    /// Master switch: off = nothing happens on insert/remove (no OSD, actions or flash).
    SetKeystoneEnabled { on: bool },
    SetMusicConfig {
        #[serde(default)] style: Option<MusicStyle>,
        #[serde(default)] scheme: Option<MusicScheme>,
        #[serde(default)] colour1: Option<[u8; 3]>,
        #[serde(default)] colour2: Option<[u8; 3]>,
        #[serde(default)] sensitivity: Option<u8>,
    },
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
pub enum Event {
    Snapshot(Snapshot),
    /// Music lighting's band levels (0..1, bass → treble), ~10 per second while it runs;
    /// an empty list once when it stops.
    MusicBands(Vec<f32>),
}

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
        assert_eq!(r, Request::SetProfile { profile: ModeChoice::Quiet });
        let r: Request = serde_json::from_str(
            r#"{"cmd":"save_manual_profile","profile":{"name":"G","settings":{"pl1":120,"epp":"balance_power"}}}"#,
        ).unwrap();
        let Request::SaveManualProfile { profile, original_name: None } = r else { panic!("{r:?}") };
        assert_eq!(profile.settings, ModeSettings { pl1: Some(120), epp: Some(Epp::BalancePower), ..Default::default() });
        let v = serde_json::to_value(ModeSettings { pl2: Some(150), ..Default::default() }).unwrap();
        assert_eq!(v, serde_json::json!({"pl2": 150}));
    }

    #[test]
    fn tuning_fields_wire_format() {
        let p: ManualProfile = serde_json::from_str(r#"{"name":"G","settings":{"uv_mv":-40,"gpu_core_offset":100,"gpu_core_lock":0}}"#).unwrap();
        assert_eq!(p.settings, ModeSettings { uv_mv: Some(-40), gpu_core_offset: Some(100), gpu_core_lock: Some(0), ..Default::default() });
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

    #[test]
    fn lighting_types() {
        for c in [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12] { assert_eq!(AuraMode::from_code(c).unwrap().code(), c); }
        assert_eq!(AuraMode::from_code(9), None);
        assert_eq!(AuraZone::Lightbar.code(), 2);
        assert_eq!(AuraZone::from_code(0), Some(AuraZone::Logo));
        let r: Request = serde_json::from_str(r#"{"cmd":"set_effect","effect":{"mode":"rainbow_wave","colour1":[255,0,0],"colour2":[0,0,0],"speed":"high","direction":"left"}}"#).unwrap();
        assert_eq!(r, Request::SetEffect { effect: AuraEffect { mode: AuraMode::RainbowWave, colour1: [255, 0, 0], colour2: [0, 0, 0], speed: Speed::High, direction: Direction::Left } });
        let z: Request = serde_json::from_str(r#"{"cmd":"set_zone_power","zone":{"zone":"logo","boot":true,"awake":false,"sleep":false,"shutdown":false}}"#).unwrap();
        assert!(matches!(z, Request::SetZonePower { .. }));
        assert_eq!(serde_json::to_string(&Request::KbdIdle).unwrap(), r#"{"cmd":"kbd_idle"}"#);
    }

    #[test]
    fn system_types() {
        let r: Request = serde_json::from_str(r#"{"cmd":"set_charge_limit","percent":80}"#).unwrap();
        assert_eq!(r, Request::SetChargeLimit { percent: 80 });
        let r: Request = serde_json::from_str(r#"{"cmd":"set_refresh","hz":60.0}"#).unwrap();
        assert_eq!(r, Request::SetRefresh { hz: 60.0 });
        let r: Request = serde_json::from_str(r#"{"cmd":"set_sleep_mode","mode":"deep"}"#).unwrap();
        assert_eq!(r, Request::SetSleepMode { mode: SleepMode::Deep });
        let r: Request = serde_json::from_str(r#"{"cmd":"set_source_profile","ac":"performance","battery":"quiet"}"#).unwrap();
        assert_eq!(r, Request::SetSourceProfile { ac: Some(ModeChoice::Performance), battery: Some(ModeChoice::Quiet) });
        let r: Request = serde_json::from_str(r#"{"cmd":"set_toggle","toggle":"touchpad","on":false}"#).unwrap();
        assert_eq!(r, Request::SetToggle { toggle: Toggle::Touchpad, on: false });
        let s = Snapshot::default();
        assert!(s.system.mem_sleep.is_none() && s.display.is_empty() && s.battery_info.health_pct.is_none());
    }

    #[test]
    fn key_types() {
        let r: Request = serde_json::from_str(r#"{"cmd":"set_key_binding","key":"fan","action":"cycle_mode"}"#).unwrap();
        assert_eq!(r, Request::SetKeyBinding { key: HotKey::Fan, action: KeyAction::CycleMode, command: None });
        let r: Request = serde_json::from_str(r#"{"cmd":"set_key_binding","key":"rog","action":"command","command":"kitty"}"#).unwrap();
        assert!(matches!(r, Request::SetKeyBinding { key: HotKey::Rog, action: KeyAction::Command, command: Some(_) }));
        assert_eq!(serde_json::to_string(&Request::Keys).unwrap(), r#"{"cmd":"keys"}"#);
    }

    #[test]
    fn mode_choice_wire_names_and_cycle() {
        assert_eq!(serde_json::to_string(&ModeChoice::Manual).unwrap(), "\"manual\"");
        assert_eq!(serde_json::from_str::<ModeChoice>("\"performance\"").unwrap(), ModeChoice::Performance);
        assert_eq!(ModeChoice::CYCLE, [ModeChoice::Quiet, ModeChoice::Balanced, ModeChoice::Performance, ModeChoice::Manual]);
        assert_eq!(ModeChoice::Manual.stock(), None);
        assert_eq!(ModeChoice::Quiet.stock(), Some(Profile::Quiet));
        assert_eq!(ModeChoice::from(Profile::Performance).label(), "Turbo");
    }

    #[test]
    fn manual_profile_defaults() {
        let p: ManualProfile = serde_json::from_str(r#"{"name":"A"}"#).unwrap();
        assert_eq!((p.base, p.curves.len(), p.settings), (Profile::Performance, 0, ModeSettings::default()));
        // curves in a profile need no `enabled` flag
        let c: FanCurve = serde_json::from_str(r#"{"fan":"cpu","temps":[1,2,3,4,5,6,7,8],"percent":[0,0,0,0,0,0,0,0]}"#).unwrap();
        assert!(!c.enabled);
    }

    #[test]
    fn manual_requests_parse() {
        let r: Request = serde_json::from_str(r#"{"cmd":"set_profile","profile":"manual"}"#).unwrap();
        assert_eq!(r, Request::SetProfile { profile: ModeChoice::Manual });
        let r: Request = serde_json::from_str(r#"{"cmd":"save_manual_profile","profile":{"name":"G","base":"quiet"},"original_name":"F"}"#).unwrap();
        assert!(matches!(r, Request::SaveManualProfile { ref original_name, .. } if original_name.as_deref() == Some("F")));
        let r: Request = serde_json::from_str(r#"{"cmd":"activate_manual_profile","name":"G"}"#).unwrap();
        assert_eq!(r, Request::ActivateManualProfile { name: "G".into() });
    }

    #[test]
    fn numpad_wire_format() {
        assert_eq!(serde_json::to_string(&NumpadState::On).unwrap(), "\"on\"");
        assert_eq!(NumpadState::default(), NumpadState::Unavailable);
        let r: Request = serde_json::from_str(r#"{"cmd":"set_numpad","on":true}"#).unwrap();
        assert_eq!(r, Request::SetNumpad { on: true });
        let r: Request = serde_json::from_str(r#"{"cmd":"set_numpad_config","idle_dim_secs":0}"#).unwrap();
        assert_eq!(r, Request::SetNumpadConfig { start_brightness: None, allow_when_touchpad_off: None, idle_dim_secs: Some(0), hold_ms: None, key_repeat: None, repeat_delay_ms: None, repeat_rate_hz: None });
        assert_eq!(serde_json::from_str::<KeyAction>("\"toggle_numpad\"").unwrap(), KeyAction::ToggleNumpad);
    }

    #[test]
    fn music_bands_event_wire_format() {
        assert_eq!(serde_json::to_string(&Event::MusicBands(vec![0.5, 1.0])).unwrap(), r#"{"event":"music_bands","data":[0.5,1.0]}"#);
    }

    #[test]
    fn keystone_wire_format() {
        let a: KeystoneAction = serde_json::from_str(r#"{"mode":"performance","light":"rainbow_wave","command":"notify-send hi"}"#).unwrap();
        assert_eq!((a.mode, a.light, a.lock), (Some(ModeChoice::Performance), KeystoneLight::Effect(AuraMode::RainbowWave), false));
        for (s, l) in [("\"music\"", KeystoneLight::Music), ("\"previous\"", KeystoneLight::Previous), ("\"unchanged\"", KeystoneLight::Unchanged), ("\"static\"", KeystoneLight::Effect(AuraMode::Static))] {
            assert_eq!(serde_json::from_str::<KeystoneLight>(s).unwrap(), l);
            assert_eq!(serde_json::to_string(&l).unwrap(), s);
        }
        let r: Request = serde_json::from_str(r#"{"cmd":"set_keystone_action","event":"remove","action":{"lock":true,"light":"previous"}}"#).unwrap();
        assert_eq!(r, Request::SetKeystoneAction { event: KeystoneEvent::Remove, action: KeystoneAction { lock: true, light: KeystoneLight::Previous, ..Default::default() } });
    }

    #[test]
    fn music_wire_format() {
        assert_eq!(serde_json::from_str::<KeyAction>("\"toggle_music\"").unwrap(), KeyAction::ToggleMusic);
        let r: Request = serde_json::from_str(r#"{"cmd":"set_music","on":true}"#).unwrap();
        assert_eq!(r, Request::SetMusic { on: true });
        let r: Request = serde_json::from_str(r#"{"cmd":"set_music_config","style":"pulse","colour1":[1,2,3]}"#).unwrap();
        assert_eq!(r, Request::SetMusicConfig { style: Some(MusicStyle::Pulse), scheme: None, colour1: Some([1, 2, 3]), colour2: None, sensitivity: None });
        let l: LightingState = serde_json::from_str(r#"{"brightness":3,"on_ac":true}"#).unwrap();
        assert_eq!((l.music, l.music_error), (MusicState::Unavailable, None), "older snapshots still parse");
    }
}
