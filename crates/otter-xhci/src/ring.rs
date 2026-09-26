//! Ring buffers for xHCI 1.2: Producer (command/transfer) and Event rings.
//! Section 4.9: Ring Data Structures

use core::sync::atomic::{AtomicU32, Ordering};
use crate::trb::Trb;

/// Producer ring (command or transfer ring).
pub struct ProducerRing {
    base_phys: u64,
    size: usize,      // Total TRB slots (including the Link TRB slot)
    enqueue: u64,     // Current enqueue pointer (physical address)
    cycle: bool,      // Current producer cycle bit
}

impl ProducerRing {
    /// Create a new producer ring.
    pub fn new(base_phys: u64, size: usize) -> Self {
        ProducerRing {
            base_phys,
            size,
            enqueue: base_phys,
            cycle: true,
        }
    }

    /// Get the current enqueue pointer.
    pub fn enqueue_pointer(&self) -> u64 {
        self.enqueue
    }

    /// Get the current producer cycle bit.
    pub fn cycle(&self) -> bool {
        self.cycle
    }

    /// Push a TRB onto the ring. Returns the address where it was written.
    pub fn push(&mut self, mem: &[AtomicU32], trb: Trb) -> u64 {
        let index = ((self.enqueue - self.base_phys) / 16) as usize;
        let returned_addr = self.enqueue;

        // Check if we're at the link slot
        if index >= self.size - 1 {
            // We're at or past the link slot; this shouldn't happen on normal push,
            // but if it does, write the Link TRB first, then recurse
            let prev_chain = (trb.0[3] >> 4) & 1;
            let link_d3 = (6u32 << 10)           // TRB type = Link
                | (1u32 << 1)                   // Toggle Cycle
                | (prev_chain << 4)             // Chain bit copied from previous TRB
                | (self.cycle as u32);          // Cycle bit

            let link_mem_index = (self.size - 1) * 4;
            mem[link_mem_index].store((self.base_phys & 0xFFFF_FFFF) as u32, Ordering::Relaxed);
            mem[link_mem_index + 1].store((self.base_phys >> 32) as u32, Ordering::Relaxed);
            mem[link_mem_index + 2].store(0, Ordering::Relaxed);
            mem[link_mem_index + 3].store(link_d3, Ordering::Release);

            // Wrap around and toggle cycle
            self.enqueue = self.base_phys;
            self.cycle = !self.cycle;

            // Now push the actual TRB at the new enqueue position
            self.push(mem, trb)
        } else {
            // Write the TRB with the ring's cycle bit
            let mut trb_to_write = trb;
            trb_to_write.0[3] = (trb_to_write.0[3] & !1) | (self.cycle as u32);

            let mem_index = index * 4;
            mem[mem_index].store(trb_to_write.0[0], Ordering::Relaxed);
            mem[mem_index + 1].store(trb_to_write.0[1], Ordering::Relaxed);
            mem[mem_index + 2].store(trb_to_write.0[2], Ordering::Relaxed);
            mem[mem_index + 3].store(trb_to_write.0[3], Ordering::Release);

            // Advance enqueue pointer
            self.enqueue += 16;

            // Check if the next slot is the link slot and write it if so
            let next_index = ((self.enqueue - self.base_phys) / 16) as usize;
            if next_index >= self.size - 1 {
                let link_d3 = (6u32 << 10)                          // TRB type = Link
                    | (1u32 << 1)                                  // Toggle Cycle
                    | ((trb_to_write.0[3] >> 4) & 1) << 4         // Chain bit copied from TRB we just wrote
                    | (self.cycle as u32);                         // Cycle bit

                let link_mem_index = (self.size - 1) * 4;
                mem[link_mem_index].store((self.base_phys & 0xFFFF_FFFF) as u32, Ordering::Relaxed);
                mem[link_mem_index + 1].store((self.base_phys >> 32) as u32, Ordering::Relaxed);
                mem[link_mem_index + 2].store(0, Ordering::Relaxed);
                mem[link_mem_index + 3].store(link_d3, Ordering::Release);

                // Wrap around and toggle cycle
                self.enqueue = self.base_phys;
                self.cycle = !self.cycle;
            }

            returned_addr
        }
    }

    /// Count how many TRBs can be pushed before enqueue would reach dequeue, keeping one slot empty.
    pub fn free_slots(&self, dequeue_phys: u64) -> usize {
        let enqueue_idx = ((self.enqueue - self.base_phys) / 16) as usize;
        let dequeue_idx = ((dequeue_phys - self.base_phys) / 16) as usize;

        if dequeue_idx > enqueue_idx {
            dequeue_idx - enqueue_idx - 1
        } else if dequeue_idx == enqueue_idx {
            self.size - 2  // One slot for link, one kept empty
        } else {
            (self.size - enqueue_idx) + dequeue_idx - 2
        }
    }
}

/// Event ring (one segment).
pub struct EventRing {
    base_phys: u64,
    size: usize,      // Number of TRB slots
    dequeue: u64,     // Current dequeue pointer (physical address)
    cycle: bool,      // Current consumer cycle bit
}

impl EventRing {
    /// Create a new event ring.
    pub fn new(base_phys: u64, size: usize) -> Self {
        EventRing {
            base_phys,
            size,
            dequeue: base_phys,
            cycle: true,
        }
    }

    /// Get the current dequeue pointer.
    pub fn dequeue_pointer(&self) -> u64 {
        self.dequeue
    }

    /// Get the current ERDP (Event Ring Dequeue Pointer) register value with EHB set.
    pub fn erdp(&self) -> u64 {
        self.dequeue | 0x8
    }

    /// Get the Event Ring Segment Table (ERST) entry for this segment.
    pub fn erst_entry(&self) -> [u32; 4] {
        [
            (self.base_phys & 0xFFFF_FFFF) as u32,
            (self.base_phys >> 32) as u32,
            self.size as u32,
            0,
        ]
    }

    /// Pop an event TRB if one is available.
    pub fn pop(&mut self, mem: &[AtomicU32]) -> Option<Trb> {
        let index = ((self.dequeue - self.base_phys) / 16) as usize;
        let mem_index = index * 4;

        // Read dword 3 first with Acquire ordering
        let d3 = mem[mem_index + 3].load(Ordering::Acquire);
        let event_cycle = (d3 & 1) != 0;

        // Check if this TRB is ready (cycle bit matches consumer cycle)
        if event_cycle != self.cycle {
            return None;
        }

        // Read the other dwords
        let d0 = mem[mem_index].load(Ordering::Relaxed);
        let d1 = mem[mem_index + 1].load(Ordering::Relaxed);
        let d2 = mem[mem_index + 2].load(Ordering::Relaxed);

        let trb = Trb([d0, d1, d2, d3]);

        // Advance dequeue pointer
        self.dequeue += 16;
        if self.dequeue >= self.base_phys + (self.size as u64 * 16) {
            self.dequeue = self.base_phys;
            self.cycle = !self.cycle;
        }

        Some(trb)
    }
}
