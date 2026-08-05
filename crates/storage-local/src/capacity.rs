use std::path::Path;

pub trait CapacityProbe: Send + Sync {
    /// Reports bytes available on the filesystem containing `path`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the filesystem cannot be inspected.
    fn free_bytes(&self, path: &Path) -> std::io::Result<u64>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemCapacityProbe;

impl CapacityProbe for SystemCapacityProbe {
    fn free_bytes(&self, path: &Path) -> std::io::Result<u64> {
        fs2::available_space(path)
    }
}
