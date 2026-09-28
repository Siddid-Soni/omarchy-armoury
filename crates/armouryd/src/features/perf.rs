use crate::control::{PKEXEC, ROOT_HELPER};
use crate::features::limits::Limit;
use crate::hw::{Asusd, Services, with_retry};
use armoury_proto::{ModeSettings, Profile};

/// Applies every setting that is Some; keeps going after failures and returns their messages.
pub async fn apply_mode(profile: Profile, s: &ModeSettings, asusd: &dyn Asusd, svc: &dyn Services) -> Vec<String> {
    let mut errors = Vec::new();
    if Limit::ALL.iter().any(|l| l.value(s).is_some()) {
        // asusd only writes PPT values while the mode's tuning group is enabled,
        // and refuses to enable tuning unless the mode's custom fan curves are on.
        if let Err(e) = with_retry(|| asusd.set_fan_curves_enabled(profile.to_asusd(), true)).await {
            errors.push(format!("enable fan curves: {e:#}"));
        }
        if let Err(e) = with_retry(|| asusd.set_ppt_group(true)).await { errors.push(format!("enable tuning: {e:#}")); }
        for l in Limit::ALL {
            if let Some(v) = l.value(s) {
                if let Err(e) = with_retry(|| asusd.armoury_set(l.attr(), v)).await { errors.push(format!("{}: {e:#}", l.label())); }
            }
        }
    }
    if let Some(epp) = s.epp {
        if let Err(e) = with_retry(|| asusd.set_profile_epp(profile.to_asusd(), epp.to_asusd())).await { errors.push(format!("EPP: {e:#}")); }
    }
    if let Some(on) = s.cpu_boost {
        let arg = if on { "on" } else { "off" };
        if let Err(e) = svc.run(&[PKEXEC, ROOT_HELPER, "cpu-boost", arg]).await { errors.push(format!("CPU boost: {e:#}")); }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::fake::{FakeAsusd, FakeServices};
    use armoury_proto::Epp;

    fn full() -> ModeSettings {
        ModeSettings { pl1: Some(120), pl2: Some(150), nv_boost: Some(25), nv_temp: Some(87), epp: Some(Epp::Performance), cpu_boost: Some(false) }
    }

    #[tokio::test]
    async fn applies_everything_in_order() {
        let (a, s) = (FakeAsusd::default(), FakeServices::default());
        assert!(apply_mode(Profile::Performance, &full(), &a, &s).await.is_empty());
        assert_eq!(*a.calls.lock().unwrap(), [
            "set_fan_curves_enabled 1 true",
            "set_ppt_group true",
            "armoury_set ppt_pl1_spl 120",
            "armoury_set ppt_pl2_sppt 150",
            "armoury_set nv_dynamic_boost 25",
            "armoury_set nv_temp_target 87",
            "set_profile_epp 1 1",
        ]);
        assert_eq!(*s.calls.lock().unwrap(), [format!("{PKEXEC} {ROOT_HELPER} cpu-boost off")]);
    }

    #[tokio::test]
    async fn empty_settings_touch_nothing() {
        let (a, s) = (FakeAsusd::default(), FakeServices::default());
        assert!(apply_mode(Profile::Quiet, &ModeSettings::default(), &a, &s).await.is_empty());
        assert!(a.calls.lock().unwrap().is_empty() && s.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn apply_continues_after_error() {
        let (a, s) = (FakeAsusd::default(), FakeServices::default());
        *a.fail_on.lock().unwrap() = Some("ppt_pl1_spl".into());
        let errs = apply_mode(Profile::Performance, &full(), &a, &s).await;
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("PL1"), "{errs:?}");
        assert!(a.calls.lock().unwrap().iter().any(|c| c.contains("nv_temp_target")));
        assert_eq!(s.calls.lock().unwrap().len(), 1);
    }
}
