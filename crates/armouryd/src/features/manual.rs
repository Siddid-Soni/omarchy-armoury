//! Manual mode vs the stock firmware modes.
//! Stock: only the thermal-policy change (done by the caller), plus a one-time cleanup when
//! leaving Manual or at start/takeover. Manual: base mode, the profile's curves written into
//! that mode's asusd slot and enabled, then the usual apply_mode for limits and GPU.
use crate::features::fan::{self, RawCurve};
use crate::features::limits::{self, Bounds, Limit};
use crate::features::perf::{apply_mode, HwCtx};
use crate::hw::{with_retry, Asusd, Services};
use armoury_proto::{Fan, FanCurve, ManualProfile, ModeSettings, Profile};

/// A stock mode is the firmware's own: switch_profile has already changed the policy, and
/// nothing else is sent (no curves, EPP or PPT; the user's rule after the EC hang).
/// Only when leaving Manual, or at start/takeover (`cleanup`: the slot whose custom
/// curves may be on), is there a one-time cleanup: that slot's curves off, the policy
/// re-asserted if it didn't change (so the firmware reloads its own PPT), and
/// boost / undervolt / NVIDIA back to stock where they aren't already.
pub async fn apply_stock(p: Profile, cleanup: Option<Profile>, ctx: HwCtx, asusd: &dyn Asusd, svc: &dyn Services) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(slot) = cleanup {
        if let Err(e) = with_retry(|| asusd.set_fan_curves_enabled(slot.to_asusd(), false)).await {
            errors.push(format!("fan curves off: {e:#}"));
        }
        if slot == p && !ctx.mode_just_set {
            if let Err(e) = with_retry(|| asusd.set_profile(p.to_asusd())).await {
                errors.push(format!("re-assert mode: {e:#}"));
            }
        }
    }
    // no limits → apply_mode only restores boost (skipped when already on) and resets
    // undervolt / NVIDIA if they were changed; it never touches asusd.
    errors.extend(apply_mode(p, &ModeSettings::default(), ctx, asusd, svc).await);
    errors
}

pub async fn apply_manual(p: &ManualProfile, ctx: HwCtx, asusd: &dyn Asusd, svc: &dyn Services) -> Vec<String> {
    let base = p.base.to_asusd();
    if let Err(e) = with_retry(|| asusd.set_profile(base)).await {
        return vec![format!("set base mode: {e:#}")];
    }
    let mut errors = Vec::new();
    for c in &p.curves {
        let raw = fan::to_raw(&FanCurve { enabled: true, ..c.clone() });
        if let Err(e) = with_retry(|| asusd.set_fan_curve(base, raw.clone())).await {
            errors.push(format!("{} fan curve: {e:#}", c.fan.asusd_name()));
        }
    }
    if let Err(e) = with_retry(|| asusd.set_fan_curves_enabled(base, true)).await {
        errors.push(format!("enable fan curves: {e:#}"));
    }
    // the base policy was just set and its curves enabled: apply_mode must not repeat either
    let ctx = HwCtx { mode_just_set: true, curves_on: true, ..ctx };
    errors.extend(apply_mode(p.base, &p.settings, ctx, asusd, svc).await);
    errors
}

/// Used when asusd can't report curves.
pub fn fallback_curves() -> Vec<FanCurve> {
    [Fan::Cpu, Fan::Gpu].into_iter()
        .map(|fan| FanCurve { fan, temps: [30, 40, 50, 60, 70, 80, 90, 100], percent: [0, 10, 20, 35, 55, 75, 90, 100], enabled: true })
        .collect()
}

/// A new profile on Turbo with the given asusd curves (or the fallback) and no tuning.
pub fn default_profile(name: &str, raw: &[RawCurve]) -> ManualProfile {
    let curves: Vec<FanCurve> = raw.iter().filter_map(fan::from_raw).collect();
    ManualProfile {
        name: name.to_string(),
        base: Profile::Performance,
        curves: if curves.is_empty() { fallback_curves() } else { curves },
        settings: ModeSettings::default(),
    }
}

pub fn validate_profile(p: &ManualProfile, b: impl Fn(Limit) -> Bounds) -> Result<(), String> {
    for (i, c) in p.curves.iter().enumerate() {
        fan::validate(c).map_err(|e| format!("{} fan: {e}", c.fan.asusd_name()))?;
        if p.curves[..i].iter().any(|d| d.fan == c.fan) { return Err(format!("{} fan curve given twice", c.fan.asusd_name())); }
    }
    limits::validate(&p.settings, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::fake::{FakeAsusd, FakeServices};
    use armoury_proto::{Fan, FanCurve, ModeSettings};

    fn curve(fan: Fan) -> FanCurve {
        FanCurve { fan, temps: [30, 40, 50, 60, 70, 80, 90, 100], percent: [0, 10, 20, 35, 55, 75, 90, 100], enabled: false }
    }

    #[tokio::test]
    async fn manual_sets_base_then_curves_then_limits() {
        let (asusd, svc) = (FakeAsusd::default(), FakeServices::default());
        let p = ManualProfile { name: "G".into(), base: Profile::Performance, curves: vec![curve(Fan::Cpu), curve(Fan::Gpu)],
            settings: ModeSettings { pl1: Some(20), pl2: Some(40), ..Default::default() } };
        let errs = apply_manual(&p, HwCtx::default(), &asusd, &svc).await;
        assert!(errs.is_empty(), "{errs:?}");
        let calls = asusd.calls.lock().unwrap().clone();
        assert_eq!(&calls[..4], ["set_profile 1", "set_fan_curve 1 CPU", "set_fan_curve 1 GPU", "set_fan_curves_enabled 1 true"], "{calls:?}");
        // curves are written enabled even though the profile stores enabled=false
        assert!(asusd.curves.lock().unwrap()[&1].iter().all(|c| c.3));
        assert!(svc.calls.lock().unwrap().iter().any(|c| c.ends_with("set-limits pl1=20 pl2=40 cpu_boost=on")));
        // minimal EC traffic: one policy change and one curves-enable per apply
        assert_eq!(calls.iter().filter(|c| c.starts_with("set_profile")).count(), 1, "{calls:?}");
        assert_eq!(calls.iter().filter(|c| c.starts_with("set_fan_curves_enabled")).count(), 1, "{calls:?}");
    }

    #[tokio::test]
    async fn stock_to_stock_sends_nothing_extra() {
        let (asusd, svc) = (FakeAsusd::default(), FakeServices::default());
        let ctx = HwCtx { cpu_boost_now: Some(true), mode_just_set: true, uv_at_stock: true, nv_at_stock: true, ..Default::default() };
        let errs = apply_stock(Profile::Quiet, None, ctx, &asusd, &svc).await;
        assert!(errs.is_empty(), "{errs:?}");
        assert!(asusd.calls.lock().unwrap().is_empty(), "no curves / EPP / policy re-assert");
        assert!(svc.calls.lock().unwrap().is_empty(), "no root call when boost is already on");
    }

    #[tokio::test]
    async fn leaving_manual_cleans_up_once() {
        let (asusd, svc) = (FakeAsusd::default(), FakeServices::default());
        // Manual ran on Turbo; the user picks Turbo (policy unchanged): curves off + re-assert
        let ctx = HwCtx { cpu_boost_now: Some(false), ..Default::default() };
        apply_stock(Profile::Performance, Some(Profile::Performance), ctx, &asusd, &svc).await;
        assert_eq!(*asusd.calls.lock().unwrap(), ["set_fan_curves_enabled 1 false", "set_profile 1"]);
        assert!(svc.calls.lock().unwrap().iter().any(|c| c.ends_with("set-limits cpu_boost=on")), "boost was off");
        // Manual on Turbo → Silent: the policy already changed, so only the old slot's curves go off
        let (asusd, svc) = (FakeAsusd::default(), FakeServices::default());
        let ctx = HwCtx { cpu_boost_now: Some(true), mode_just_set: true, ..Default::default() };
        apply_stock(Profile::Quiet, Some(Profile::Performance), ctx, &asusd, &svc).await;
        assert_eq!(*asusd.calls.lock().unwrap(), ["set_fan_curves_enabled 1 false"]);
    }

    #[tokio::test]
    async fn manual_stops_when_base_cannot_be_set() {
        let (asusd, svc) = (FakeAsusd::default(), FakeServices::default());
        *asusd.fail_on.lock().unwrap() = Some("set_profile".into());
        let p = ManualProfile { name: "G".into(), base: Profile::Quiet, curves: vec![curve(Fan::Cpu)], settings: ModeSettings::default() };
        let errs = apply_manual(&p, HwCtx::default(), &asusd, &svc).await;
        assert!(errs[0].contains("base mode"), "{errs:?}");
        assert!(!asusd.calls.lock().unwrap().iter().any(|c| c.starts_with("set_fan_curve")), "no curves on the wrong mode");
    }

    #[test]
    fn default_profile_uses_asusd_curves_or_fallback() {
        let raw: Vec<crate::features::fan::RawCurve> = vec![("CPU".into(), [0, 25, 51, 76, 102, 153, 204, 255], [30, 40, 50, 60, 70, 80, 90, 100], true)];
        let p = default_profile("Manual 1", &raw);
        assert_eq!((p.name.as_str(), p.base, p.curves.len()), ("Manual 1", Profile::Performance, 1));
        assert_eq!(default_profile("X", &[]).curves, fallback_curves());
        assert_eq!(fallback_curves().iter().map(|c| c.fan).collect::<Vec<_>>(), [Fan::Cpu, Fan::Gpu]);
    }

    #[test]
    fn profile_validation() {
        let b = |_| crate::features::limits::Bounds { min: 5, max: 150 };
        let mut p = default_profile("A", &[]);
        assert!(validate_profile(&p, b).is_ok());
        p.curves[0].percent[3] = 0; // decreasing
        assert!(validate_profile(&p, b).is_err());
        let mut q = default_profile("A", &[]);
        q.curves.push(q.curves[0].clone());
        assert!(validate_profile(&q, b).unwrap_err().contains("twice"));
    }
}
