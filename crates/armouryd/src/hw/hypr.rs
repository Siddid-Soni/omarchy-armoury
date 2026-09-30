use armoury_proto::DisplayInfo;
use tokio::process::Command;

/// Hyprland runtime control. This Hyprland takes Lua through `hyprctl eval`
/// (the legacy `hyprctl keyword` is rejected), same calls as ~/.config/hypr/*.lua.
#[async_trait::async_trait]
pub trait Hypr: Send + Sync {
    async fn monitors(&self) -> anyhow::Result<Vec<DisplayInfo>>;
    async fn eval(&self, lua: &str) -> anyhow::Result<()>;
    /// hyprsunset gamma, percent.
    async fn gamma(&self, pct: u8) -> anyhow::Result<()>;
    /// NumLock state (seat-wide; true if any keyboard reports it on).
    async fn numlock(&self) -> anyhow::Result<bool>;
    /// Keyboard repeats per second (`input:repeat_rate`).
    async fn repeat_rate(&self) -> anyhow::Result<u32>;
    /// Keyboard delay before repeating, ms (`input:repeat_delay`).
    async fn repeat_delay(&self) -> anyhow::Result<u32>;
}

#[async_trait::async_trait]
impl<T: Hypr + ?Sized> Hypr for std::sync::Arc<T> {
    async fn monitors(&self) -> anyhow::Result<Vec<DisplayInfo>> { (**self).monitors().await }
    async fn eval(&self, lua: &str) -> anyhow::Result<()> { (**self).eval(lua).await }
    async fn gamma(&self, pct: u8) -> anyhow::Result<()> { (**self).gamma(pct).await }
    async fn numlock(&self) -> anyhow::Result<bool> { (**self).numlock().await }
    async fn repeat_rate(&self) -> anyhow::Result<u32> { (**self).repeat_rate().await }
    async fn repeat_delay(&self) -> anyhow::Result<u32> { (**self).repeat_delay().await }
}

pub fn parse_repeat_rate(json: &str) -> anyhow::Result<u32> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    v["int"].as_u64().map(|r| r as u32).ok_or_else(|| anyhow::anyhow!("no input:repeat_rate in {json}"))
}

pub fn parse_numlock(json: &str) -> anyhow::Result<bool> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    Ok(v["keyboards"].as_array().into_iter().flatten().any(|k| k["numLock"].as_bool() == Some(true)))
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
            x: m["x"].as_i64().unwrap_or(0) as i32,
            y: m["y"].as_i64().unwrap_or(0) as i32,
            transform: m["transform"].as_u64().unwrap_or(0) as u32,
        })
    }).collect())
}

/// Lua that switches `m` to `hz`, keeping its resolution, scale, position and transform;
/// `hz` must be an available rate. Output names go into Lua, so only Omarchy's safe set is allowed.
pub fn lua_monitor(m: &DisplayInfo, hz: f32) -> Result<String, String> {
    if m.output.is_empty() || !m.output.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) {
        return Err(format!("refusing unexpected output name {:?}", m.output));
    }
    let rate = m.rates.iter().copied().find(|r| (r - hz).abs() < 0.5).ok_or_else(|| {
        let mut rs: Vec<u32> = m.rates.iter().map(|r| r.round() as u32).collect();
        rs.sort();
        let rs: Vec<String> = rs.iter().map(u32::to_string).collect();
        format!("{hz} Hz is not available on {} (available: {})", m.output, rs.join(", "))
    })?;
    let mode = if (rate - rate.round()).abs() < 0.01 { format!("{}", rate.round()) } else { format!("{rate:.2}") };
    let scale = format!("{:.6}", m.scale).trim_end_matches('0').trim_end_matches('.').to_string();
    Ok(format!(
        r#"hl.monitor({{ output = "{}", mode = "{}x{}@{mode}", position = "{}x{}", scale = {scale}, transform = {} }})"#,
        m.output, m.width, m.height, m.x, m.y, m.transform
    ))
}

/// Newest running Hyprland instance from `hyprctl instances -j`.
pub fn pick_instance(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.as_array()?.iter().max_by_key(|i| i["time"].as_u64().unwrap_or(0))?["instance"].as_str().map(String::from)
}

pub struct RealHypr;

/// Runs hyprctl against the live Hyprland. armouryd starts before the session exports
/// HYPRLAND_INSTANCE_SIGNATURE and outlives Hyprland restarts, so the instance is looked
/// up on every call (`hyprctl instances` works without it).
async fn hyprctl(args: &[&str]) -> anyhow::Result<String> {
    let mut cmd = Command::new("hyprctl");
    let instances = tokio::time::timeout(crate::hw::CALL_TIMEOUT, Command::new("hyprctl").args(["instances", "-j"]).kill_on_drop(true).output()).await;
    if let Ok(Ok(o)) = instances {
        if let Some(sig) = pick_instance(&String::from_utf8_lossy(&o.stdout)) { cmd.env("HYPRLAND_INSTANCE_SIGNATURE", sig); }
    }
    let out = tokio::time::timeout(crate::hw::CALL_TIMEOUT, cmd.args(args).kill_on_drop(true).output())
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
    async fn numlock(&self) -> anyhow::Result<bool> { parse_numlock(&hyprctl(&["devices", "-j"]).await?) }
    async fn repeat_rate(&self) -> anyhow::Result<u32> { parse_repeat_rate(&hyprctl(&["getoption", "input:repeat_rate", "-j"]).await?) }
    async fn repeat_delay(&self) -> anyhow::Result<u32> { parse_repeat_rate(&hyprctl(&["getoption", "input:repeat_delay", "-j"]).await?) }


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

    #[test]
    fn repeat_rate_from_getoption_json() {
        assert_eq!(parse_repeat_rate(r#"{"option":"input:repeat_rate","int":40,"set":true}"#).unwrap(), 40);
        assert_eq!(parse_repeat_rate(r#"{"option":"input:repeat_delay","int":300,"set":true}"#).unwrap(), 300, "same shape for the delay");
        assert!(parse_repeat_rate(r#"{"option":"x"}"#).is_err());
    }

    #[test]
    fn numlock_from_devices_json() {
        assert!(parse_numlock(r#"{"keyboards":[{"name":"a","numLock":false},{"name":"b","numLock":true}]}"#).unwrap());
        assert!(!parse_numlock(r#"{"keyboards":[{"name":"a","numLock":false}]}"#).unwrap());
    }

    // captured from this machine: hyprctl monitors -j
    const MONITORS: &str = r#"[{"name": "eDP-1", "width": 2560, "height": 1440, "refreshRate": 240.00301, "scale": 1.6, "x": 0, "y": 0, "transform": 0, "availableModes": ["2560x1440@240.00Hz", "2560x1440@60.00Hz"]}]"#;

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
        assert_eq!(lua_monitor(m, 60.0).unwrap(), r#"hl.monitor({ output = "eDP-1", mode = "2560x1440@60", position = "0x0", scale = 1.6, transform = 0 })"#);
        assert!(lua_monitor(m, 144.0).unwrap_err().contains("60, 240"));
        let mut evil = m.clone();
        evil.output = "eDP-1\"}) os.execute(\"id\") --".into();
        assert!(lua_monitor(&evil, 60.0).unwrap_err().contains("name"));
    }

    #[test]
    fn picks_newest_instance() {
        let j = r#"[{"instance":"old_1","time":100,"pid":1,"wl_socket":"wayland-0"},{"instance":"new_2","time":200,"pid":2,"wl_socket":"wayland-1"}]"#;
        assert_eq!(pick_instance(j).as_deref(), Some("new_2"));
        assert_eq!(pick_instance("[]"), None);
    }
}
