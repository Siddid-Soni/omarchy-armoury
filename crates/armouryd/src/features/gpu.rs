use armoury_proto::{GpuMode, GpuState, GpuStep};

/// The spec's GPU matrix. Never returns a step that can leave gpu_mux_mode=0 with
/// dgpu_disable=1 (black screen at boot).
pub fn plan_switch(g: &GpuState, target: GpuMode) -> Result<GpuStep, String> {
    use GpuMode::*;
    let current = g.mode.ok_or("supergfxd is not answering; try again")?;
    if g.supported.is_empty() { return Err("supergfxd is not answering; try again".into()); }
    // Anything that might already be queued for the next boot blocks a new switch.
    if let Some(p) = g.pending { return Err(format!("a switch to {p:?} is pending; reboot first")); }
    if g.pending_unknown { return Err("cannot tell whether a GPU switch is pending (supergfxd did not answer); try again".into()); }
    if g.pending_reboot == Some(true) { return Err("the firmware has a GPU change waiting; reboot first".into()); }
    if let Some(c) = g.conf_mode.filter(|c| *c != current) {
        return Err(format!("/etc/supergfxd.conf already says {c:?} (a switch waiting for reboot); reboot first"));
    }
    if g.toggle_running { return Err("Omarchy's GPU toggle is open; finish or close it first".into()); }
    if let Some(p) = g.armoury_pending { return Err(format!("a switch to {p:?} was made this boot; reboot first")); }
    if !matches!(target, Integrated | Hybrid | AsusMuxDgpu) || !g.supported.contains(&target) {
        return Err(format!("{target:?} is not supported on this machine"));
    }
    if target == current { return Err(format!("already in {target:?}")); }
    // Unknown hardware state is never treated as safe.
    let mux_igpu = g.mux == Some(1);
    let dgpu_on = g.dgpu_disable == Some(0);
    match (current, target) {
        (Hybrid, Integrated) if !mux_igpu => Err("the MUX is not confirmed on the iGPU; cannot switch to Integrated".into()),
        (Hybrid, Integrated) | (Integrated, Hybrid) => Ok(GpuStep::OmarchyToggle { to: target }),
        (Hybrid, AsusMuxDgpu) if !dgpu_on => Err("the dGPU is not confirmed enabled; cannot switch to Ultimate".into()),
        (Hybrid, AsusMuxDgpu) | (AsusMuxDgpu, Hybrid) => Ok(GpuStep::Supergfx { to: target }),
        (Integrated, AsusMuxDgpu) => Ok(GpuStep::FirstOfTwo { first: Box::new(GpuStep::OmarchyToggle { to: Hybrid }), then: AsusMuxDgpu }),
        (AsusMuxDgpu, Integrated) => Ok(GpuStep::FirstOfTwo { first: Box::new(GpuStep::Supergfx { to: Hybrid }), then: Integrated }),
        _ => Err(format!("no path from {current:?} to {target:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(mode: GpuMode, mux: u8, off: u8) -> GpuState {
        GpuState { mode: Some(mode), supported: vec![GpuMode::Integrated, GpuMode::Hybrid, GpuMode::Vfio, GpuMode::AsusMuxDgpu], mux: Some(mux), dgpu_disable: Some(off), ..Default::default() }
    }

    #[test]
    fn matrix() {
        use GpuMode::*;
        assert_eq!(plan_switch(&st(Hybrid, 1, 0), Integrated), Ok(GpuStep::OmarchyToggle { to: Integrated }));
        assert_eq!(plan_switch(&st(Integrated, 1, 1), Hybrid), Ok(GpuStep::OmarchyToggle { to: Hybrid }));
        assert_eq!(plan_switch(&st(Hybrid, 1, 0), AsusMuxDgpu), Ok(GpuStep::Supergfx { to: AsusMuxDgpu }));
        assert_eq!(plan_switch(&st(AsusMuxDgpu, 0, 0), Hybrid), Ok(GpuStep::Supergfx { to: Hybrid }));
        assert_eq!(plan_switch(&st(Integrated, 1, 1), AsusMuxDgpu),
            Ok(GpuStep::FirstOfTwo { first: Box::new(GpuStep::OmarchyToggle { to: Hybrid }), then: AsusMuxDgpu }));
        assert_eq!(plan_switch(&st(AsusMuxDgpu, 0, 0), Integrated),
            Ok(GpuStep::FirstOfTwo { first: Box::new(GpuStep::Supergfx { to: Hybrid }), then: Integrated }));
        assert!(plan_switch(&st(Hybrid, 1, 0), Hybrid).unwrap_err().contains("already"));
    }

    #[test]
    fn pending_blocks_switch() {
        let mut g = st(GpuMode::Hybrid, 1, 0);
        g.pending = Some(GpuMode::AsusMuxDgpu);
        assert!(plan_switch(&g, GpuMode::Integrated).unwrap_err().contains("AsusMuxDgpu"));
    }

    #[test]
    fn unsupported_target_refused() {
        let mut g = st(GpuMode::Hybrid, 1, 0);
        g.supported = vec![GpuMode::Integrated, GpuMode::Hybrid];
        assert!(plan_switch(&g, GpuMode::AsusMuxDgpu).unwrap_err().contains("not supported"));
        assert!(plan_switch(&g, GpuMode::Vfio).is_err());
    }

    #[test]
    fn unknown_mode_refused() {
        let g = GpuState::default();
        assert!(plan_switch(&g, GpuMode::Hybrid).unwrap_err().contains("not answering"));
    }

    #[test]
    fn no_step_reaches_black_screen() {
        use GpuMode::*;
        for mode in [Integrated, Hybrid, AsusMuxDgpu] {
            for mux in [Option::None, Some(0u8), Some(1)] {
                for off in [Option::None, Some(0u8), Some(1)] {
                    for target in [Integrated, Hybrid, AsusMuxDgpu] {
                        let mut g = st(mode, 1, 0);
                        g.mux = mux;
                        g.dgpu_disable = off;
                        let Ok(step) = plan_switch(&g, target) else { continue };
                        let first = match &step { GpuStep::FirstOfTwo { first, .. } => (**first).clone(), s => s.clone() };
                        // the only direct write that sets mux=0 must start from the dGPU enabled
                        if first == (GpuStep::Supergfx { to: AsusMuxDgpu }) { assert_eq!(off, Some(0), "{mode:?} mux={mux:?} off={off:?} -> {target:?}"); }
                        // Integrated (dgpu_disable=1) must never be entered while mux=0
                        if first == (GpuStep::OmarchyToggle { to: Integrated }) { assert_eq!(mux, Some(1), "{mode:?} mux={mux:?} off={off:?} -> {target:?}"); }
                    }
                }
            }
        }
    }

    fn blocked(f: impl FnOnce(&mut GpuState), needle: &str) {
        let mut g = st(GpuMode::Hybrid, 1, 0);
        f(&mut g);
        let e = plan_switch(&g, GpuMode::AsusMuxDgpu).unwrap_err();
        assert!(e.contains(needle), "{e}");
    }

    #[test]
    fn every_pending_signal_blocks() {
        blocked(|g| g.pending_unknown = true, "cannot tell");
        blocked(|g| g.pending_reboot = Some(true), "firmware");
        blocked(|g| g.conf_mode = Some(GpuMode::Integrated), "supergfxd.conf");
        blocked(|g| g.toggle_running = true, "toggle");
        blocked(|g| g.armoury_pending = Some(GpuMode::AsusMuxDgpu), "reboot");
        blocked(|g| g.supported.clear(), "not answering");
        // a conf that agrees with the running mode is not pending
        let mut g = st(GpuMode::Hybrid, 1, 0);
        g.conf_mode = Some(GpuMode::Hybrid);
        assert!(plan_switch(&g, GpuMode::AsusMuxDgpu).is_ok());
    }

    #[test]
    fn unknown_hardware_state_refuses() {
        let mut g = st(GpuMode::Hybrid, 1, 0);
        g.mux = None;
        assert!(plan_switch(&g, GpuMode::Integrated).is_err());
        let mut g = st(GpuMode::Hybrid, 1, 0);
        g.dgpu_disable = None;
        assert!(plan_switch(&g, GpuMode::AsusMuxDgpu).is_err());
    }
}
