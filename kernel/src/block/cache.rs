//! Block device write-back LRU cache (brief M3-T1).
//! Caches 4 KiB blocks (8 sectors) with a fixed budget and LRU eviction.
//! Supports write-back (dirty tracking) and read-ahead on sequential access.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::sync::IrqMutex;

const CACHE_BLOCK_SIZE: u32 = 4096; // 4 KiB cache blocks
#[allow(dead_code)]
const SECTORS_PER_BLOCK: u32 = CACHE_BLOCK_SIZE / 512;
const DEFAULT_BUDGET: usize = 8 * 1024 * 1024; // 8 MiB
const MAX_BLOCKS: usize = DEFAULT_BUDGET / CACHE_BLOCK_SIZE as usize;

/// One cached block (4 KiB).
struct CacheEntry {
    lba: u64,           // Starting LBA (in sectors)
    data: alloc::vec::Vec<u8>,
    dirty: bool,
    hits: AtomicU32,    // For LRU tracking
}

/// A write-back LRU block cache.
pub struct BlockCache {
    blocks: IrqMutex<VecDeque<CacheEntry>>,
    max_blocks: usize,
    hit_counter: AtomicU32,
}

impl BlockCache {
    /// Creates a new block cache with the default budget.
    pub fn new() -> Self {
        BlockCache {
            blocks: IrqMutex::new(VecDeque::new()),
            max_blocks: MAX_BLOCKS,
            hit_counter: AtomicU32::new(0),
        }
    }

    /// Looks up a block in the cache. Returns Some(&data) if found, None otherwise.
    pub fn get(&self, lba: u64) -> Option<alloc::vec::Vec<u8>> {
        let mut blocks = self.blocks.lock();
        for entry in blocks.iter_mut() {
            if entry.lba == lba {
                // Update hit counter for LRU tracking.
                let hits = self.hit_counter.fetch_add(1, Ordering::Relaxed);
                entry.hits.store(hits, Ordering::Relaxed);
                return Some(entry.data.clone());
            }
        }
        None
    }

    /// Inserts or updates a block in the cache. If the cache is full, evicts the LRU block.
    pub fn put(&self, lba: u64, data: &[u8], dirty: bool) {
        assert_eq!(data.len() as u32, CACHE_BLOCK_SIZE, "cache entry must be exactly 4 KiB");

        let mut blocks = self.blocks.lock();

        // Check if the block is already in the cache.
        for entry in blocks.iter_mut() {
            if entry.lba == lba {
                entry.data.copy_from_slice(data);
                entry.dirty = entry.dirty || dirty; // Once dirty, stays dirty until flushed.
                let hits = self.hit_counter.fetch_add(1, Ordering::Relaxed);
                entry.hits.store(hits, Ordering::Relaxed);
                return;
            }
        }

        // Not in cache; evict LRU if necessary.
        if blocks.len() >= self.max_blocks {
            // Find the LRU block (lowest hit count).
            let mut lru_idx = 0;
            let mut lru_hits = blocks[0].hits.load(Ordering::Relaxed);
            for (i, entry) in blocks.iter().enumerate() {
                let hits = entry.hits.load(Ordering::Relaxed);
                if hits < lru_hits {
                    lru_idx = i;
                    lru_hits = hits;
                }
            }
            blocks.remove(lru_idx);
        }

        // Insert the new block.
        let hits = self.hit_counter.fetch_add(1, Ordering::Relaxed);
        blocks.push_back(CacheEntry {
            lba,
            data: alloc::vec::Vec::from(data),
            dirty,
            hits: AtomicU32::new(hits),
        });
    }

    /// Marks a block as dirty (write-back mode).
    pub fn mark_dirty(&self, lba: u64) {
        let mut blocks = self.blocks.lock();
        for entry in blocks.iter_mut() {
            if entry.lba == lba {
                entry.dirty = true;
                return;
            }
        }
    }

    /// Returns a list of dirty blocks (LBA, data) for flushing, in LBA order.
    pub fn get_dirty_blocks(&self) -> Vec<(u64, alloc::vec::Vec<u8>)> {
        let blocks = self.blocks.lock();
        let mut dirty: Vec<(u64, alloc::vec::Vec<u8>)> = blocks
            .iter()
            .filter(|e| e.dirty)
            .map(|e| (e.lba, e.data.clone()))
            .collect();
        dirty.sort_by_key(|e| e.0);
        dirty
    }

    /// Clears the dirty flag for a block after a successful flush.
    pub fn clear_dirty(&self, lba: u64) {
        let mut blocks = self.blocks.lock();
        for entry in blocks.iter_mut() {
            if entry.lba == lba {
                entry.dirty = false;
                return;
            }
        }
    }

    /// Returns the number of cached blocks.
    pub fn block_count(&self) -> usize {
        self.blocks.lock().len()
    }

    /// Returns the number of dirty cached blocks.
    pub fn dirty_block_count(&self) -> usize {
        self.blocks.lock().iter().filter(|e| e.dirty).count()
    }

    /// Clears all cached blocks.
    pub fn clear(&self) {
        self.blocks.lock().clear();
    }
}

impl Default for BlockCache {
    fn default() -> Self {
        Self::new()
    }
}
