use anyhow::{Context, bail};
use armoury_root::*;
use clap::{Parser, Subcommand};
use std::path::Path;
use std::process::Command;

mod msr;
mod nv;

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
    /// Reinstall the packaged aura_support.ron (used by uninstall)
    AsusdSupportRestore,
    /// Write power limits / CPU boost: pl1= pl2= nv_boost= nv_temp= cpu_boost=on|off
    SetLimits { args: Vec<String> },
    /// NVIDIA clocks: <core_off> <mem_off> <core_lock|off> <mem_lock|off> (MHz)
    NvClocks {
        #[arg(allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Intel CPU undervolt via the OC mailbox (MSR 0x150)
    Undervolt {
        #[command(subcommand)]
        action: UvAction,
    },
}

#[derive(Subcommand)]
enum UvAction {
    /// Write -5 mV, read it back, restore 0; prints unlocked|locked
    Probe,
    /// Set core+cache offset (-150..0 mV)
    Set {
        #[arg(allow_hyphen_values = true)]
        mv: String,
    },
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

/// Returns whether anything changed.
fn asusd_support_fix() -> anyhow::Result<bool> {
    let board = std::fs::read_to_string(BOARD_NAME).context("read board name")?.trim().to_string();
    let Some(fix) = fix_for_board(&board) else {
        println!("armoury-root: no asusd fix needed for {board}");
        return Ok(false);
    };
    let text = std::fs::read_to_string(SUPPORT_FILE).context("read asusd support file")?;
    let support_changed = match patch_support(&text, fix).map_err(anyhow::Error::msg)? {
        Some(new) => {
            write_atomic(Path::new(SUPPORT_FILE), &new)?;
            println!("armoury-root: {} power_zones -> {:?}", fix.device, fix.zones);
            true
        }
        None => {
            println!("armoury-root: support file already has {:?}", fix.zones);
            false
        }
    };
    let configs: Vec<(String, String)> = std::fs::read_dir(AURA_CFG_DIR).into_iter().flatten().flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            (name.starts_with("aura_") && name.ends_with(".ron")).then_some(())?;
            Some((name, std::fs::read_to_string(e.path()).ok()?))
        })
        .collect();
    for name in stale_configs(support_changed, &configs, fix) {
        let p = Path::new(AURA_CFG_DIR).join(name);
        let bak = backup_path(&p, |q| q.exists());
        std::fs::rename(&p, &bak)?;
        println!("armoury-root: moved stale {name} to {}; asusd will regenerate it", bak.display());
    }
    if support_changed && asusd_active() { systemctl(&["restart", "asusd"])?; }
    Ok(support_changed)
}

/// Put the packaged aura_support.ron back (uninstall).
fn asusd_support_restore() -> anyhow::Result<()> {
    let q = Command::new("pacman").args(["-Q", "asusctl"]).output()?;
    let cached = cached_package(&String::from_utf8_lossy(&q.stdout), std::env::consts::ARCH)
        .filter(|p| Path::new(p).exists());
    match cached {
        Some(pkg) => {
            let out = Command::new("bsdtar").args(["-xOf", &pkg, "usr/share/asusd/aura_support.ron"]).output()?;
            if !out.status.success() { bail!("bsdtar failed on {pkg}"); }
            write_atomic(Path::new(SUPPORT_FILE), &String::from_utf8(out.stdout)?)?;
        }
        None => {
            let st = Command::new("pacman").args(["-S", "--noconfirm", "asusctl"]).status()?;
            if !st.success() { bail!("pacman -S asusctl failed"); }
        }
    }
    println!("armoury-root: restored packaged {SUPPORT_FILE}");
    Ok(())
}

/// Hardware-writing commands only run while the calling user's armouryd is active.
fn require_active() -> anyhow::Result<()> {
    let uid = std::env::var("PKEXEC_UID").ok().and_then(|u| u.parse().ok()).context("must be run via pkexec")?;
    let user = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))?.context("unknown calling user")?;
    if !active_flag(&user.dir).exists() { bail!("armouryd is not in active mode; run 'armoury takeover' first"); }
    Ok(())
}

fn undervolt(action: UvAction) -> anyhow::Result<()> {
    if let UvAction::Set { mv } = &action { parse_uv(mv).map_err(anyhow::Error::msg)?; }
    require_active()?;
    let msr = msr::Msr::open()?;
    match action {
        UvAction::Probe => {
            let (core, _) = msr.set_uv(-5)?;
            msr.set_uv(0)?;
            println!("{}", if uv_matches(-5, core) { "unlocked" } else { "locked" });
        }
        UvAction::Set { mv } => {
            let mv = parse_uv(&mv).map_err(anyhow::Error::msg)?;
            let (core, cache) = msr.set_uv(mv)?;
            println!("core={core} cache={cache}");
            if !uv_matches(mv, core) { bail!("requested {mv} mV, read back {core} mV: undervolt is BIOS-locked"); }
        }
    }
    Ok(())
}

fn nv_clocks(args: &[String]) -> anyhow::Result<()> {
    let a = parse_nv_args(args).map_err(anyhow::Error::msg)?;
    require_active()?;
    let (core, mem) = nv::apply(a)?;
    println!("core_offset={core} mem_offset={mem}");
    Ok(())
}

fn set_limits(args: &[String]) -> anyhow::Result<()> {
    let pairs = parse_limits(args).map_err(anyhow::Error::msg)?;
    require_active()?;
    let errors = write_all(&pairs, |node, value| std::fs::write(node, value).map_err(|e| e.to_string()));
    if !errors.is_empty() { bail!("{}", errors.join("; ")); }
    Ok(())
}

struct RealHost;

impl Host for RealHost {
    fn systemctl(&mut self, args: &[&str]) -> anyhow::Result<()> { systemctl(args) }
    fn asusd_masked(&mut self) -> bool {
        Command::new("systemctl").args(["is-enabled", "asusd"]).output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "masked").unwrap_or(false)
    }
    fn set_marker(&mut self, present: bool) -> anyhow::Result<()> {
        let m = Path::new(STATE_DIR).join("asusd-was-masked");
        if present {
            std::fs::create_dir_all(STATE_DIR)?;
            std::fs::write(m, b"")?;
        } else if m.exists() {
            std::fs::remove_file(m)?;
        }
        Ok(())
    }
    fn marker_exists(&mut self) -> bool { Path::new(STATE_DIR).join("asusd-was-masked").exists() }
    fn support_fix(&mut self) -> anyhow::Result<()> { asusd_support_fix().map(|_| ()) }
    fn warn(&mut self, msg: &str) { eprintln!("armoury-root: warning: {msg}"); }
}

fn main() {
    let cli = Cli::parse();
    if !nix::unistd::geteuid().is_root() {
        eprintln!("armoury-root: must run as root (via pkexec)");
        std::process::exit(2);
    }
    let r = match cli.cmd {
        Cmd::AsusdSupportFix => asusd_support_fix().map(|_| ()),
        Cmd::Takeover => takeover(&mut RealHost),
        Cmd::Handback => handback(&mut RealHost),
        Cmd::AsusdSupportRestore => asusd_support_restore(),
        Cmd::SetLimits { args } => set_limits(&args),
        Cmd::Undervolt { action } => undervolt(action),
        Cmd::NvClocks { args } => nv_clocks(&args),
    };
    if let Err(e) = r {
        eprintln!("armoury-root: {e:#}");
        std::process::exit(1);
    }
}
