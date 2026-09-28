use super::Asusd;
use crate::features::fan::RawCurve;

#[zbus::proxy(interface = "xyz.ljones.Platform", default_service = "xyz.ljones.Asusd", default_path = "/xyz/ljones")]
trait Platform {
    fn next_platform_profile(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn platform_profile(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn set_platform_profile(&self, value: u32) -> zbus::Result<()>;
    #[zbus(property)]
    fn profile_quiet_epp(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn set_profile_quiet_epp(&self, value: u32) -> zbus::Result<()>;
    #[zbus(property)]
    fn profile_balanced_epp(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn set_profile_balanced_epp(&self, value: u32) -> zbus::Result<()>;
    #[zbus(property)]
    fn profile_performance_epp(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn set_profile_performance_epp(&self, value: u32) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "xyz.ljones.FanCurves", default_service = "xyz.ljones.Asusd", default_path = "/xyz/ljones")]
trait FanCurves {
    fn fan_curve_data(&self, profile: u32) -> zbus::Result<Vec<RawCurve>>;
    fn set_fan_curve(&self, profile: u32, curve: RawCurve) -> zbus::Result<()>;
    fn set_curves_to_defaults(&self, profile: u32) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "xyz.ljones.AsusArmoury", default_service = "xyz.ljones.Asusd")]
trait Armoury {
    #[zbus(property)]
    fn min_value(&self) -> zbus::Result<i32>;
    #[zbus(property)]
    fn max_value(&self) -> zbus::Result<i32>;
}

pub struct AsusdClient {
    conn: zbus::Connection,
}

impl AsusdClient {
    pub async fn new() -> anyhow::Result<Self> {
        Ok(Self { conn: zbus::Connection::system().await? })
    }

    // Uncached proxies: asusd may be stopped/started by takeover and handback.
    async fn platform(&self) -> zbus::Result<PlatformProxy<'_>> {
        PlatformProxy::builder(&self.conn).cache_properties(zbus::proxy::CacheProperties::No).build().await
    }

    async fn fans(&self) -> zbus::Result<FanCurvesProxy<'_>> {
        FanCurvesProxy::new(&self.conn).await
    }

    async fn armoury(&self, attr: &str) -> anyhow::Result<ArmouryProxy<'_>> {
        anyhow::ensure!(attr.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'), "bad attribute {attr}");
        Ok(ArmouryProxy::builder(&self.conn)
            .path(format!("/xyz/ljones/asus_armoury/{attr}"))?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await?)
    }
}

#[async_trait::async_trait]
impl Asusd for AsusdClient {
    async fn set_profile(&self, p: u32) -> anyhow::Result<()> { Ok(self.platform().await?.set_platform_profile(p).await?) }
    async fn next_profile(&self) -> anyhow::Result<()> { Ok(self.platform().await?.next_platform_profile().await?) }
    async fn fan_curves(&self, profile: u32) -> anyhow::Result<Vec<RawCurve>> { Ok(self.fans().await?.fan_curve_data(profile).await?) }
    async fn set_fan_curve(&self, profile: u32, curve: RawCurve) -> anyhow::Result<()> { Ok(self.fans().await?.set_fan_curve(profile, curve).await?) }
    async fn reset_fan_curves(&self, profile: u32) -> anyhow::Result<()> { Ok(self.fans().await?.set_curves_to_defaults(profile).await?) }
    async fn armoury_range(&self, attr: &str) -> anyhow::Result<(i32, i32)> {
        let a = self.armoury(attr).await?;
        Ok((a.min_value().await?, a.max_value().await?))
    }
    async fn set_profile_epp(&self, profile: u32, epp: u32) -> anyhow::Result<()> {
        let p = self.platform().await?;
        Ok(match profile {
            2 => p.set_profile_quiet_epp(epp).await?,
            0 => p.set_profile_balanced_epp(epp).await?,
            1 => p.set_profile_performance_epp(epp).await?,
            _ => anyhow::bail!("no EPP slot for profile {profile}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Type;

    #[test]
    fn raw_curve_matches_asusd_signature() {
        // FanCurveData on asusd 6.4.0 returns a(s(yyyyyyyy)(yyyyyyyy)b)
        assert_eq!(<RawCurve as Type>::SIGNATURE.to_string(), "(s(yyyyyyyy)(yyyyyyyy)b)");
    }
}
