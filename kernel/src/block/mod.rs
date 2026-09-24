//! Block device abstraction and caching layer (brief M3-T1).

pub mod cache;

pub use cache::BlockCache;

/// A block device's basic interface.
pub trait BlockDevice: Send + Sync {
    /// Reads blocks from the device. `lba` is the starting logical block address.
    /// Returns true on success.
    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> bool;

    /// Writes blocks to the device.
    fn write_blocks(&self, lba: u64, data: &[u8]) -> bool;

    /// Flushes pending writes to the device.
    fn flush(&self) -> bool;

    /// Returns the device's block size (typically 512 or 4096).
    fn block_size(&self) -> u32;

    /// Returns the total number of blocks on the device.
    fn block_count(&self) -> u64;

    /// Returns the device's name (e.g. "vda", "sda").
    fn name(&self) -> &str;
}
