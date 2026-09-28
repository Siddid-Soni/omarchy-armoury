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
    if let Some(epp) = s.epp {
        if let Err(e) = with_retry(|| asusd.set_profile_epp(profile.to_asusd(), epp.to_asusd())).await { errors.push(format!("EPP: {e:#}")); }
    }
    let mut args: Vec<String> = Limit::ALL.iter().filter_map(|l| Some(format!("{}={}", l.key(), l.value(s)?))).collect();
    if let Some(on) = s.cpu_boost {
        args.push(format!("cpu_boost={}", if on { "on" } else { "off" }));
    }
    if !args.is_empty() {
        let mut argv = vec![PKEXEC, ROOT_HELPER, "set-limits"];
        argv.extend(args.iter().map(String::as_str));
        if let Err(e) = svc.run(&argv).await { errors.push(format!("power limits: {e:#}")); }
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
        assert_eq!(*a.calls.lock().unwrap(), ["set_profile_epp 1 1"]);
        assert_eq!(*s.calls.lock().unwrap(), [
            format!("{PKEXEC} {ROOT_HELPER} set-limits pl1=120 pl2=150 nv_boost=25 nv_temp=87 cpu_boost=off"),
        ]);
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
        *s.fail_on.lock().unwrap() = Some("set-limits".into());
        let errs = apply_mode(Profile::Performance, &full(), &a, &s).await;
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("power limits"), "{errs:?}");
        assert_eq!(*a.calls.lock().unwrap(), ["set_profile_epp 1 1"], "EPP still applied");
    }
}
