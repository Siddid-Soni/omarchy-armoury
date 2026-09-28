use anyhow::{Context, bail};
use armoury_proto::{ControlMode, Request, Response, Snapshot, socket_path};
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
}

fn connect() -> anyhow::Result<UnixStream> {
    let p = socket_path();
    UnixStream::connect(&p).with_context(|| format!("armouryd not running ({})", p.display()))
}

fn call(req: &Request) -> anyhow::Result<serde_json::Value> {
    let mut s = connect()?;
    writeln!(s, "{}", serde_json::to_string(req)?)?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line)?;
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
    format!(
        "Model      {}\nControl    {control}\nProfile    {}\nGPU        {gpu}{pending}\nKeystone   {keystone}\nBattery    {battery}\nasusd      {}\nG-Helper   {}\n",
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
}
