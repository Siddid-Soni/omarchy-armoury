use tokio::process::{Child, Command};

/// Keeps a logind lid-switch inhibitor (systemd-inhibit) alive while clamshell mode is
/// wanted and the laptop is on AC, so closing the lid does not suspend. The child is
/// killed on drop, so no inhibitor outlives armouryd.
pub struct Clamshell {
    argv: Vec<String>,
    child: Option<Child>,
    spawns: u32,
}

impl Clamshell {
    pub fn new(argv: Vec<String>) -> Self { Self { argv, child: None, spawns: 0 } }

    pub fn systemd_inhibit() -> Self {
        Self::new(["systemd-inhibit", "--what=handle-lid-switch", "--who=omarchy-armoury", "--why=Clamshell mode (on AC)", "sleep", "infinity"]
            .map(String::from).to_vec())
    }

    pub fn holding(&mut self) -> bool {
        matches!(self.child.as_mut().map(|c| c.try_wait()), Some(Ok(None)))
    }

    pub fn spawn_count(&self) -> u32 { self.spawns }

    pub async fn update(&mut self, wanted: bool, on_ac: bool) {
        if wanted && on_ac {
            if !self.holding() {
                let (prog, args) = self.argv.split_first().expect("argv");
                match Command::new(prog).args(args).kill_on_drop(true).spawn() {
                    Ok(c) => { self.child = Some(c); self.spawns += 1; }
                    Err(e) => eprintln!("armouryd: clamshell inhibitor: {e}"),
                }
            }
        } else if let Some(mut c) = self.child.take() {
            let _ = c.kill().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn holds_inhibitor_only_when_wanted_and_on_ac() {
        let mut c = Clamshell::new(vec!["sleep".into(), "30".into()]);
        c.update(true, true).await;
        assert!(c.holding());
        c.update(true, false).await; // unplugged
        assert!(!c.holding());
        c.update(true, true).await;
        assert!(c.holding());
        c.update(false, true).await; // turned off
        assert!(!c.holding());
    }

    #[tokio::test]
    async fn restarts_a_dead_inhibitor() {
        let mut c = Clamshell::new(vec!["true".into()]); // exits immediately
        c.update(true, true).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        c.update(true, true).await;
        assert!(c.spawn_count() >= 2, "respawned after it exited");
    }
}
