//! `FrameBitmap`: one bit per 4 KiB physical frame, over caller-supplied
//! storage (brief M1-T2 step 3). Pure logic -- no static/global state, no
//! `unsafe`, no knowledge of Limine or the HHDM -- so it's exercised
//! directly by `#[test_case]`s over a small on-stack array, independent of
//! `pmm`'s real, RAM-sized bitmap.
//!
//! Bit convention: `1` = used/unavailable, `0` = free. A freshly
//! constructed bitmap's bits are whatever the backing storage already held
//! (see `new`'s docs); callers that need a specific starting state call
//! `fill_used`/`fill_free` right after.

const BITS_PER_WORD: usize = u64::BITS as usize;

/// A bitmap over frames `[base_frame, base_frame + frame_count)`, backed by
/// `words` (which must hold at least `frame_count.div_ceil(64)` of them).
/// "Frame" here just means "index `n`"; `pmm` is the only caller that
/// gives those indices physical-address meaning (`addr / FRAME_SIZE`).
pub struct FrameBitmap<'a> {
    words: &'a mut [u64],
    base_frame: u64,
    frame_count: usize,
}

impl<'a> FrameBitmap<'a> {
    /// Wraps `words` as a bitmap for `frame_count` frames starting at
    /// `base_frame`. Bits are left exactly as `words` already held --
    /// callers relying on a specific starting state call `fill_used`/
    /// `fill_free` right after this.
    ///
    /// # Panics
    /// If `words` is too small to hold `frame_count` bits.
    pub fn new(words: &'a mut [u64], base_frame: u64, frame_count: usize) -> Self {
        assert!(
            frame_count <= words.len() * BITS_PER_WORD,
            "FrameBitmap::new: {} word(s) can't hold {frame_count} frame(s)",
            words.len(),
        );
        Self { words, base_frame, frame_count }
    }

    pub fn base_frame(&self) -> u64 {
        self.base_frame
    }

    pub fn frame_count(&self) -> usize {
        self.frame_count
    }

    /// Marks every frame in range used. `pmm::init` starts here, then
    /// frees only the frames the Limine memory map calls USABLE.
    pub fn fill_used(&mut self) {
        self.words.fill(u64::MAX);
    }

    /// Marks every frame in range free.
    pub fn fill_free(&mut self) {
        self.words.fill(0);
    }

    /// Converts an absolute frame number to a `(word index, bit index)`
    /// pair into `self.words`.
    ///
    /// # Panics
    /// If `frame` is outside `[base_frame, base_frame + frame_count)`.
    fn word_and_bit(&self, frame: u64) -> (usize, u32) {
        let rel = frame.checked_sub(self.base_frame).expect("frame is below base_frame");
        let rel = rel as usize;
        assert!(rel < self.frame_count, "frame {frame} is out of range");
        (rel / BITS_PER_WORD, (rel % BITS_PER_WORD) as u32)
    }

    /// Marks `frame` used.
    ///
    /// # Panics
    /// If `frame` is out of range (see `word_and_bit`).
    pub fn set_used(&mut self, frame: u64) {
        let (word, bit) = self.word_and_bit(frame);
        self.words[word] |= 1u64 << bit;
    }

    /// Marks `frame` free.
    ///
    /// # Panics
    /// If `frame` is out of range (see `word_and_bit`).
    pub fn set_free(&mut self, frame: u64) {
        let (word, bit) = self.word_and_bit(frame);
        self.words[word] &= !(1u64 << bit);
    }

    /// Whether `frame` is currently marked used.
    ///
    /// # Panics
    /// If `frame` is out of range (see `word_and_bit`).
    pub fn is_used(&self, frame: u64) -> bool {
        let (word, bit) = self.word_and_bit(frame);
        self.words[word] & (1u64 << bit) != 0
    }

    /// Number of free (bit == 0) frames in `[base_frame, base_frame +
    /// frame_count)`. Any padding bits in the final word (beyond
    /// `frame_count`, when it isn't a multiple of 64) are never counted
    /// either way.
    pub fn count_free(&self) -> usize {
        let full_words = self.frame_count / BITS_PER_WORD;
        let mut free =
            self.words[..full_words].iter().map(|w| w.count_zeros() as usize).sum::<usize>();

        let remaining_bits = self.frame_count % BITS_PER_WORD;
        if remaining_bits > 0 {
            let mask = (1u64 << remaining_bits) - 1;
            free += (!self.words[full_words] & mask).count_ones() as usize;
        }
        free
    }

    /// Finds `n` consecutive free frames whose starting *absolute* frame
    /// number is a multiple of `align_frames`, scanning word-at-a-time
    /// (whole free/used 64-frame words are skipped in one comparison, and
    /// a used bit inside a word jumps the search straight past it, rather
    /// than testing every frame one at a time). Returns the absolute frame
    /// number of the run's first frame, or `None` if no such run exists.
    ///
    /// # Panics
    /// If `n` is 0 or `align_frames` is 0.
    pub fn find_free_run(&self, n: usize, align_frames: u64) -> Option<u64> {
        assert!(n > 0, "find_free_run: n must be at least 1");
        assert!(align_frames > 0, "find_free_run: align_frames must be at least 1");

        let mut start_rel: usize = 0;
        while start_rel + n <= self.frame_count {
            let abs = self.base_frame + start_rel as u64;
            if !abs.is_multiple_of(align_frames) {
                let next_abs = abs.div_ceil(align_frames) * align_frames;
                start_rel = (next_abs - self.base_frame) as usize;
                continue;
            }

            match self.first_used_rel(start_rel, n) {
                None => return Some(abs),
                Some(used_rel) => start_rel = used_rel + 1,
            }
        }
        None
    }

    /// Scans relative frames `[start, start + len)` word-at-a-time; returns
    /// the relative index of the first used frame in that range, or `None`
    /// if every frame in it is free.
    fn first_used_rel(&self, start: usize, len: usize) -> Option<usize> {
        let end = start + len;
        let mut i = start;
        while i < end {
            let word = i / BITS_PER_WORD;
            let bit = i % BITS_PER_WORD;
            let take = (BITS_PER_WORD - bit).min(end - i);
            let mask = if take == BITS_PER_WORD { u64::MAX } else { ((1u64 << take) - 1) << bit };

            let hit = self.words[word] & mask;
            if hit != 0 {
                return Some(word * BITS_PER_WORD + hit.trailing_zeros() as usize);
            }
            i += take;
        }
        None
    }
}
