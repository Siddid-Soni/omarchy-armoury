use armoury_proto::DisplayInfo;
use tokio::process::Command;

/// Hyprland runtime control. This Hyprland takes Lua through `hyprctl eval`
/// (the legacy `hyprctl keyword` is rejected), same calls as ~/.config/hypr/*.lua.
#[async_trait::async_trait]
pub trait Hypr: Send + Sync {
    async fn monitors(&self) -> anyhow::Result<Vec<DisplayInfo>>;
    /// The built-in touchpad's Hyprland device name.
    async fn touchpad(&self) -> Option<String>;
    async fn eval(&self, lua: &str) -> anyhow::Result<()>;
    /// hyprsunset gamma, percent.
    async fn gamma(&self, pct: u8) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
impl<T: Hypr + ?Sized> Hypr for std::sync::Arc<T> {
    async fn monitors(&self) -> anyhow::Result<Vec<DisplayInfo>> { (**self).monitors().await }
    async fn touchpad(&self) -> Option<String> { (**self).touchpad().await }
    async fn eval(&self, lua: &str) -> anyhow::Result<()> { (**self).eval(lua).await }
    async fn gamma(&self, pct: u8) -> anyhow::Result<()> { (**self).gamma(pct).await }
}

pub fn parse_monitors(json: &str) -> anyhow::Result<Vec<DisplayInfo>> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    Ok(v.as_array().into_iter().flatten().filter_map(|m| {
        let (w, h) = (m["width"].as_u64()? as u32, m["height"].as_u64()? as u32);
        let res = format!("{w}x{h}@");
        let rates = m["availableModes"].as_array().into_iter().flatten()
            .filter_map(|s| s.as_str()?.strip_prefix(&res)?.trim_end_matches("Hz").parse::<f32>().ok())
            .collect();
        Some(DisplayInfo {
            output: m["name"].as_str()?.to_string(),
            width: w,
            height: h,
            refresh_hz: m["refreshRate"].as_f64()? as f32,
            rates,
            scale: m["scale"].as_f64().unwrap_or(1.0) as f32,
        })
    }).collect())
}

/// Lua that switches `m` to `hz` at its current resolution and scale; `hz` must be an available rate.
pub fn lua_monitor(m: &DisplayInfo, hz: f32) -> Result<String, String> {
    let rate = m.rates.iter().copied().find(|r| (r - hz).abs() < 0.5).ok_or_else(|| {
        let mut rs: Vec<u32> = m.rates.iter().map(|r| r.round() as u32).collect();
        rs.sort();
        let rs: Vec<String> = rs.iter().map(u32::to_string).collect();
        format!("{hz} Hz is not available on {} (available: {})", m.output, rs.join(", "))
    })?;
    let mode = if (rate - rate.round()).abs() < 0.01 { format!("{}", rate.round()) } else { format!("{rate:.2}") };
    Ok(format!(r#"hl.monitor({{ output = "{}", mode = "{}x{}@{mode}", position = "auto", scale = {} }})"#, m.output, m.width, m.height, m.scale))
}

pub fn lua_touchpad(name: &str, on: bool) -> String {
    format!(r#"hl.device({{ name = "{name}", enabled = {on} }})"#)
}

pub struct RealHypr;

async fn hyprctl(args: &[&str]) -> anyhow::Result<String> {
    let out = tokio::time::timeout(crate::hw::CALL_TIMEOUT, Command::new("hyprctl").args(args).kill_on_drop(true).output())
        .await
        .map_err(|_| anyhow::anyhow!("hyprctl timed out"))??;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || text.starts_with("Couldn't") {
        let detail = if text.is_empty() { String::from_utf8_lossy(&out.stderr).trim().to_string() } else { text };
        anyhow::bail!("hyprctl {}: {detail}", args.join(" "));
    }
    Ok(text)
}

#[async_trait::async_trait]
impl Hypr for RealHypr {
    async fn monitors(&self) -> anyhow::Result<Vec<DisplayInfo>> { parse_monitors(&hyprctl(&["monitors", "-j"]).await?) }

    async fn touchpad(&self) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(&hyprctl(&["devices", "-j"]).await.ok()?).ok()?;
        v["mice"].as_array()?.iter().filter_map(|m| m["name"].as_str()).find(|n| n.ends_with("-touchpad")).map(String::from)
    }

    async fn eval(&self, lua: &str) -> anyhow::Result<()> {
        let out = hyprctl(&["eval", lua]).await?;
        if out != "ok" { anyhow::bail!("Hyprland rejected {lua}: {out}"); }
        Ok(())
    }

    async fn gamma(&self, pct: u8) -> anyhow::Result<()> {
        hyprctl(&["hyprsunset", "gamma", &pct.to_string()]).await
            .map(|_| ())
            .map_err(|_| anyhow::anyhow!("hyprsunset is not running (turn on Omarchy's night light, or start hyprsunset)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // captured from this machine: hyprctl monitors -j
    const MONITORS: &str = r#"[{"name": "eDP-1", "width": 2560, "height": 1440, "refreshRate": 240.00301, "scale": 1.6, "availableModes": ["2560x1440@240.00Hz", "2560x1440@60.00Hz"]}]"#;

    #[test]
    fn parses_monitors() {
        let m = parse_monitors(MONITORS).unwrap();
        assert_eq!(m[0].output, "eDP-1");
        assert_eq!(m[0].rates, vec![240.0, 60.0]);
        assert!((m[0].refresh_hz - 240.0).abs() < 0.01);
        assert_eq!(m[0].scale, 1.6);
    }

    #[test]
    fn lua_for_refresh_and_touchpad() {
        let m = &parse_monitors(MONITORS).unwrap()[0];
        assert_eq!(lua_monitor(m, 60.0).unwrap(), r#"hl.monitor({ output = "eDP-1", mode = "2560x1440@60", position = "auto", scale = 1.6 })"#);
        assert!(lua_monitor(m, 144.0).unwrap_err().contains("60, 240"));
        assert_eq!(lua_touchpad("asue1403:00-04f3:319a-touchpad", false), r#"hl.device({ name = "asue1403:00-04f3:319a-touchpad", enabled = false })"#);
    }
}
