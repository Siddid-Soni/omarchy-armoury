use anyhow::{Context, bail};
use armoury_root::*;
use clap::{Parser, Subcommand};
use std::path::Path;
use std::process::Command;

#[derive(Parser)]
#[command(name = "armoury-root", about = "Privileged helper for omarchy-armoury (fixed command set)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Patch asusd's model database so all lighting zones are enabled
    AsusdSupportFix,
    /// Unmask/enable/start asusd (remembers if it was masked)
    Takeover,
    /// Stop/disable asusd and restore its mask if it had one
    Handback,
}

fn systemctl(args: &[&str]) -> anyhow::Result<()> {
    let st = Command::new("systemctl").args(args).status().context("spawn systemctl")?;
    if !st.success() { bail!("systemctl {} failed ({st})", args.join(" ")); }
    Ok(())
}

fn asusd_active() -> bool {
    Command::new("systemctl").args(["is-active", "--quiet", "asusd"]).status().map(|s| s.success()).unwrap_or(false)
}

fn write_atomic(path: &Path, contents: &str) -> anyhow::Result<()> {
    let tmp = path.with_extension("armoury-tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn asusd_support_fix() -> anyhow::Result<()> {
    let board = std::fs::read_to_string(BOARD_NAME).context("read board name")?.trim().to_string();
    let Some(fix) = fix_for_board(&board) else {
        println!("armoury-root: no asusd fix needed for {board}");
        return Ok(());
    };
    let mut changed = false;
    let text = std::fs::read_to_string(SUPPORT_FILE).context("read asusd support file")?;
    match patch_support(&text, fix).map_err(anyhow::Error::msg)? {
        Some(new) => {
            std::fs::create_dir_all(STATE_DIR)?;
            std::fs::write(Path::new(STATE_DIR).join("aura_support.ron.orig"), &text)?;
            write_atomic(Path::new(SUPPORT_FILE), &new)?;
            println!("armoury-root: {} power_zones -> {:?}", fix.device, fix.zones);
            changed = true;
        }
        None => println!("armoury-root: support file already has {:?}", fix.zones),
    }
    for entry in std::fs::read_dir(AURA_CFG_DIR).into_iter().flatten().flatten() {
        let p = entry.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !(name.starts_with("aura_") && name.ends_with(".ron")) { continue; }
        if aura_config_stale(&std::fs::read_to_string(&p)?, fix) {
            std::fs::rename(&p, p.with_extension("ron.armoury-bak"))?;
            println!("armoury-root: set aside stale {name}; asusd will regenerate it");
            changed = true;
        }
    }
    if changed && asusd_active() { systemctl(&["restart", "asusd"])?; }
    Ok(())
}

fn takeover() -> anyhow::Result<()> {
    let enabled = Command::new("systemctl").args(["is-enabled", "asusd"]).output()?;
    if String::from_utf8_lossy(&enabled.stdout).trim() == "masked" {
        std::fs::create_dir_all(STATE_DIR)?;
        std::fs::write(Path::new(STATE_DIR).join("asusd-was-masked"), b"")?;
        systemctl(&["unmask", "asusd"])?;
    }
    asusd_support_fix()?;
    systemctl(&["enable", "--now", "asusd"])
}

fn handback() -> anyhow::Result<()> {
    systemctl(&["disable", "--now", "asusd"])?;
    let marker = Path::new(STATE_DIR).join("asusd-was-masked");
    if marker.exists() {
        systemctl(&["mask", "asusd"])?;
        std::fs::remove_file(marker)?;
    }
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    if !nix::unistd::geteuid().is_root() {
        eprintln!("armoury-root: must run as root (via pkexec)");
        std::process::exit(2);
    }
    let r = match cli.cmd {
        Cmd::AsusdSupportFix => asusd_support_fix(),
        Cmd::Takeover => takeover(),
        Cmd::Handback => handback(),
    };
    if let Err(e) = r {
        eprintln!("armoury-root: {e:#}");
        std::process::exit(1);
    }
}
