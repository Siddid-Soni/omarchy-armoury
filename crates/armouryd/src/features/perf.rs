use crate::control::{PKEXEC, ROOT_HELPER};
use crate::features::limits::Limit;
use crate::hw::{Asusd, Services, with_retry};
use armoury_proto::{ModeSettings, Profile};

/// Applies every setting that is Some; keeps going after failures and returns their messages.
/// Power limits and CPU boost go through one root call to the asus-nb-wmi nodes:
/// the asus-armoury firmware-attributes driver asusd uses cannot read or write
/// PPT on every model (ENODEV on the G533ZW).
pub async fn apply_mode(profile: Profile, s: &ModeSettings, asusd: &dyn Asusd, svc: &dyn Services) -> Vec<String> {
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
    errors
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
        assert!(apply_mode(Profile::Performance, &full(), &a, &s).await.is_empty());
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
        assert!(apply_mode(Profile::Quiet, &ModeSettings::default(), &a, &s).await.is_empty());
        assert!(a.calls.lock().unwrap().is_empty());
        assert_eq!(*s.calls.lock().unwrap(), [format!("{PKEXEC} {ROOT_HELPER} set-limits cpu_boost=on")]);
    }

    #[tokio::test]
    async fn apply_continues_after_error() {
        let (a, s) = (FakeAsusd::default(), FakeServices::default());
        *s.fail_on.lock().unwrap() = Some("set-limits".into());
        let errs = apply_mode(Profile::Performance, &full(), &a, &s).await;
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("power limits"), "{errs:?}");
        assert_eq!(*a.calls.lock().unwrap(), ["set_profile 1", "set_profile_epp 1 1", "set_fan_curves_enabled 1 true"], "EPP still applied");
    }
}
