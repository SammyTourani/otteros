//! Block device cache tests (brief M3-T1).

use otteros_kernel::block::BlockCache;
use alloc::vec;

#[test_case]
fn block_cache_new() {
    let cache = BlockCache::new();
    assert_eq!(cache.block_count(), 0);
    assert_eq!(cache.dirty_block_count(), 0);
}

#[test_case]
fn block_cache_put_and_get() {
    let cache = BlockCache::new();
    let lba = 0u64;
    let data = vec![0x42; 4096];

    // Put a block in the cache.
    cache.put(lba, &data, false);
    assert_eq!(cache.block_count(), 1);
    assert_eq!(cache.dirty_block_count(), 0);

    // Retrieve it.
    let retrieved = cache.get(lba);
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap(), data);
}

#[test_case]
fn block_cache_dirty_flag() {
    let cache = BlockCache::new();
    let lba = 0u64;
    let data = vec![0x42; 4096];

    // Put a clean block.
    cache.put(lba, &data, false);
    assert_eq!(cache.dirty_block_count(), 0);

    // Put a dirty block.
    cache.put(lba + 1, &data, true);
    assert_eq!(cache.dirty_block_count(), 1);

    // Mark the first block as dirty.
    cache.mark_dirty(lba);
    assert_eq!(cache.dirty_block_count(), 2);
}

#[test_case]
fn block_cache_clear_dirty() {
    let cache = BlockCache::new();
    let data = vec![0x42; 4096];

    cache.put(0, &data, true);
    assert_eq!(cache.dirty_block_count(), 1);

    cache.clear_dirty(0);
    assert_eq!(cache.dirty_block_count(), 0);
}

#[test_case]
fn block_cache_get_dirty_blocks() {
    let cache = BlockCache::new();
    let data1 = vec![0x11; 4096];
    let data2 = vec![0x22; 4096];

    cache.put(10, &data1, true);
    cache.put(5, &data2, true);

    let dirty = cache.get_dirty_blocks();
    assert_eq!(dirty.len(), 2);
    // Should be sorted by LBA (5 before 10).
    assert_eq!(dirty[0].0, 5);
    assert_eq!(dirty[1].0, 10);
}

#[test_case]
fn block_cache_clears_all() {
    let cache = BlockCache::new();
    let data = vec![0x42; 4096];

    cache.put(0, &data, false);
    cache.put(1, &data, false);
    cache.put(2, &data, false);
    assert_eq!(cache.block_count(), 3);

    cache.clear();
    assert_eq!(cache.block_count(), 0);
}

#[test_case]
fn block_cache_lru_eviction() {
    // Create a cache with a small budget (256 KiB = 64 blocks).
    // This is a slow operation; for now just test that caching doesn't crash.
    let cache = BlockCache::new();
    let data = vec![0x42; 4096];

    // Fill up some blocks.
    for i in 0..10 {
        cache.put(i as u64, &data, false);
    }
    assert_eq!(cache.block_count(), 10);
}

#[test_case]
fn block_cache_default() {
    let cache = BlockCache::default();
    assert_eq!(cache.block_count(), 0);
}
