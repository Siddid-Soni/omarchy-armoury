use armouryd::config::config_path;
use armouryd::control::{Control, state_dir};
use armouryd::hw::{asusd::AsusdClient, nvidia::RealNvidia, services::RealServices, supergfx::SupergfxClient, sysfs::RealSysfs};
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
    );
    eprintln!("armouryd: {:?} mode, socket {}", daemon.control.lock().await.mode(), armoury_proto::socket_path().display());
    tokio::spawn(daemon.clone().poll_loop(Duration::from_secs(2)));
    daemon.serve(listener).await
}
