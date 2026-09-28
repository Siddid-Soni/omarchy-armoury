use anyhow::{Context, bail};
use armoury_proto::{ControlMode, Epp, Fan, FanCurve, ModeSettings, Profile, Request, Response, Snapshot, socket_path};
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
    /// Show or edit a mode's power limits, EPP and CPU boost
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
    },
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
    format!(
        "Model      {}\nControl    {control}\nProfile    {}\nMode       {perf}\nGPU        {gpu}{pending}\nKeystone   {keystone}\nBattery    {battery}\nasusd      {}\nG-Helper   {}\n",
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
                Some(ModeAction::Set { pl1, pl2, nv_boost, nv_temp, epp, cpu_boost }) => Request::SetModeSettings {
                    profile,
                    settings: ModeSettings { pl1, pl2, nv_boost, nv_temp, epp, cpu_boost: cpu_boost.map(|v| v == "on"), ..Default::default() },
                },
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
}
