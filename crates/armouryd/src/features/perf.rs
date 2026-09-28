use crate::control::{PKEXEC, ROOT_HELPER};
use crate::features::limits::Limit;
use crate::hw::{Asusd, Services, with_retry};
use armoury_proto::{ModeSettings, Profile};

/// Applies every setting that is Some; keeps going after failures and returns their messages.
/// Power limits and CPU boost go through one root call to the asus-nb-wmi nodes:
/// the asus-armoury firmware-attributes driver asusd uses cannot read or write
/// PPT on every model (ENODEV on the G533ZW).
/// State of hardware that decides whether undervolt / NVIDIA settings can be sent.
#[derive(Debug, Clone, Copy, Default)]
pub struct HwCtx {
    /// The OC mailbox accepted a probe write (BIOS leaves undervolt unlocked).
    pub uv_unlocked: bool,
    /// The dGPU is awake; NVIDIA settings are never sent to a suspended dGPU.
    pub dgpu_active: bool,
}

pub async fn apply_mode(profile: Profile, s: &ModeSettings, ctx: HwCtx, asusd: &dyn Asusd, svc: &dyn Services) -> Vec<String> {
    let mut errors = Vec::new();
    let has_limits = Limit::ALL.iter().any(|l| l.value(s).is_some());
    if has_limits {
        // The firmware only honours PPT written right after a thermal-policy change
        // (measured on the G533ZW: a 15 W cap written mid-mode is ignored, the same
        // write after a mode switch holds exactly). Re-assert the mode first, as g-helper does.
        if let Err(e) = with_retry(|| asusd.set_profile(profile.to_asusd())).await {
            errors.push(format!("re-assert mode: {e:#}"));
        }
        tokio::time::sleep(POLICY_SETTLE).await;
    }
    if let Some(epp) = s.epp {
        if let Err(e) = with_retry(|| asusd.set_profile_epp(profile.to_asusd(), epp.to_asusd())).await { errors.push(format!("EPP: {e:#}")); }
    }
    if has_limits {
        // Some ASUS firmware also ignores PPT unless the EC is in manual fan mode,
        // i.e. the mode's custom fan curves are on (g-helper and asusd both require it).
        if let Err(e) = with_retry(|| asusd.set_fan_curves_enabled(profile.to_asusd(), true)).await {
            errors.push(format!("enable fan curves: {e:#}"));
        }
    }
    let mut args: Vec<String> = Limit::ALL.iter().filter_map(|l| Some(format!("{}={}", l.key(), l.value(s)?))).collect();
    // no_turbo is kernel state no firmware reset touches: a mode without its own
    // setting gets the default (on) instead of inheriting the previous mode's value.
    args.push(format!("cpu_boost={}", if s.cpu_boost.unwrap_or(true) { "on" } else { "off" }));
    let mut argv = vec![PKEXEC, ROOT_HELPER, "set-limits"];
    argv.extend(args.iter().map(String::as_str));
    if let Err(e) = svc.run(&argv).await { errors.push(format!("power limits: {e:#}")); }
    // MSR offsets and NVIDIA clocks persist across modes, so unset means stock (0 / off).
    if ctx.uv_unlocked {
        let mv = s.uv_mv.unwrap_or(0).to_string();
        if let Err(e) = svc.run(&[PKEXEC, ROOT_HELPER, "undervolt", "set", &mv]).await { errors.push(format!("undervolt: {e:#}")); }
    }
    if ctx.dgpu_active {
        errors.extend(apply_nv(s, svc).await);
    }
    errors
}

/// NVIDIA offsets/locks for a mode (unset = stock). Only call while the dGPU is awake.
pub async fn apply_nv(s: &ModeSettings, svc: &dyn Services) -> Vec<String> {
    let lock = |v: Option<u32>| v.filter(|&m| m > 0).map_or("off".to_string(), |m| m.to_string());
    let args = [s.gpu_core_offset.unwrap_or(0).to_string(), s.gpu_mem_offset.unwrap_or(0).to_string(), lock(s.gpu_core_lock), lock(s.gpu_mem_lock)];
    let mut argv = vec![PKEXEC, ROOT_HELPER, "nv-clocks"];
    argv.extend(args.iter().map(String::as_str));
    match svc.run(&argv).await {
        Ok(()) => Vec::new(),
        Err(e) => vec![format!("NVIDIA clocks: {e:#}")],
    }
}

/// Time for the EC to settle after a thermal-policy write (g-helper waits 100 ms).
const POLICY_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::fake::{FakeAsusd, FakeServices};
    use armoury_proto::Epp;

    fn full() -> ModeSettings {
        ModeSettings { pl1: Some(120), pl2: Some(150), nv_boost: Some(25), nv_temp: Some(87), epp: Some(Epp::Performance), cpu_boost: Some(false), ..Default::default() }
    }

    #[tokio::test]
    async fn applies_everything_in_order() {
        let (a, s) = (FakeAsusd::default(), FakeServices::default());
        assert!(apply_mode(Profile::Performance, &full(), HwCtx::default(), &a, &s).await.is_empty());
        // firmware only honours PPT written right after a thermal-policy change (measured:
        // 15 W cap ignored mid-mode, exact after a mode switch), so the mode is re-asserted first
        assert_eq!(*a.calls.lock().unwrap(), ["set_profile 1", "set_profile_epp 1 1", "set_fan_curves_enabled 1 true"]);
        assert_eq!(*s.calls.lock().unwrap(), [
            format!("{PKEXEC} {ROOT_HELPER} set-limits pl1=120 pl2=150 nv_boost=25 nv_temp=87 cpu_boost=off"),
        ]);
    }

    #[tokio::test]
    async fn unset_boost_restores_default_on() {
        // no_turbo is kernel state the firmware never resets, so a mode without
        // its own setting must not inherit the previous mode's "off"
        let (a, s) = (FakeAsusd::default(), FakeServices::default());
        assert!(apply_mode(Profile::Quiet, &ModeSettings::default(), HwCtx::default(), &a, &s).await.is_empty());
        assert!(a.calls.lock().unwrap().is_empty());
        assert_eq!(*s.calls.lock().unwrap(), [format!("{PKEXEC} {ROOT_HELPER} set-limits cpu_boost=on")]);
    }

    #[tokio::test]
    async fn apply_continues_after_error() {
        let (a, s) = (FakeAsusd::default(), FakeServices::default());
        *s.fail_on.lock().unwrap() = Some("set-limits".into());
        let errs = apply_mode(Profile::Performance, &full(), HwCtx::default(), &a, &s).await;
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("power limits"), "{errs:?}");
        assert_eq!(*a.calls.lock().unwrap(), ["set_profile 1", "set_profile_epp 1 1", "set_fan_curves_enabled 1 true"], "EPP still applied");
    }

    fn ctx(uv: bool, gpu: bool) -> HwCtx { HwCtx { uv_unlocked: uv, dgpu_active: gpu } }

    #[tokio::test]
    async fn uv_skipped_when_locked() {
        let (a, s) = (FakeAsusd::default(), FakeServices::default());
        let m = ModeSettings { uv_mv: Some(-40), ..Default::default() };
        apply_mode(Profile::Quiet, &m, ctx(false, false), &a, &s).await;
        assert!(!s.calls.lock().unwrap().iter().any(|c| c.contains("undervolt")));
        apply_mode(Profile::Quiet, &m, ctx(true, false), &a, &s).await;
        assert!(s.calls.lock().unwrap().iter().any(|c| c.ends_with("undervolt set -40")));
    }

    #[tokio::test]
    async fn nv_deferred_until_dgpu_wakes() {
        let (a, s) = (FakeAsusd::default(), FakeServices::default());
        let m = ModeSettings { gpu_core_offset: Some(100), gpu_core_lock: Some(1500), ..Default::default() };
        apply_mode(Profile::Performance, &m, ctx(false, false), &a, &s).await;
        assert!(!s.calls.lock().unwrap().iter().any(|c| c.contains("nv-clocks")));
        apply_mode(Profile::Performance, &m, ctx(false, true), &a, &s).await;
        assert!(s.calls.lock().unwrap().iter().any(|c| c.ends_with("nv-clocks 100 0 1500 off")));
    }

    #[tokio::test]
    async fn unset_values_reset_to_stock() {
        let (a, s) = (FakeAsusd::default(), FakeServices::default());
        apply_mode(Profile::Quiet, &ModeSettings::default(), ctx(true, true), &a, &s).await;
        let calls = s.calls.lock().unwrap().clone();
        assert!(calls.iter().any(|c| c.ends_with("undervolt set 0")));
        assert!(calls.iter().any(|c| c.ends_with("nv-clocks 0 0 off off")));
    }
}
