use armoury_proto::{GpuMode, GpuState, GpuStep};

/// The spec's GPU matrix. Never returns a step that can leave gpu_mux_mode=0 with
/// dgpu_disable=1 (black screen at boot).
pub fn plan_switch(g: &GpuState, target: GpuMode) -> Result<GpuStep, String> {
    use GpuMode::*;
    let current = g.mode.ok_or("supergfxd is not answering; try again")?;
    if let Some(p) = g.pending { return Err(format!("a switch to {p:?} is pending; reboot first")); }
    if !matches!(target, Integrated | Hybrid | AsusMuxDgpu) || !g.supported.contains(&target) {
        return Err(format!("{target:?} is not supported on this machine"));
    }
    if target == current { return Err(format!("already in {target:?}")); }
    let mux_dgpu = g.mux == Some(0);
    let dgpu_off = g.dgpu_disable == Some(1);
    match (current, target) {
        (Hybrid, Integrated) if mux_dgpu => Err("MUX is set to the dGPU; switch to Hybrid and reboot first".into()),
        (Hybrid, Integrated) | (Integrated, Hybrid) => Ok(GpuStep::OmarchyToggle { to: target }),
        (Hybrid, AsusMuxDgpu) if dgpu_off => Err("the dGPU is disabled; it must be enabled before selecting Ultimate".into()),
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
            for mux in [0u8, 1] {
                for off in [0u8, 1] {
                    for target in [Integrated, Hybrid, AsusMuxDgpu] {
                        let Ok(step) = plan_switch(&st(mode, mux, off), target) else { continue };
                        let first = match &step { GpuStep::FirstOfTwo { first, .. } => (**first).clone(), s => s.clone() };
                        // the only direct write that sets mux=0 must start from the dGPU enabled
                        if first == (GpuStep::Supergfx { to: AsusMuxDgpu }) { assert_eq!(off, 0, "{mode:?} mux={mux} off={off} -> {target:?}"); }
                        // Integrated (dgpu_disable=1) must never be entered while mux=0
                        if first == (GpuStep::OmarchyToggle { to: Integrated }) { assert_eq!(mux, 1, "{mode:?} mux={mux} off={off} -> {target:?}"); }
                    }
                }
            }
        }
    }
}
