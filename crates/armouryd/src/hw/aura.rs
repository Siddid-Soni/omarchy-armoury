use super::Aura;
use crate::features::lighting::{RawMode, RawPower};
use anyhow::Context;

#[zbus::proxy(interface = "xyz.ljones.Aura", default_service = "xyz.ljones.Asusd")]
trait AuraDev {
    #[zbus(property)]
    fn led_mode_data(&self) -> zbus::Result<RawMode>;
    #[zbus(property)]
    fn set_led_mode_data(&self, value: RawMode) -> zbus::Result<()>;
    #[zbus(property)]
    fn led_power(&self) -> zbus::Result<RawPower>;
    #[zbus(property)]
    fn set_led_power(&self, value: RawPower) -> zbus::Result<()>;
    #[zbus(property)]
    fn brightness(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn set_brightness(&self, value: u32) -> zbus::Result<()>;
    #[zbus(property)]
    fn supported_basic_modes(&self) -> zbus::Result<Vec<u32>>;
    #[zbus(property)]
    fn supported_power_zones(&self) -> zbus::Result<Vec<u32>>;
}

pub struct AuraClient {
    conn: zbus::Connection,
}

impl AuraClient {
    pub async fn new() -> anyhow::Result<Self> {
        Ok(Self { conn: zbus::Connection::system().await? })
    }

    /// The laptop keyboard's Aura object (DeviceType 0), found through asusd's ObjectManager.
    /// Looked up per call: asusd comes and goes with takeover/handback.
    async fn device(&self) -> anyhow::Result<AuraDevProxy<'_>> {
        let om = zbus::fdo::ObjectManagerProxy::builder(&self.conn)
            .destination("xyz.ljones.Asusd")?
            .path("/")?
            .build()
            .await?;
        let objects = om.get_managed_objects().await.context("asusd not reachable")?;
        let mut paths: Vec<_> = objects.iter()
            .filter_map(|(path, ifaces)| {
                let aura = ifaces.iter().find(|(name, _)| name.as_str() == "xyz.ljones.Aura")?.1;
                let dev_type = aura.get("DeviceType").and_then(|v| u32::try_from(v).ok()).unwrap_or(u32::MAX);
                Some((dev_type, path.clone()))
            })
            .collect();
        paths.sort_by_key(|(t, _)| *t);
        let (_, path) = paths.into_iter().next().context("asusd has no Aura keyboard device")?;
        Ok(AuraDevProxy::builder(&self.conn)
            .path(path)?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await?)
    }
}

#[async_trait::async_trait]
impl Aura for AuraClient {
    async fn info(&self) -> anyhow::Result<(RawMode, RawPower, Vec<u32>, Vec<u32>)> {
        let d = self.device().await?;
        Ok((d.led_mode_data().await?, d.led_power().await?, d.supported_basic_modes().await?, d.supported_power_zones().await?))
    }
    async fn set_mode_data(&self, m: RawMode) -> anyhow::Result<()> { Ok(self.device().await?.set_led_mode_data(m).await?) }
    async fn set_power(&self, p: RawPower) -> anyhow::Result<()> { Ok(self.device().await?.set_led_power(p).await?) }
    async fn set_brightness(&self, level: u32) -> anyhow::Result<()> { Ok(self.device().await?.set_brightness(level).await?) }
}

#[cfg(test)]
mod tests {
    use crate::features::lighting::{RawMode, RawPower};
    use zbus::zvariant::Type;

    #[test]
    fn raw_types_match_asusd_signatures() {
        // LedModeData and LedPower on asusd 6.4.0
        assert_eq!(<RawMode as Type>::SIGNATURE.to_string(), "(uu(yyy)(yyy)ss)");
        assert_eq!(<RawPower as Type>::SIGNATURE.to_string(), "(a(ubbbb))");
    }
}
