use armouryd::hw::{Gfx, supergfx::SupergfxClient};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let g = SupergfxClient::new().await?;
    println!("mode={} supported={:?} pending={} power={}", g.mode().await?, g.supported().await?, g.pending_mode().await?, g.power().await?);
    Ok(())
}
