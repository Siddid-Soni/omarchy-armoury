use armoury_proto::HotKey;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// ASUS hotkeys all arrive on this device on the G533ZW (captured on device).
pub const NKEY_NAME: &str = "ASUSTek Computer Inc. N-KEY Device";
const EV_KEY: u16 = 1;
const KEY_PROG1: u16 = 148; // ROG key
const KEY_PROG3: u16 = 202; // Fn+F4 (Aura)
const KEY_PROG4: u16 = 203; // Fn+F5
const EVENT_SIZE: usize = 24; // struct input_event on 64-bit

pub fn hotkey(code: u16) -> Option<HotKey> {
    match code { KEY_PROG1 => Some(HotKey::Rog), KEY_PROG3 => Some(HotKey::Aura), KEY_PROG4 => Some(HotKey::Fan), _ => None }
}

/// Key-down events for our hotkeys; auto-repeat (value 2) and releases are ignored.
pub fn presses(raw: &[u8]) -> Vec<HotKey> {
    raw.chunks_exact(EVENT_SIZE).filter_map(|e| {
        let typ = u16::from_ne_bytes([e[16], e[17]]);
        let code = u16::from_ne_bytes([e[18], e[19]]);
        let value = i32::from_ne_bytes([e[20], e[21], e[22], e[23]]);
        (typ == EV_KEY && value == 1).then(|| hotkey(code)).flatten()
    }).collect()
}

/// The N-KEY keyboard's event node, looked up by name (event numbers change across boots).
pub fn find_nkey(sys_root: &Path) -> Option<PathBuf> {
    let mut nodes: Vec<String> = std::fs::read_dir(sys_root.join("sys/class/input")).ok()?.flatten()
        .filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.starts_with("event")).collect();
    nodes.sort();
    nodes.into_iter()
        .find(|n| std::fs::read_to_string(sys_root.join("sys/class/input").join(n).join("device/name")).is_ok_and(|s| s.trim() == NKEY_NAME))
        .map(|n| PathBuf::from("/dev/input").join(n))
}

/// Set by the kernel when the firmware changes the keyboard backlight itself (Fn+F2/F3);
/// pollable (POLLPRI), so waiting on it costs nothing and never calls into the EC.
pub const KBD_HW_CHANGED: &str = "/sys/class/leds/asus::kbd_backlight/brightness_hw_changed";

pub fn parse_level(raw: &[u8]) -> Option<u8> {
    std::str::from_utf8(raw).ok()?.trim().parse::<u8>().ok().filter(|l| *l <= 3)
}

/// `brightness_hw_changed` reads ENODATA until the firmware first changes the backlight
/// after boot: that means "nothing yet", not a broken watch.
pub fn no_change_yet(r: std::io::Result<Option<u8>>) -> std::io::Result<Option<u8>> {
    match r {
        Err(e) if e.raw_os_error() == Some(61) => Ok(None), // ENODATA
        other => other,
    }
}

/// Forwards every firmware keyboard-brightness change (0–3). Reopens after errors.
pub async fn run_kbd_watch(tx: tokio::sync::mpsc::Sender<u8>) {
    use std::os::unix::fs::FileExt;
    use tokio::io::{unix::AsyncFd, Interest};
    let read = |f: &std::fs::File| -> std::io::Result<Option<u8>> {
        let mut buf = [0u8; 8];
        no_change_yet(f.read_at(&mut buf, 0).map(|n| parse_level(&buf[..n])))
    };
    let mut failures = 0u32;
    loop {
        let result: std::io::Result<()> = async {
            let fd = AsyncFd::with_interest(std::fs::File::open(KBD_HW_CHANGED)?, Interest::PRIORITY)?;
            read(fd.get_ref())?; // sysfs arms the notification on read
            failures = 0;
            loop {
                let mut guard = fd.ready(Interest::PRIORITY).await?;
                let level = read(guard.get_inner())?;
                guard.clear_ready();
                if let Some(l) = level {
                    if tx.send(l).await.is_err() { return Ok(()); }
                }
            }
        }.await;
        match result {
            Ok(()) => return,
            Err(e) => {
                eprintln!("armouryd: keyboard brightness watch: {e}");
                tokio::time::sleep(backoff(failures)).await;
                failures += 1;
            }
        }
    }
}

/// Signals every Keystone insert/remove: asus-wmi calls sysfs_notify on `keystone`
/// (seen on the G533ZW). Only waits; the value is read by the poll tick. Reopens after errors.
pub async fn run_keystone_watch(tx: tokio::sync::mpsc::Sender<()>) {
    use std::os::unix::fs::FileExt;
    use tokio::io::{unix::AsyncFd, Interest};
    let path = Path::new("/").join(crate::hw::sysfs::KEYSTONE);
    // sysfs arms the notification on read
    let rearm = |f: &std::fs::File| f.read_at(&mut [0u8; 8], 0).map(|_| ());
    let mut failures = 0u32;
    loop {
        let result: std::io::Result<()> = async {
            let fd = AsyncFd::with_interest(std::fs::File::open(&path)?, Interest::PRIORITY)?;
            rearm(fd.get_ref())?;
            failures = 0;
            loop {
                let mut guard = fd.ready(Interest::PRIORITY).await?;
                rearm(guard.get_inner())?;
                guard.clear_ready();
                if tx.send(()).await.is_err() { return Ok(()); }
            }
        }.await;
        match result {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return, // no Keystone slot
            Err(e) => {
                eprintln!("armouryd: Keystone watch: {e}");
                tokio::time::sleep(backoff(failures)).await;
                failures += 1;
            }
        }
    }
}

pub fn backoff(failures: u32) -> Duration {
    Duration::from_secs((2u64 << failures.min(4)).min(30))
}

/// Reads the N-KEY device forever (no grab, so every key still reaches Hyprland) and
/// forwards ROG / Fn+F4 / Fn+F5 presses. Reopens after errors (resume, replug) with a backoff.
pub async fn run_reader(tx: tokio::sync::mpsc::Sender<HotKey>) {
    let mut failures = 0u32;
    loop {
        let result: std::io::Result<()> = async {
            let path = find_nkey(Path::new("/")).ok_or_else(|| std::io::Error::other("N-KEY keyboard not found"))?;
            let mut f = tokio::fs::File::open(&path).await?;
            failures = 0;
            let mut buf = vec![0u8; EVENT_SIZE * 64];
            loop {
                let n = f.read(&mut buf).await?;
                if n == 0 { return Err(std::io::Error::other("device closed")); }
                for k in presses(&buf[..n - n % EVENT_SIZE]) {
                    if tx.send(k).await.is_err() { return Ok(()); }
                }
            }
        }.await;
        match result {
            Ok(()) => return,
            Err(e) => {
                if failures == 0 { eprintln!("armouryd: hotkeys: {e}"); }
                tokio::time::sleep(backoff(failures)).await;
                failures += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(typ: u16, code: u16, value: i32) -> Vec<u8> {
        let mut b = vec![0u8; 16]; // struct timeval
        b.extend(typ.to_ne_bytes());
        b.extend(code.to_ne_bytes());
        b.extend(value.to_ne_bytes());
        b
    }

    #[test]
    fn no_brightness_change_since_boot_is_not_an_error() {
        // the kernel answers ENODATA until the first firmware change after boot
        assert_eq!(no_change_yet(Err(std::io::Error::from_raw_os_error(61))).unwrap(), None);
        assert!(no_change_yet(Err(std::io::Error::from_raw_os_error(5))).is_err());
        assert_eq!(no_change_yet(Ok(Some(2))).unwrap(), Some(2));
    }

    #[test]
    fn kbd_level_parsing() {
        assert_eq!(parse_level(b"2\n"), Some(2));
        assert_eq!(parse_level(b"x"), None);
        assert_eq!(parse_level(b"9\n"), None, "levels are 0-3");
    }

    #[test]
    fn aura_key_code() {
        assert_eq!(hotkey(202), Some(HotKey::Aura)); // Fn+F4, captured on the G533ZW
    }

    #[test]
    fn presses_from_raw_events() {
        // captured shape: MSC_SCAN then KEY_PROG4 press, SYN, release; then an auto-repeat
        let mut raw = ev(4, 4, -13565778);
        raw.extend(ev(1, 203, 1));
        raw.extend(ev(0, 0, 0));
        raw.extend(ev(1, 203, 0));
        raw.extend(ev(1, 148, 1));
        raw.extend(ev(1, 148, 2)); // repeat
        raw.extend(ev(1, 115, 1)); // volume up: not ours
        assert_eq!(presses(&raw), vec![HotKey::Fan, HotKey::Rog]);
    }

    #[test]
    fn finds_nkey_device() {
        let d = tempfile::tempdir().unwrap();
        for (ev, name) in [("event3", "AT Translated Set 2 keyboard"), ("event12", "ASUSTek Computer Inc. N-KEY Device")] {
            let p = d.path().join("sys/class/input").join(ev).join("device");
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("name"), format!("{name}\n")).unwrap();
        }
        assert_eq!(find_nkey(d.path()), Some(std::path::PathBuf::from("/dev/input/event12")));
        assert_eq!(find_nkey(&d.path().join("none")), None);
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(0), std::time::Duration::from_secs(2));
        assert_eq!(backoff(1), std::time::Duration::from_secs(4));
        assert_eq!(backoff(10), std::time::Duration::from_secs(30));
    }
}
