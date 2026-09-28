use anyhow::Context;
use armoury_root::{MSR_OC_MAILBOX, uv_decode, uv_read_cmd, uv_write_cmd};
use std::os::unix::fs::FileExt;

const ALLOW_WRITES: &str = "/sys/module/msr/parameters/allow_writes";

/// /dev/cpu/0/msr opened for the OC mailbox; restores msr's allow_writes on drop.
pub struct Msr {
    f: std::fs::File,
    prev_allow: Option<String>,
}

impl Msr {
    pub fn open() -> anyhow::Result<Self> {
        if !std::path::Path::new("/dev/cpu/0/msr").exists() {
            let _ = std::process::Command::new("modprobe").arg("msr").status();
        }
        let prev_allow = std::fs::read_to_string(ALLOW_WRITES).ok().map(|s| s.trim().to_string()).filter(|s| s != "on");
        let _ = std::fs::write(ALLOW_WRITES, "on");
        let f = std::fs::OpenOptions::new().read(true).write(true).open("/dev/cpu/0/msr").context("open /dev/cpu/0/msr")?;
        Ok(Self { f, prev_allow })
    }

    fn write(&self, v: u64) -> anyhow::Result<()> {
        self.f.write_all_at(&v.to_le_bytes(), MSR_OC_MAILBOX).context("write MSR 0x150")
    }

    fn read(&self) -> anyhow::Result<u64> {
        let mut b = [0u8; 8];
        self.f.read_exact_at(&mut b, MSR_OC_MAILBOX).context("read MSR 0x150")?;
        Ok(u64::from_le_bytes(b))
    }

    /// Sets the core and cache offsets (package-wide); returns their decoded readbacks.
    pub fn set_uv(&self, mv: i32) -> anyhow::Result<(i32, i32)> {
        self.write(uv_write_cmd(0, mv))?;
        self.write(uv_write_cmd(2, mv))?;
        let mut rb = [0; 2];
        for (i, plane) in [0u64, 2].into_iter().enumerate() {
            self.write(uv_read_cmd(plane))?;
            rb[i] = uv_decode(self.read()? as u32);
        }
        Ok((rb[0], rb[1]))
    }
}

impl Drop for Msr {
    fn drop(&mut self) {
        if let Some(prev) = &self.prev_allow { let _ = std::fs::write(ALLOW_WRITES, prev); }
    }
}
