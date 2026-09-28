use super::Gfx;

#[zbus::proxy(
    interface = "org.supergfxctl.Daemon",
    default_service = "org.supergfxctl.Daemon",
    default_path = "/org/supergfxctl/Gfx"
)]
trait Supergfx {
    fn mode(&self) -> zbus::Result<u32>;
    fn supported(&self) -> zbus::Result<Vec<u32>>;
    fn pending_mode(&self) -> zbus::Result<u32>;
    fn power(&self) -> zbus::Result<u32>;
}

pub struct SupergfxClient {
    proxy: SupergfxProxy<'static>,
}

impl SupergfxClient {
    pub async fn new() -> anyhow::Result<Self> {
        let conn = zbus::Connection::system().await?;
        Ok(Self { proxy: SupergfxProxy::new(&conn).await? })
    }
}

#[async_trait::async_trait]
impl Gfx for SupergfxClient {
    async fn mode(&self) -> anyhow::Result<u32> { Ok(self.proxy.mode().await?) }
    async fn supported(&self) -> anyhow::Result<Vec<u32>> { Ok(self.proxy.supported().await?) }
    async fn pending_mode(&self) -> anyhow::Result<u32> { Ok(self.proxy.pending_mode().await?) }
    async fn power(&self) -> anyhow::Result<u32> { Ok(self.proxy.power().await?) }
}
