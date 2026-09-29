use armouryd::config::config_path;
use armouryd::control::{Control, state_dir};
use armouryd::hw::{asusd::AsusdClient, aura::AuraClient, nvidia::RealNvidia, services::RealServices, supergfx::SupergfxClient, sysfs::RealSysfs};
use armouryd::ipc::{Daemon, bind};
use std::path::PathBuf;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME unset"));
    let dir = state_dir(&home);
    std::fs::create_dir_all(&dir)?;
    let listener = bind(&armoury_proto::socket_path())?;
    let gfx = SupergfxClient::new().await?;
    let asusd = AsusdClient::new().await?;
    let daemon = Daemon::new(
        Box::new(RealSysfs::new("/")),
        Box::new(gfx),
        Box::new(RealServices::default()),
        Box::new(asusd),
        Control::load(&dir),
        config_path(&home),
        Box::new(RealNvidia::new("/")),
    )
    .with_aura(Box::new(AuraClient::new().await?));
    eprintln!("armouryd: {:?} mode, socket {}", daemon.control.lock().await.mode(), armoury_proto::socket_path().display());
    tokio::spawn(daemon.clone().poll_loop(Duration::from_secs(2)));
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(armouryd::features::keys::run_reader(tx));
    let keys = daemon.clone();
    tokio::spawn(async move { while let Some(k) = rx.recv().await { keys.on_hotkey(k).await; } });
    let (ktx, mut krx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(armouryd::features::keys::run_kbd_watch(ktx));
    let kbd = daemon.clone();
    tokio::spawn(async move { while let Some(l) = krx.recv().await { kbd.on_kbd_brightness(l).await; } });
    daemon.serve(listener).await
}
