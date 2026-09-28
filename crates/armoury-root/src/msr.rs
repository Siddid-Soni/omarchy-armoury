use anyhow::Context;
use armoury_root::{MSR_OC_MAILBOX, uv_decode, uv_read_cmd, uv_write_cmd};
use std::os::unix::fs::FileExt;

pub struct Msr(std::fs::File);

impl Msr {
    pub fn open() -> anyhow::Result<Self> {
        if !std::path::Path::new("/dev/cpu/0/msr").exists() {
            let _ = std::process::Command::new("modprobe").arg("msr").status();
        }
        let _ = std::fs::write("/sys/module/msr/parameters/allow_writes", "on");
        let f = std::fs::OpenOptions::new().read(true).write(true).open("/dev/cpu/0/msr").context("open /dev/cpu/0/msr")?;
        Ok(Self(f))
    }

    fn write(&self, v: u64) -> anyhow::Result<()> {
        self.0.write_all_at(&v.to_le_bytes(), MSR_OC_MAILBOX).context("write MSR 0x150")
    }

    fn read(&self) -> anyhow::Result<u64> {
        let mut b = [0u8; 8];
        self.0.read_exact_at(&mut b, MSR_OC_MAILBOX).context("read MSR 0x150")?;
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
