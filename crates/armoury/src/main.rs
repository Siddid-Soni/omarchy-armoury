use anyhow::{Context, bail};
use armoury_proto::{AuraEffect, AuraMode, AuraZone, ControlMode, LightingInfo, ZonePower, Epp, Fan, FanCurve, GpuMode, GpuStep, GpuSwitchResult, ModeSettings, Profile, Request, Response, Snapshot, socket_path};
use clap::{Parser, Subcommand};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

#[derive(Parser)]
#[command(name = "armoury", about = "Control ASUS ROG hardware via armouryd")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show hardware state
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Check the daemon is alive
    Ping,
    /// Stop G-Helper, start asusd, let armouryd control hardware
    Takeover,
    /// Stop asusd, restart G-Helper, armouryd back to observe mode
    Handback,
    /// Stream state changes as JSON lines
    Watch,
    /// Show or change the performance mode
    Profile {
        #[command(subcommand)]
        action: Option<ProfileAction>,
    },
    /// Show or edit a mode's fan curves
    Fan {
        #[arg(value_parser = parse_profile)]
        profile: Profile,
        #[command(subcommand)]
        action: Option<FanAction>,
    },
    /// Keyboard, lightbar and logo lighting
    Light {
        #[command(subcommand)]
        action: Option<LightAction>,
    },
    /// Keyboard backlight idle dim (driven by the shell's idle monitor)
    Kbd {
        #[command(subcommand)]
        action: KbdAction,
    },
    /// CPU undervolt availability
    Undervolt {
        #[command(subcommand)]
        action: UvAction,
    },
    /// NVIDIA dGPU information
    Gpu {
        #[command(subcommand)]
        action: GpuAction,
    },
    /// Show or edit a mode's power limits, EPP, CPU boost, undervolt and GPU clocks
    Mode {
        #[arg(value_parser = parse_profile)]
        profile: Profile,
        #[command(subcommand)]
        action: Option<ModeAction>,
    },
}

#[derive(Subcommand)]
enum ProfileAction {
    Set {
        #[arg(value_parser = parse_profile)]
        profile: Profile,
    },
    Next,
}

#[derive(Subcommand)]
enum FanAction {
    /// 8 points as TEMP:PERCENT, e.g. 40:0,50:10,60:20,70:30,80:40,90:60,95:80,97:100
    Set {
        #[arg(value_parser = parse_fan)]
        fan: Fan,
        points: String,
    },
    Reset,
}

#[derive(Subcommand)]
enum ModeAction {
    Set {
        #[arg(long)]
        pl1: Option<i32>,
        #[arg(long)]
        pl2: Option<i32>,
        #[arg(long)]
        nv_boost: Option<i32>,
        #[arg(long)]
        nv_temp: Option<i32>,
        #[arg(long, value_parser = parse_epp)]
        epp: Option<Epp>,
        #[arg(long, value_parser = ["on", "off"])]
        cpu_boost: Option<String>,
        /// CPU undervolt, mV (-150..0; applied only if the BIOS allows it)
        #[arg(long, allow_hyphen_values = true)]
        uv: Option<i32>,
        /// GPU core clock offset, MHz
        #[arg(long, allow_hyphen_values = true)]
        gpu_core: Option<i32>,
        /// GPU memory clock offset, MHz
        #[arg(long, allow_hyphen_values = true)]
        gpu_mem: Option<i32>,
        /// Max GPU core clock, MHz, or off
        #[arg(long, value_parser = parse_lock)]
        gpu_core_lock: Option<u32>,
        /// Max GPU memory clock, MHz, or off
        #[arg(long, value_parser = parse_lock)]
        gpu_mem_lock: Option<u32>,
    },
}

#[derive(Subcommand)]
enum LightAction {
    /// Set keyboard brightness (off|low|med|high or 0-3); remembered separately for AC and battery
    Brightness {
        #[arg(value_parser = parse_level)]
        level: u8,
    },
    /// Set the lighting effect
    Effect {
        #[arg(value_parser = parse_aura_mode)]
        mode: AuraMode,
        /// Primary colour, RRGGBB
        #[arg(long, value_parser = parse_colour, default_value = "ff0000")]
        color: [u8; 3],
        /// Secondary colour, RRGGBB
        #[arg(long, value_parser = parse_colour, default_value = "000000")]
        color2: [u8; 3],
        #[arg(long, value_parser = ["low", "med", "high"], default_value = "med")]
        speed: String,
        #[arg(long, value_parser = ["right", "left", "up", "down"], default_value = "right")]
        direction: String,
    },
    /// Turn a zone on/off per power state; unspecified states keep their current value
    Zone {
        #[arg(value_parser = ["keyboard", "lightbar", "logo", "lid", "rear"])]
        zone: String,
        #[arg(long, value_parser = ["on", "off"])]
        boot: Option<String>,
        #[arg(long, value_parser = ["on", "off"])]
        awake: Option<String>,
        #[arg(long, value_parser = ["on", "off"])]
        sleep: Option<String>,
        #[arg(long, value_parser = ["on", "off"])]
        shutdown: Option<String>,
    },
}

#[derive(Subcommand)]
enum KbdAction {
    /// Dim the keyboard (remembers the current level)
    Idle,
    /// Restore the level from before `idle`
    Resume,
}

fn parse_colour(s: &str) -> Result<[u8; 3], String> {
    let h = s.trim_start_matches('#');
    if h.len() != 6 { return Err(format!("colour must be RRGGBB, got {s:?}")); }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| format!("colour must be RRGGBB, got {s:?}"));
    Ok([byte(0)?, byte(2)?, byte(4)?])
}

fn parse_level(s: &str) -> Result<u8, String> {
    match s {
        "off" => Ok(0), "low" => Ok(1), "med" => Ok(2), "high" => Ok(3),
        n => n.parse().ok().filter(|v| *v <= 3).ok_or_else(|| format!("brightness must be off|low|med|high or 0-3, got {s:?}")),
    }
}

fn parse_aura_mode(s: &str) -> Result<AuraMode, String> {
    serde_json::from_value(serde_json::json!(s.replace('-', "_")))
        .map_err(|_| format!("unknown effect {s:?} (static, breathe, rainbow-cycle, rainbow-wave, star, rain, highlight, laser, ripple, pulse, comet, flash)"))
}

#[derive(Subcommand)]
enum UvAction {
    /// Re-check whether the BIOS allows undervolting
    Probe,
}

#[derive(Subcommand)]
enum GpuAction {
    /// Processes keeping the dGPU awake
    Users,
    /// Show what switching to a mode would do, without doing it
    Plan {
        #[arg(value_parser = parse_gpu_mode)]
        mode: GpuMode,
    },
    /// Switch GPU mode (integrated|hybrid|ultimate); a reboot finishes it
    Set {
        #[arg(value_parser = parse_gpu_mode)]
        mode: GpuMode,
    },
}

fn parse_gpu_mode(s: &str) -> Result<GpuMode, String> {
    match s {
        "integrated" | "eco" => Ok(GpuMode::Integrated),
        "hybrid" | "standard" => Ok(GpuMode::Hybrid),
        "ultimate" => Ok(GpuMode::AsusMuxDgpu),
        _ => Err(format!("unknown GPU mode {s:?} (integrated|hybrid|ultimate)")),
    }
}

fn describe_step(step: &GpuStep) -> String {
    match step {
        GpuStep::OmarchyToggle { to } => format!("run Omarchy's GPU toggle to {to:?} (it asks to confirm, then reboots)"),
        GpuStep::Supergfx { to } => format!("supergfxd switches to {to:?}; reboot to finish"),
        GpuStep::FirstOfTwo { first, then } => format!("two steps: 1) {} 2) after reboot, switch to {then:?}", describe_step(first)),
    }
}

fn parse_lock(s: &str) -> Result<u32, String> {
    if s == "off" { return Ok(0); }
    s.parse().map_err(|_| format!("expected MHz or off, got {s:?}"))
}

fn parse_profile(s: &str) -> Result<Profile, String> {
    Profile::from_sysfs(s).ok_or_else(|| format!("unknown mode {s} (quiet|balanced|performance)"))
}

fn parse_fan(s: &str) -> Result<Fan, String> {
    serde_json::from_value(serde_json::json!(s)).map_err(|_| format!("unknown fan {s} (cpu|gpu|mid)"))
}

fn parse_epp(s: &str) -> Result<Epp, String> {
    serde_json::from_value(serde_json::json!(s))
        .map_err(|_| format!("unknown EPP {s} (default|performance|balance_performance|balance_power|power)"))
}

fn parse_curve(fan: Fan, s: &str) -> Result<FanCurve, String> {
    let pts: Vec<&str> = s.split(',').map(str::trim).collect();
    if pts.len() != 8 { return Err(format!("need exactly 8 TEMP:PERCENT points, got {}", pts.len())); }
    let mut temps = [0u8; 8];
    let mut percent = [0u8; 8];
    for (i, p) in pts.iter().enumerate() {
        let (t, v) = p.split_once(':').ok_or_else(|| format!("point {p:?} is not TEMP:PERCENT"))?;
        temps[i] = t.trim_end_matches('c').parse().map_err(|_| format!("bad temperature in {p:?}"))?;
        percent[i] = v.trim_end_matches('%').parse().map_err(|_| format!("bad percent in {p:?}"))?;
    }
    Ok(FanCurve { fan, temps, percent, enabled: true })
}

fn read_timeout(req: &Request) -> std::time::Duration {
    std::time::Duration::from_secs(match req {
        Request::Takeover | Request::Handback => 180,
        Request::SetGpuMode { .. } | Request::PlanGpuMode { .. } => 60,
        _ => 10,
    })
}

fn connect() -> anyhow::Result<UnixStream> {
    let p = socket_path();
    UnixStream::connect(&p).with_context(|| format!("armouryd not running ({})", p.display()))
}

fn call(req: &Request) -> anyhow::Result<serde_json::Value> {
    let mut s = connect()?;
    s.set_read_timeout(Some(read_timeout(req)))?;
    writeln!(s, "{}", serde_json::to_string(req)?)?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).context("no answer from armouryd")?;
    let r: Response = serde_json::from_str(&line).context("bad response from armouryd")?;
    if !r.ok { bail!("{}", r.error.unwrap_or_default()); }
    Ok(r.data.unwrap_or_default())
}

fn summary(s: &Snapshot) -> String {
    let opt = |v: Option<String>| v.unwrap_or_else(|| "-".into());
    let control = match s.control { ControlMode::Observe => "observe", ControlMode::Active => "active" };
    let gpu = opt(s.gpu.mode.map(|m| format!("{m:?}")));
    let pending = s.gpu.pending.map(|m| format!(" → {m:?} after reboot")).unwrap_or_default();
    let keystone = match s.keystone { Some(true) => "inserted", Some(false) => "removed", None => "-" };
    let battery = match (s.battery.capacity, s.battery.charge_limit) {
        (Some(c), Some(l)) => format!("{c}% (limit {l}%)"),
        (Some(c), None) => format!("{c}%"),
        _ => "-".into(),
    };
    let perf = {
        let p = &s.perf;
        let mut parts = vec![p.profile.map(|m| m.sysfs().to_string()).unwrap_or_else(|| "-".into())];
        if let Some(t) = p.cpu_temp_c { parts.push(format!("CPU {t:.0}°C")); }
        if let (Some(c), Some(g)) = (p.cpu_fan_rpm, p.gpu_fan_rpm) { parts.push(format!("fans {c}/{g} rpm")); }
        if let Some(w) = p.power_draw_w { parts.push(format!("{w:.1} W")); }
        parts.join(" · ")
    };
    let dgpu = match (s.gpu.dgpu_active, &s.gpu.nvidia) {
        (Some(true), Some(n)) => {
            let users = s.gpu.users.len();
            let mut line = format!("active · {}/{} MHz · {}°C · {:.1} W", n.core_mhz, n.mem_mhz, n.temp_c, n.power_w);
            if n.core_offset != 0 { line.push_str(&format!(" · offset {:+}", n.core_offset)); }
            line.push_str(&format!(" · {users} user{}", if users == 1 { "" } else { "s" }));
            line
        }
        (Some(true), None) => "active".into(),
        (Some(false), _) => "suspended".into(),
        (None, _) => "-".into(),
    };
    let uv = match s.perf.undervolt { Some(u) if u.unlocked => "unlocked", Some(_) => "locked", None => "-" };
    format!(
        "Model      {}\nControl    {control}\nProfile    {}\nMode       {perf}\nUndervolt  {uv}\nGPU        {gpu}{pending}\ndGPU       {dgpu}\nKeystone   {keystone}\nBattery    {battery}\nasusd      {}\nG-Helper   {}\n",
        opt(s.model.clone()),
        opt(s.platform_profile.clone()),
        if s.asusd_running { "running" } else { "stopped" },
        if s.ghelper_running { "running" } else { "stopped" },
    )
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.cmd {
        Cmd::Status { json } => {
            let v = call(&Request::Status)?;
            if json { println!("{v}"); } else { print!("{}", summary(&serde_json::from_value(v)?)); }
        }
        Cmd::Ping => println!("{}", call(&Request::Ping)?),
        Cmd::Takeover => { call(&Request::Takeover)?; println!("armouryd active: G-Helper stopped, asusd running"); }
        Cmd::Handback => { call(&Request::Handback)?; println!("armouryd observing: asusd stopped, G-Helper running"); }
        Cmd::Watch => {
            let mut s = connect()?;
            writeln!(s, "{}", serde_json::to_string(&Request::Subscribe)?)?;
            for line in BufReader::new(s).lines().skip(1) { println!("{}", line?); }
        }
        Cmd::Light { action: None } => {
            let s: Snapshot = serde_json::from_value(call(&Request::Status)?)?;
            let names = ["off", "low", "med", "high"];
            let level = s.lighting.brightness.and_then(|b| names.get(b as usize).copied()).unwrap_or("-");
            let source = match s.lighting.on_ac { Some(true) => "on AC", Some(false) => "on battery", None => "" };
            println!("Brightness {level} ({source})");
            match call(&Request::Lighting).and_then(|v| Ok(serde_json::from_value::<LightingInfo>(v)?)) {
                Ok(info) => {
                    if let Some(e) = info.effect {
                        let hex = |c: [u8; 3]| format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
                        println!("Effect     {:?} {} {} {:?} {:?}", e.mode, hex(e.colour1), hex(e.colour2), e.speed, e.direction);
                    }
                    for z in info.zones {
                        let on = |b: bool| if b { "on " } else { "off" };
                        println!("Zone       {:<9} boot {} awake {} sleep {} shutdown {}", format!("{:?}", z.zone), on(z.boot), on(z.awake), on(z.sleep), on(z.shutdown));
                    }
                }
                Err(e) => println!("(effects and zones need asusd: {e:#})"),
            }
        }
        Cmd::Light { action: Some(LightAction::Brightness { level }) } => { call(&Request::SetBrightness { level })?; }
        Cmd::Light { action: Some(LightAction::Effect { mode, color, color2, speed, direction }) } => {
            let effect = AuraEffect {
                mode, colour1: color, colour2: color2,
                speed: serde_json::from_value(serde_json::json!(speed))?,
                direction: serde_json::from_value(serde_json::json!(direction))?,
            };
            call(&Request::SetEffect { effect })?;
        }
        Cmd::Light { action: Some(LightAction::Zone { zone, boot, awake, sleep, shutdown }) } => {
            let zone = match zone.as_str() {
                "keyboard" => AuraZone::Keyboard, "lightbar" => AuraZone::Lightbar, "logo" => AuraZone::Logo,
                "lid" => AuraZone::Lid, _ => AuraZone::RearGlow,
            };
            let info: LightingInfo = serde_json::from_value(call(&Request::Lighting)?)?;
            let cur = info.zones.iter().find(|z| z.zone == zone).copied()
                .unwrap_or(ZonePower { zone, boot: false, awake: false, sleep: false, shutdown: false });
            let pick = |v: Option<String>, d: bool| v.map_or(d, |s| s == "on");
            let z = ZonePower { zone, boot: pick(boot, cur.boot), awake: pick(awake, cur.awake), sleep: pick(sleep, cur.sleep), shutdown: pick(shutdown, cur.shutdown) };
            call(&Request::SetZonePower { zone: z })?;
        }
        Cmd::Kbd { action: KbdAction::Idle } => { call(&Request::KbdIdle)?; }
        Cmd::Kbd { action: KbdAction::Resume } => { call(&Request::KbdResume)?; }
        Cmd::Undervolt { action: UvAction::Probe } => {
            let v = call(&Request::ProbeUndervolt)?;
            println!("{}", if v["unlocked"] == true { "unlocked" } else { "locked" });
        }
        Cmd::Gpu { action: GpuAction::Plan { mode } } => {
            let step: GpuStep = serde_json::from_value(call(&Request::PlanGpuMode { mode })?)?;
            println!("{}", describe_step(&step));
        }
        Cmd::Gpu { action: GpuAction::Set { mode } } => {
            if mode == GpuMode::Integrated {
                let s: Snapshot = serde_json::from_value(call(&Request::Status)?)?;
                if !s.gpu.users.is_empty() {
                    let names: Vec<String> = s.gpu.users.iter().map(|u| u.name.clone()).collect();
                    eprintln!("note: these processes use the dGPU and will lose it: {}", names.join(", "));
                }
            }
            let r: GpuSwitchResult = serde_json::from_value(call(&Request::SetGpuMode { mode })?)?;
            println!("{}", r.message);
        }
        Cmd::Gpu { action: GpuAction::Users } => {
            let s: Snapshot = serde_json::from_value(call(&Request::Status)?)?;
            match s.gpu.dgpu_active {
                Some(true) if s.gpu.users.is_empty() => println!("dGPU awake, no user processes"),
                Some(true) => for u in s.gpu.users { println!("{:>7}  {}", u.pid, u.name) },
                Some(false) => println!("dGPU suspended (nothing is using it)"),
                None => println!("no NVIDIA dGPU"),
            }
        }
        Cmd::Profile { action } => {
            let req = match action {
                None => Request::Status,
                Some(ProfileAction::Set { profile }) => Request::SetProfile { profile },
                Some(ProfileAction::Next) => Request::NextProfile,
            };
            let s: Snapshot = serde_json::from_value(call(&req)?)?;
            println!("{}", s.perf.profile.map(|p| p.sysfs()).unwrap_or("-"));
        }
        Cmd::Fan { profile, action } => {
            let req = match action {
                None => Request::FanCurves { profile },
                Some(FanAction::Set { fan, points }) => {
                    Request::SetFanCurve { profile, curve: parse_curve(fan, &points).map_err(anyhow::Error::msg)? }
                }
                Some(FanAction::Reset) => Request::ResetFanCurves { profile },
            };
            let curves: Vec<FanCurve> = serde_json::from_value(call(&req)?)?;
            for c in curves {
                let pts: Vec<String> = c.temps.iter().zip(c.percent).map(|(t, p)| format!("{t}:{p}")).collect();
                println!("{:?} {} {}", c.fan, if c.enabled { "on " } else { "off" }, pts.join(","));
            }
        }
        Cmd::Mode { profile, action } => {
            let req = match action {
                None => Request::ModeSettings { profile },
                Some(ModeAction::Set { pl1, pl2, nv_boost, nv_temp, epp, cpu_boost, uv, gpu_core, gpu_mem, gpu_core_lock, gpu_mem_lock }) => {
                    Request::SetModeSettings {
                        profile,
                        settings: ModeSettings {
                            pl1, pl2, nv_boost, nv_temp, epp,
                            cpu_boost: cpu_boost.map(|v| v == "on"),
                            uv_mv: uv, gpu_core_offset: gpu_core, gpu_mem_offset: gpu_mem, gpu_core_lock, gpu_mem_lock,
                        },
                    }
                }
            };
            println!("{}", serde_json::to_string_pretty(&call(&req)?)?);
        }
    }
    Ok(())
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("armoury: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_formats_key_fields() {
        let mut s = Snapshot::default();
        s.model = Some("ROG Strix G533ZW_G533ZW".into());
        s.keystone = Some(true);
        s.gpu.mode = Some(armoury_proto::GpuMode::Hybrid);
        s.battery.capacity = Some(80);
        s.battery.charge_limit = Some(80);
        let out = summary(&s);
        assert!(out.contains("Model      ROG Strix G533ZW_G533ZW"));
        assert!(out.contains("Control    observe"));
        assert!(out.contains("GPU        Hybrid"));
        assert!(out.contains("Keystone   inserted"));
        assert!(out.contains("Battery    80% (limit 80%)"));
    }

    #[test]
    fn read_timeouts() {
        assert_eq!(read_timeout(&Request::Status), std::time::Duration::from_secs(10));
        assert_eq!(read_timeout(&Request::Takeover), std::time::Duration::from_secs(180));
        assert_eq!(read_timeout(&Request::SetGpuMode { mode: armoury_proto::GpuMode::Hybrid }), std::time::Duration::from_secs(60));
    }

    #[test]
    fn parse_curve_points() {
        let c = parse_curve(armoury_proto::Fan::Gpu, "40:0,50:10%,60:20,70:30,80:40,90:60,95:80,97:100").unwrap();
        assert_eq!(c.temps, [40, 50, 60, 70, 80, 90, 95, 97]);
        assert_eq!(c.percent[1], 10);
        assert!(c.enabled);
        assert!(parse_curve(armoury_proto::Fan::Cpu, "40:0,50:10").unwrap_err().contains("8"));
        assert!(parse_curve(armoury_proto::Fan::Cpu, "a:b,1:1,1:1,1:1,1:1,1:1,1:1,1:1").is_err());
    }

    #[test]
    fn summary_has_perf_line() {
        let mut s = Snapshot::default();
        s.perf.profile = Some(armoury_proto::Profile::Performance);
        s.perf.cpu_temp_c = Some(72.0);
        s.perf.cpu_fan_rpm = Some(3300);
        s.perf.gpu_fan_rpm = Some(5200);
        s.perf.power_draw_w = Some(18.25);
        assert!(summary(&s).contains("Mode       performance · CPU 72°C · fans 3300/5200 rpm · 18.2 W"), "{}", summary(&s));
    }

    #[test]
    fn lock_parsing() {
        assert_eq!(parse_lock("off"), Ok(0));
        assert_eq!(parse_lock("1500"), Ok(1500));
        assert!(parse_lock("fast").is_err());
    }

    #[test]
    fn summary_gpu_and_undervolt_lines() {
        let mut s = Snapshot::default();
        s.gpu.dgpu_active = Some(false);
        s.perf.undervolt = Some(armoury_proto::UndervoltState { unlocked: false });
        let out = summary(&s);
        assert!(out.contains("dGPU       suspended"), "{out}");
        assert!(out.contains("Undervolt  locked"), "{out}");
        s.gpu.dgpu_active = Some(true);
        s.gpu.nvidia = Some(armoury_proto::NvStatus { core_mhz: 1500, mem_mhz: 7000, temp_c: 62, power_w: 35.2, pstate: "Zero".into(), core_offset: 50, ..Default::default() });
        s.gpu.users = vec![armoury_proto::GpuUser { pid: 1, name: "a".into() }];
        assert!(summary(&s).contains("dGPU       active · 1500/7000 MHz · 62°C · 35.2 W · offset +50 · 1 user"), "{}", summary(&s));
    }

    #[test]
    fn gpu_mode_names() {
        assert_eq!(parse_gpu_mode("ultimate"), Ok(armoury_proto::GpuMode::AsusMuxDgpu));
        assert_eq!(parse_gpu_mode("integrated"), Ok(armoury_proto::GpuMode::Integrated));
        assert_eq!(parse_gpu_mode("hybrid"), Ok(armoury_proto::GpuMode::Hybrid));
        assert!(parse_gpu_mode("vfio").is_err());
    }

    #[test]
    fn lighting_parsers() {
        assert_eq!(parse_colour("ff0080"), Ok([255, 0, 128]));
        assert_eq!(parse_colour("#00FF00"), Ok([0, 255, 0]));
        assert!(parse_colour("fff").is_err());
        assert_eq!(parse_level("off"), Ok(0));
        assert_eq!(parse_level("high"), Ok(3));
        assert_eq!(parse_level("2"), Ok(2));
        assert!(parse_level("4").is_err());
        assert_eq!(parse_aura_mode("rainbow-wave"), Ok(armoury_proto::AuraMode::RainbowWave));
        assert!(parse_aura_mode("disco").is_err());
    }
}
