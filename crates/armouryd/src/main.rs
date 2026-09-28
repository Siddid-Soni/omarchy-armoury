use armouryd::control::Control;
use armouryd::hw::{services::RealServices, supergfx::SupergfxClient, sysfs::RealSysfs};
use armouryd::ipc::{Daemon, bind};
use std::path::PathBuf;
use std::time::Duration;

fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME unset")).join(".local/state"))
        .join("omarchy-armoury")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    let listener = bind(&armoury_proto::socket_path())?;
    let gfx = SupergfxClient::new().await?;
    let daemon = Daemon::new(Box::new(RealSysfs::new("/")), Box::new(gfx), Box::new(RealServices), Control::load(&dir));
    eprintln!("armouryd: {:?} mode, socket {}", daemon.control.lock().await.mode(), armoury_proto::socket_path().display());
    tokio::spawn(daemon.clone().poll_loop(Duration::from_secs(2)));
    daemon.serve(listener).await
}
