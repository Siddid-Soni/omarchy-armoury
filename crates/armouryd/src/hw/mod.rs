pub mod sysfs;

pub trait Sysfs: Send + Sync {
    fn read(&self, rel: &str) -> Option<String>;
    fn list(&self, rel: &str) -> Vec<String>;
}
