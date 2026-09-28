# Plan 2b — Intel undervolt, NVIDIA clocks, GPU status

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** per-mode Intel CPU undervolt (only when the BIOS leaves the OC mailbox unlocked), per-mode NVIDIA core/memory clock offsets and clock locks, live NVIDIA status and the list of processes keeping the dGPU awake — all without ever waking a suspended dGPU.

**Architecture:** Two new allowlisted root commands: `armoury-root undervolt probe|set <mv>` (MSR 0x150 OC mailbox, core + cache planes) and `armoury-root nv-clocks <core_off> <mem_off> <core_lock|off> <mem_lock|off>` (NVML via `nvml-wrapper`). armouryd reads the dGPU's PCI runtime status from sysfs and only touches NVML while it is already `active`; NVIDIA settings are applied then and re-applied when the dGPU wakes. `apply_mode` gains a small hardware context (`uv_unlocked`, `dgpu_active`).

**Tech Stack:** as Plan 2, plus `nvml-wrapper = "0.13"` (loads `libnvidia-ml.so` at runtime).

**Spec:** `docs/superpowers/specs/2026-09-29-omarchy-armoury-design.md` §3.3, §4.1, §4.3 (dGPU users view).

## Global Constraints

- Root commands re-validate everything and refuse unless armouryd's active flag exists for `PKEXEC_UID` (same rule as `set-limits`).
- Undervolt range −150…0 mV (undervolt only; g-helper-linux's bound for Intel).
- NVIDIA offsets: within NVML's reported min/max for P0, and hard caps core −500…500 MHz, memory −2000…3000 MHz; locks 200…3000 MHz or `off`.
- Never wake a suspended dGPU: no NVML, no `/dev/nvidia*` access unless `power/runtime_status` is `active`.
- Kernel/firmware state that persists across mode switches (MSR offset, NVIDIA offsets/locks) is reset to stock for a mode that has no value of its own (same rule as CPU boost in Plan 2).
- Everything else from Plan 2's constraints still holds (writes only in active mode, config saved only after success).

**Facts (reference machine):** i9-12900H, `/dev/cpu/*/msr` present, `msr` module loaded; NVIDIA RTX 3070 Ti at `0000:01:00.0`, driver 610 (NVML has `nvmlDeviceSetClockOffsets`, `SetGpuLockedClocks`, `SetMemoryLockedClocks`); dGPU idles in D3cold (`runtime_status=suspended`).

**MSR 0x150 protocol (from g-helper-linux `vendor/gpu-helper/msr_ops.c`):** write command `0x80000011_xxxxxxxx | plane<<40` with payload `(round(mv*1.024) & 0x7FF) << 21`; read command `0x80000010_00000000 | plane<<40`, then read MSR 0x150 and decode bits 31:21 as 11-bit two's complement / 1.024. Planes: 0 core, 2 cache. Before writing: `modprobe msr` if `/dev/cpu/0/msr` is missing, and write `on` to `/sys/module/msr/parameters/allow_writes`. A BIOS-locked mailbox reads back 0.

## Review Focus

1. **Locked BIOS** → probe reports locked, the undervolt control is hidden, stored `uv_mv` is never sent. Pinned in Task 5 (`uv_skipped_when_locked`).
2. **dGPU suspended when a mode is applied** → no NVIDIA call; settings applied once the dGPU turns active. Pinned in Task 5 (`nv_deferred_until_dgpu_wakes`).
3. **Mode without undervolt/NVIDIA values after one that had them** → offsets/locks reset to stock. Pinned in Task 5 (`unset_values_reset_to_stock`).
4. **Out-of-range values (−200 mV, +900 MHz, lock 50 MHz, junk)** → rejected by armouryd and again by armoury-root. Pinned in Tasks 2, 3, 6.
5. **Status polling while the dGPU sleeps** → no NVML access (a test fake counts NVML reads). Pinned in Task 4 (`no_nvml_while_suspended`).

---

## File Structure

```
crates/proto/src/lib.rs                    + ModeSettings fields, NvStatus, GpuUser, GpuState/PerfState fields, ProbeUndervolt
crates/armoury-root/src/lib.rs             + uv_encode/uv_decode/uv commands, parse_nv_args
crates/armoury-root/src/msr.rs             MSR file I/O
crates/armoury-root/src/nv.rs              NVML clock writes
crates/armoury-root/src/main.rs            + undervolt, nv-clocks
crates/armouryd/src/hw/nvidia.rs           Nvidia trait: dgpu_active, status (NVML), users (/proc scan)
crates/armouryd/src/hw/fake.rs             + FakeNvidia
crates/armouryd/src/features/limits.rs     + validation of new fields
crates/armouryd/src/features/perf.rs       apply_mode(ctx) with undervolt + NVIDIA
crates/armouryd/src/ipc.rs                 probe on activation, dGPU wake re-apply, ProbeUndervolt
crates/armouryd/src/state.rs               GPU status/users into Snapshot
crates/armoury/src/main.rs                 + mode flags, status GPU line, `gpu users`
```

---

### Task 1: Protocol additions

**Files:** Modify `crates/proto/src/lib.rs`

**Produces:**
- `ModeSettings` += `uv_mv: Option<i32>`, `gpu_core_offset: Option<i32>`, `gpu_mem_offset: Option<i32>`, `gpu_core_lock: Option<u32>`, `gpu_mem_lock: Option<u32>` (lock `0` = off). All `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- `NvStatus { core_mhz: u32, mem_mhz: u32, temp_c: u32, power_w: f32, util_pct: u32, vram_used_mb: u64, vram_total_mb: u64, pstate: String, core_offset: i32, mem_offset: i32 }`.
- `GpuUser { pid: u32, name: String }`.
- `GpuState` += `dgpu_active: Option<bool>`, `nvidia: Option<NvStatus>`, `users: Vec<GpuUser>`.
- `PerfState` += `undervolt: Option<UndervoltState>`; `UndervoltState { unlocked: bool }` (None = not probed yet).
- `Request::ProbeUndervolt`.

- [ ] **Step 1: Failing test**

```rust
    #[test]
    fn tuning_fields_wire_format() {
        let r: Request = serde_json::from_str(r#"{"cmd":"set_mode_settings","profile":"performance","settings":{"uv_mv":-40,"gpu_core_offset":100,"gpu_core_lock":0}}"#).unwrap();
        assert_eq!(r, Request::SetModeSettings { profile: Profile::Performance, settings: ModeSettings {
            uv_mv: Some(-40), gpu_core_offset: Some(100), gpu_core_lock: Some(0), ..Default::default() } });
        assert_eq!(serde_json::to_string(&Request::ProbeUndervolt).unwrap(), r#"{"cmd":"probe_undervolt"}"#);
    }
```

- [ ] **Step 2: Run** `cargo test -p armoury-proto` → FAIL (unknown fields).
- [ ] **Step 3: Implement** the fields/types above (derive `Debug, Clone, PartialEq, Serialize, Deserialize`, plus `Default` where the struct has only optional/collection fields; `NvStatus` has `f32` so no `Eq`). Add `ProbeUndervolt` to `Request`. Fix any struct literals in other crates that no longer compile by adding `..Default::default()`.
- [ ] **Step 4: Run** `cargo test --workspace` → all pass.
- [ ] **Step 5: Commit** `feat(proto): undervolt, NVIDIA tuning and GPU status types`

---

### Task 2: armoury-root undervolt

**Files:** Modify `crates/armoury-root/src/lib.rs`, `main.rs`; create `crates/armoury-root/src/msr.rs`

**Produces (lib):** `pub const UV_MIN_MV: i32 = -150`, `pub const UV_MAX_MV: i32 = 0`; `pub fn uv_encode(mv: i32) -> u32`; `pub fn uv_decode(low: u32) -> i32`; `pub fn uv_write_cmd(plane: u64, mv: i32) -> u64`; `pub fn uv_read_cmd(plane: u64) -> u64`; `pub fn parse_uv(s: &str) -> Result<i32, String>`; `pub fn uv_matches(requested: i32, readback: i32) -> bool` (|diff| ≤ 3). **Produces (bin):** `armoury-root undervolt probe` → prints `unlocked` or `locked` (writes −5 mV, reads back, restores 0); `armoury-root undervolt set <mv>` → prints `core=<mv> cache=<mv>`, exit 1 with "BIOS-locked" when the readback does not match.

- [ ] **Step 1: Failing tests**

```rust
    #[test]
    fn uv_encoding_matches_gpu_helper() {
        // -50 mV: round(-51.2) = -51 → 11-bit 0x7CD → <<21
        assert_eq!(uv_encode(-50), 0x7CD << 21);
        assert_eq!(uv_encode(0), 0);
        assert_eq!(uv_decode(uv_encode(-50)), -50);
        assert_eq!(uv_decode(uv_encode(-150)), -150);
        assert_eq!(uv_write_cmd(0, -50), 0x8000_0011_0000_0000 | (0x7CD << 21) as u64);
        assert_eq!(uv_write_cmd(2, 0), 0x8000_0211_0000_0000);
        assert_eq!(uv_read_cmd(2), 0x8000_0210_0000_0000);
    }

    #[test]
    fn uv_arg_validation() {
        assert_eq!(parse_uv("-40"), Ok(-40));
        assert!(parse_uv("-151").is_err());
        assert!(parse_uv("5").is_err());
        assert!(parse_uv("-4O").is_err());
        assert!(uv_matches(-40, -39) && !uv_matches(-40, 0));
    }
```

- [ ] **Step 2: Run** `cargo test -p armoury-root` → FAIL.
- [ ] **Step 3: Implement** lib:

```rust
pub const UV_MIN_MV: i32 = -150;
pub const UV_MAX_MV: i32 = 0;
pub const MSR_OC_MAILBOX: u64 = 0x150;

pub fn uv_encode(mv: i32) -> u32 {
    let v = (mv as f64 * 1.024).round() as i32;
    ((v & 0x7FF) as u32) << 21
}

pub fn uv_decode(low: u32) -> i32 {
    let mut o = ((low >> 21) & 0x7FF) as i32;
    if o & 0x400 != 0 { o -= 0x800; }
    (o as f64 / 1.024).round() as i32
}

pub fn uv_write_cmd(plane: u64, mv: i32) -> u64 { 0x8000_0011_0000_0000 | (plane << 40) | uv_encode(mv) as u64 }
pub fn uv_read_cmd(plane: u64) -> u64 { 0x8000_0010_0000_0000 | (plane << 40) }

pub fn parse_uv(s: &str) -> Result<i32, String> {
    s.parse::<i32>().ok().filter(|v| (UV_MIN_MV..=UV_MAX_MV).contains(v))
        .ok_or_else(|| format!("undervolt must be an integer {UV_MIN_MV}–{UV_MAX_MV} mV, got {s:?}"))
}

pub fn uv_matches(requested: i32, readback: i32) -> bool { (requested - readback).abs() <= 3 }
```

`msr.rs`:

```rust
use anyhow::Context;
use std::os::unix::fs::FileExt;

pub struct Msr(std::fs::File);

impl Msr {
    pub fn open() -> anyhow::Result<Self> {
        if !std::path::Path::new("/dev/cpu/0/msr").exists() {
            let _ = std::process::Command::new("modprobe").arg("msr").status();
        }
        let _ = std::fs::write("/sys/module/msr/parameters/allow_writes", "on");
        Ok(Self(std::fs::OpenOptions::new().read(true).write(true).open("/dev/cpu/0/msr").context("open /dev/cpu/0/msr")?))
    }
    pub fn write(&self, reg: u64, v: u64) -> anyhow::Result<()> {
        self.0.write_all_at(&v.to_le_bytes(), reg).with_context(|| format!("write MSR {reg:#x}"))
    }
    pub fn read(&self, reg: u64) -> anyhow::Result<u64> {
        let mut b = [0u8; 8];
        self.0.read_exact_at(&mut b, reg).with_context(|| format!("read MSR {reg:#x}"))?;
        Ok(u64::from_le_bytes(b))
    }
    /// Sets core (plane 0) and cache (plane 2) offsets; returns the decoded readbacks.
    pub fn set_uv(&self, mv: i32) -> anyhow::Result<(i32, i32)> {
        use armoury_root::{MSR_OC_MAILBOX, uv_decode, uv_read_cmd, uv_write_cmd};
        self.write(MSR_OC_MAILBOX, uv_write_cmd(0, mv))?;
        self.write(MSR_OC_MAILBOX, uv_write_cmd(2, mv))?;
        let mut rb = [0; 2];
        for (i, plane) in [0u64, 2].into_iter().enumerate() {
            self.write(MSR_OC_MAILBOX, uv_read_cmd(plane))?;
            rb[i] = uv_decode(self.read(MSR_OC_MAILBOX)? as u32);
        }
        Ok((rb[0], rb[1]))
    }
}
```

`main.rs`: `mod msr;`; add `Undervolt { #[command(subcommand)] action: UvAction }` with `enum UvAction { Probe, Set { mv: String } }`; factor the active-flag check from `set_limits` into `fn require_active() -> anyhow::Result<()>` and call it first in `undervolt`; arms:

```rust
fn undervolt(action: UvAction) -> anyhow::Result<()> {
    require_active()?;
    let msr = msr::Msr::open()?;
    match action {
        UvAction::Probe => {
            let (core, _) = msr.set_uv(-5)?;
            msr.set_uv(0)?;
            println!("{}", if uv_matches(-5, core) { "unlocked" } else { "locked" });
        }
        UvAction::Set { mv } => {
            let mv = parse_uv(&mv).map_err(anyhow::Error::msg)?;
            let (core, cache) = msr.set_uv(mv)?;
            println!("core={core} cache={cache}");
            if !uv_matches(mv, core) { bail!("requested {mv} mV, read back {core} mV: undervolt is BIOS-locked"); }
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run** `cargo test -p armoury-root` → pass; `cargo build -p armoury-root` clean.
- [ ] **Step 5: Commit** `feat(armoury-root): undervolt probe/set via MSR 0x150`

---

### Task 3: armoury-root nv-clocks

**Files:** Modify `crates/armoury-root/Cargo.toml` (`nvml-wrapper = "0.13"`), `lib.rs`, `main.rs`; create `crates/armoury-root/src/nv.rs`

**Produces (lib):** `pub struct NvArgs { pub core_offset: i32, pub mem_offset: i32, pub core_lock: Option<u32>, pub mem_lock: Option<u32> }`; `pub fn parse_nv_args(a: &[String]) -> Result<NvArgs, String>` (exactly 4 args: two ints, two `MHz|off`); caps as in Global Constraints. **Produces (bin):** `armoury-root nv-clocks <core_off> <mem_off> <core_lock|off> <mem_lock|off>`: requires active flag; opens device 0 via NVML; checks each offset against `clock_offset(Clock, P0)` min/max; sets offsets for Graphics and Memory at P0; sets or resets locks (`set_gpu_locked_clocks(Numeric{min_clock_mhz: 0, max_clock_mhz})`, `set_mem_locked_clocks(0, max)`, `reset_*_locked_clocks`); prints `core_offset=… mem_offset=…` read back from NVML.

- [ ] **Step 1: Failing tests**

```rust
    #[test]
    fn nv_args() {
        let a = |v: &[&str]| parse_nv_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&["100", "-200", "1500", "off"]), Ok(NvArgs { core_offset: 100, mem_offset: -200, core_lock: Some(1500), mem_lock: None }));
        assert!(a(&["900", "0", "off", "off"]).unwrap_err().contains("core"));
        assert!(a(&["0", "0", "50", "off"]).unwrap_err().contains("lock"));
        assert!(a(&["0", "0", "off"]).unwrap_err().contains("4"));
        assert!(a(&["x", "0", "off", "off"]).is_err());
    }
```

- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** lib:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvArgs { pub core_offset: i32, pub mem_offset: i32, pub core_lock: Option<u32>, pub mem_lock: Option<u32> }

pub fn parse_nv_args(a: &[String]) -> Result<NvArgs, String> {
    if a.len() != 4 { return Err(format!("nv-clocks takes 4 arguments, got {}", a.len())); }
    let off = |s: &str, name: &str, lo: i32, hi: i32| s.parse::<i32>().ok().filter(|v| (lo..=hi).contains(v))
        .ok_or_else(|| format!("{name} offset must be {lo}–{hi} MHz, got {s:?}"));
    let lock = |s: &str, name: &str| -> Result<Option<u32>, String> {
        if s == "off" { return Ok(None); }
        s.parse::<u32>().ok().filter(|v| (200..=3000).contains(v)).map(Some)
            .ok_or_else(|| format!("{name} lock must be 200–3000 MHz or off, got {s:?}"))
    };
    Ok(NvArgs {
        core_offset: off(&a[0], "core", -500, 500)?,
        mem_offset: off(&a[1], "memory", -2000, 3000)?,
        core_lock: lock(&a[2], "core")?,
        mem_lock: lock(&a[3], "memory")?,
    })
}
```

`nv.rs`:

```rust
use anyhow::{Context, bail};
use armoury_root::NvArgs;
use nvml_wrapper::enum_wrappers::device::{Clock, PerformanceState};
use nvml_wrapper::enums::device::GpuLockedClocksSetting;

pub fn apply(a: NvArgs) -> anyhow::Result<(i32, i32)> {
    let nvml = nvml_wrapper::Nvml::init().context("NVML init")?;
    let mut dev = nvml.device_by_index(0).context("NVIDIA device 0")?;
    for (clock, v, name) in [(Clock::Graphics, a.core_offset, "core"), (Clock::Memory, a.mem_offset, "memory")] {
        let r = dev.clock_offset(clock, PerformanceState::Zero).with_context(|| format!("{name} offset range"))?;
        if v < r.min_clock_offset_mhz || v > r.max_clock_offset_mhz {
            bail!("{name} offset {v} outside the card's range {}–{}", r.min_clock_offset_mhz, r.max_clock_offset_mhz);
        }
        dev.set_clock_offset(clock, PerformanceState::Zero, v).with_context(|| format!("set {name} offset"))?;
    }
    match a.core_lock {
        Some(max) => dev.set_gpu_locked_clocks(GpuLockedClocksSetting::Numeric { min_clock_mhz: 0, max_clock_mhz: max })?,
        None => dev.reset_gpu_locked_clocks()?,
    }
    match a.mem_lock {
        Some(max) => dev.set_mem_locked_clocks(0, max)?,
        None => dev.reset_mem_locked_clocks()?,
    }
    let core = dev.clock_offset(Clock::Graphics, PerformanceState::Zero)?.clock_offset_mhz;
    let mem = dev.clock_offset(Clock::Memory, PerformanceState::Zero)?.clock_offset_mhz;
    Ok((core, mem))
}
```

`main.rs`: `mod nv;`, `NvClocks { args: Vec<String> }` → `require_active()?; let a = parse_nv_args(&args)?; let (c, m) = nv::apply(a)?; println!("core_offset={c} mem_offset={m}")`.

- [ ] **Step 4: Run** `cargo test -p armoury-root && cargo build -p armoury-root` → pass/clean (if `reset_mem_locked_clocks` has a different name in 0.13, use the one in `nvml-wrapper-0.13.0/src/device.rs` and ledger it).
- [ ] **Step 5: Commit** `feat(armoury-root): nv-clocks offsets and locks via NVML`

---

### Task 4: armouryd NVIDIA reader (never wakes the dGPU)

**Files:** Create `crates/armouryd/src/hw/nvidia.rs`; modify `hw/mod.rs`, `hw/fake.rs`, `state.rs`, `Cargo.toml` (`nvml-wrapper = "0.13"`)

**Produces:** `trait Nvidia: Send + Sync { fn dgpu_active(&self) -> Option<bool>; fn status(&self) -> Option<NvStatus>; fn users(&self) -> Vec<GpuUser>; }`; `RealNvidia::new(sys_root)` (finds the first PCI device with `vendor` `0x10de` and `class` starting `0x03` under `sys/bus/pci/devices`; `dgpu_active` = `power/runtime_status == "active"`; `status` initialises NVML per call and drops it; `users` scans `/proc/*/fd` for links to `/dev/nvidia*`, skipping armouryd's own pid); `FakeNvidia { active: Option<bool>, status_reads: AtomicU32 }`. `collect` takes `nv: &dyn Nvidia` and fills `gpu.dgpu_active`, and **only if active** `gpu.nvidia` and `gpu.users`.

- [ ] **Step 1: Failing tests** (state.rs)

```rust
    #[tokio::test]
    async fn no_nvml_while_suspended() {
        let nv = FakeNvidia { active: Some(false), ..Default::default() };
        let s = collect(&machine(), &FakeGfx::default(), &FakeServices::default(), &nv, ControlMode::Observe).await;
        assert_eq!(s.gpu.dgpu_active, Some(false));
        assert!(s.gpu.nvidia.is_none() && s.gpu.users.is_empty());
        assert_eq!(nv.status_reads.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn nvml_read_when_active() {
        let nv = FakeNvidia { active: Some(true), ..Default::default() };
        let s = collect(&machine(), &FakeGfx::default(), &FakeServices::default(), &nv, ControlMode::Observe).await;
        assert_eq!(s.gpu.nvidia.unwrap().core_mhz, 1500);
        assert_eq!(s.gpu.users[0].name, "game");
    }
```

and a sysfs test in `nvidia.rs`:

```rust
    #[test]
    fn finds_dgpu_and_runtime_status() {
        let d = tempfile::tempdir().unwrap();
        let igpu = d.path().join("sys/bus/pci/devices/0000:00:02.0");
        let dgpu = d.path().join("sys/bus/pci/devices/0000:01:00.0");
        for (p, v, c) in [(&igpu, "0x8086", "0x030000"), (&dgpu, "0x10de", "0x030000")] {
            std::fs::create_dir_all(p.join("power")).unwrap();
            std::fs::write(p.join("vendor"), v).unwrap();
            std::fs::write(p.join("class"), c).unwrap();
        }
        std::fs::write(dgpu.join("power/runtime_status"), "suspended\n").unwrap();
        let nv = RealNvidia::new(d.path());
        assert_eq!(nv.dgpu_active(), Some(false));
        std::fs::write(dgpu.join("power/runtime_status"), "active\n").unwrap();
        assert_eq!(nv.dgpu_active(), Some(true));
    }
```

- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.** `FakeNvidia::status` increments `status_reads` and returns `NvStatus { core_mhz: 1500, pstate: "P0".into(), ..Default::default() }` (derive `Default` on `NvStatus`); `users` returns `[GpuUser { pid: 42, name: "game".into() }]`. `RealNvidia`:

```rust
pub struct RealNvidia { root: PathBuf, dev: Option<PathBuf> }

impl RealNvidia {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let dir = root.join("sys/bus/pci/devices");
        let dev = std::fs::read_dir(&dir).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| {
            let r = |f: &str| std::fs::read_to_string(p.join(f)).unwrap_or_default();
            r("vendor").trim() == "0x10de" && r("class").trim().starts_with("0x03")
        }));
        Self { root, dev }
    }
}

impl Nvidia for RealNvidia {
    fn dgpu_active(&self) -> Option<bool> {
        let s = std::fs::read_to_string(self.dev.as_ref()?.join("power/runtime_status")).ok()?;
        Some(s.trim() == "active")
    }
    fn status(&self) -> Option<NvStatus> {
        use nvml_wrapper::enum_wrappers::device::{Clock, PerformanceState, TemperatureSensor};
        let nvml = nvml_wrapper::Nvml::init().ok()?;
        let d = nvml.device_by_index(0).ok()?;
        let mem = d.memory_info().ok()?;
        Some(NvStatus {
            core_mhz: d.clock_info(Clock::Graphics).unwrap_or(0),
            mem_mhz: d.clock_info(Clock::Memory).unwrap_or(0),
            temp_c: d.temperature(TemperatureSensor::Gpu).unwrap_or(0),
            power_w: d.power_usage().map(|mw| mw as f32 / 1000.0).unwrap_or(0.0),
            util_pct: d.utilization_rates().map(|u| u.gpu).unwrap_or(0),
            vram_used_mb: mem.used / 1_048_576,
            vram_total_mb: mem.total / 1_048_576,
            pstate: d.performance_state().map(|p| format!("{p:?}")).unwrap_or_default(),
            core_offset: d.clock_offset(Clock::Graphics, PerformanceState::Zero).map(|o| o.clock_offset_mhz).unwrap_or(0),
            mem_offset: d.clock_offset(Clock::Memory, PerformanceState::Zero).map(|o| o.clock_offset_mhz).unwrap_or(0),
        })
    }
    fn users(&self) -> Vec<GpuUser> {
        let me = std::process::id();
        let proc_ = self.root.join("proc");
        std::fs::read_dir(&proc_).into_iter().flatten().flatten().filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            if pid == me { return None; }
            let holds = std::fs::read_dir(e.path().join("fd")).ok()?.flatten()
                .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|l| l.to_string_lossy().starts_with("/dev/nvidia")));
            holds.then(|| GpuUser { pid, name: std::fs::read_to_string(e.path().join("comm")).unwrap_or_default().trim().to_string() })
        }).collect()
    }
}
```

`collect` signature becomes `collect(sys, gfx, svc, nv, control)`; inside `gpu_state` set `dgpu_active = nv.dgpu_active()` and, if `Some(true)`, `nvidia = nv.status()`, `users = nv.users()`. Update every `collect` caller and test (pass `&FakeNvidia::default()`), `Daemon` gets `nv: Box<dyn Nvidia>` (constructor gains it after `asusd`), `main.rs` passes `RealNvidia::new("/")`.

- [ ] **Step 4: Run** `cargo test --workspace` → pass.
- [ ] **Step 5: Commit** `feat(armouryd): NVIDIA status and dGPU users, read only while the dGPU is awake`

---

### Task 5: apply undervolt and NVIDIA settings

**Files:** Modify `crates/armouryd/src/features/perf.rs`, `ipc.rs`

**Produces:** `pub struct HwCtx { pub uv_unlocked: bool, pub dgpu_active: bool }`; `apply_mode(profile, s, ctx: HwCtx, asusd, svc) -> Vec<String>`: after Plan 2's steps, if `ctx.uv_unlocked` → `pkexec armoury-root undervolt set <s.uv_mv.unwrap_or(0)>`; if `ctx.dgpu_active` → `pkexec armoury-root nv-clocks <core|0> <mem|0> <lock|off> <lock|off>` (lock `Some(0)`/`None` → `off`). Daemon: `uv: std::sync::Mutex<Option<bool>>` (probe result); on the first active tick (and on `ProbeUndervolt`) run `pkexec armoury-root undervolt probe` via a new `Services::output(argv) -> anyhow::Result<String>` and store `unlocked`; `Snapshot.perf.undervolt = Some(UndervoltState { unlocked })` once probed. `ApplyState` gains `dgpu_was_active: bool`; when the dGPU goes inactive→active while active mode, apply the NVIDIA part (`nv-clocks`) for the current mode.

- [ ] **Step 1: Failing tests** (perf.rs)

```rust
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
```

and in ipc.rs:

```rust
    #[tokio::test]
    async fn nv_applied_when_dgpu_wakes() {
        // rig variant with a shared FakeNvidia whose `active` can be flipped
        let r = rig_with_nv("[modes.balanced]\ngpu_core_offset = 50\n", true);
        r.d.tick().await;
        assert!(!r.svc.calls.lock().unwrap().iter().any(|c| c.contains("nv-clocks")));
        *r.nv.active_flag.lock().unwrap() = Some(true);
        r.d.tick().await;
        assert!(r.svc.calls.lock().unwrap().iter().any(|c| c.ends_with("nv-clocks 50 0 off off")));
    }
```
(`FakeNvidia` gets `active_flag: Mutex<Option<bool>>` used by `dgpu_active`; keep `active` as the initial value via a constructor `FakeNvidia::with_active(Option<bool>)`.)

- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** as described; update all `apply_mode` callers (tick passes `HwCtx { uv_unlocked: self.uv.lock().unwrap().unwrap_or(false), dgpu_active: snap.gpu.dgpu_active == Some(true) }`). Add `async fn output(&self, argv: &[&str]) -> anyhow::Result<String>` to `Services` (RealServices: same as `run` but returns stdout; FakeServices: records the call and returns `self.outputs` map entry or `""`; Arc forwarding).
- [ ] **Step 4: Run** `cargo test --workspace` → pass.
- [ ] **Step 5: Commit** `feat(armouryd): per-mode undervolt and NVIDIA clocks with probe and dGPU-wake re-apply`

---

### Task 6: validation of new fields

**Files:** Modify `crates/armouryd/src/features/limits.rs`

**Produces:** `validate` also checks `uv_mv` in −150…0, offsets in core −500…500 / mem −2000…3000, locks `0` or 200…3000.

- [ ] **Step 1: Failing test**

```rust
    #[test]
    fn tuning_ranges() {
        assert!(validate(&ModeSettings { uv_mv: Some(-200), ..Default::default() }, fb).unwrap_err().contains("undervolt"));
        assert!(validate(&ModeSettings { gpu_core_offset: Some(900), ..Default::default() }, fb).is_err());
        assert!(validate(&ModeSettings { gpu_mem_lock: Some(50), ..Default::default() }, fb).is_err());
        assert!(validate(&ModeSettings { uv_mv: Some(-40), gpu_core_offset: Some(100), gpu_core_lock: Some(0), ..Default::default() }, fb).is_ok());
    }
```

- [ ] **Step 2–4:** implement (append checks after the limit loop), run → pass.
- [ ] **Step 5: Commit** `feat(armouryd): validate undervolt and NVIDIA settings`

---

### Task 7: CLI

**Files:** Modify `crates/armoury/src/main.rs`

**Produces:** `mode … set` flags `--uv <mV>` (allow_hyphen_values), `--gpu-core <MHz>`, `--gpu-mem <MHz>`, `--gpu-core-lock <MHz|off>`, `--gpu-mem-lock <MHz|off>`; `armoury undervolt probe`; `armoury gpu users`; status gains `dGPU       active · P0 · 1500/7000 MHz · 62°C · 35.2 W · 3 users` or `dGPU       suspended`, and `Undervolt  unlocked|locked|-`.

- [ ] **Step 1: Failing tests** — `parse_lock("off") == Ok(0)`, `parse_lock("1500") == Ok(1500)`, summary contains `dGPU       suspended` when `dgpu_active == Some(false)`, and `Undervolt  locked`.
- [ ] **Step 2–4:** implement, run → pass.
- [ ] **Step 5: Commit** `feat(armoury): undervolt and NVIDIA tuning flags, GPU status`

---

### Task 8: On-device verification

- [ ] **Step 1:** `cargo test --workspace && tests/uninstall_test.sh`; rebuild release; install user binaries; `pkexec install` the root helper; `systemctl --user restart armouryd`.
- [ ] **Step 2 (observe):** `armoury status` → `dGPU suspended`, no NVML access (the dGPU stays `suspended` across 10 s of polling: `cat /sys/bus/pci/devices/0000:01:00.0/power/runtime_status`).
- [ ] **Step 3 (takeover, G-Helper down ~2 min):** `armoury takeover`; `armoury status` shows `Undervolt unlocked|locked` (probe ran). If unlocked: `armoury mode performance set --uv -30` and confirm with a 20-thread load that package power at the same clocks drops or the readback line shows `core=-30`; then `--uv 0`.
- [ ] **Step 4 (NVIDIA):** wake the dGPU (`prime-run glxgears` or any CUDA/Vulkan app in the background), `armoury mode performance set --gpu-core 50`; confirm `armoury status` shows `core offset 50`; `--gpu-core 0`; stop the app; confirm the dGPU returns to `suspended`.
- [ ] **Step 5:** `armoury handback`; delete the test `config.toml`; `armoury status` shows observe + G-Helper running.
