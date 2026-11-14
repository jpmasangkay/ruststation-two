//! Event-driven downcounting hardware scheduler.
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eetimers`):
//!   * BUSCLK runs at `~147.456 MHz`, which is half the speed of the Emotion Engine (`294.912 MHz`).
//! - `docs/PS2_BLUEPRINT.md` (§1.1 & §3):
//!   * Emotion Engine (EE): `294.912 MHz` (`1` EE cycle = base scheduler tick).
//!   * System Bus / DMAC / GS: `147.456 MHz` (`2` EE cycles per Bus cycle).
//!   * I/O Processor (IOP): `36.864 MHz` (`8` EE cycles per IOP cycle).

/// Maximum number of concurrently scheduled events in the inline priority queue.
/// Keeps the scheduler 100% free of heap allocations in the hot path (`AGENTS.md`).
pub const MAX_SCHEDULED_EVENTS: usize = 64;

/// Default maximum EE cycle batch before checking the scheduler (`docs/PS2_BLUEPRINT.md` §3).
pub const DEFAULT_MAX_EE_BATCH: u64 = 256;

/// Default IOP execution timeslice in EE cycles (`256` EE cycles = `32` IOP cycles).
pub const IOP_TIMESLICE_EE_CYCLES: u64 = 256;

// TODO(verify): Exact NTSC HBlank/VBlank scanline cycle counts (9,370 Bus cycles = 18,740 EE cycles,
// 240 active scanlines, 22.5 VBlank scanlines) are not tabulated in ps2tek.
/// NTSC HBlank interval in EE cycles (`9,370` Bus cycles * 2 = `18,740` EE cycles).
pub const NTSC_HBLANK_EE_CYCLES: u64 = 18_740;

/// NTSC active display duration before `VBlankStart` (`240` scanlines * `18,740` EE cycles).
pub const NTSC_ACTIVE_FRAME_EE_CYCLES: u64 = 240 * NTSC_HBLANK_EE_CYCLES;

/// NTSC VBlank duration between `VBlankStart` and `VBlankEnd` (`22.5` scanlines = `421,650` EE cycles).
pub const NTSC_VBLANK_EE_CYCLES: u64 = 421_650;

/// Hardware subsystem event types managed by the central [`Scheduler`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HardwareEventKind {
    /// Run the R3000A IOP for a scheduled timeslice (`IOP_TIMESLICE_EE_CYCLES`).
    IopTimeslice,
    /// Step active 128-bit EE DMAC channels.
    DmacTick,
    /// Complete a scheduled EE DMAC channel transfer (`channel_index`: `0..=9`).
    DmacChannelComplete(u8),
    /// EE hardware timer compare/overflow check (`timer_index`: `0..=3`).
    EeTimer(u8),
    /// IOP hardware timer compare/overflow check (`timer_index`: `0..=5`).
    IopTimer(u8),
    /// Horizontal blanking interval boundary.
    HBlank,
    /// Start of Vertical Blanking interval (`INTC` bit 2 + `GS_CSR.VSINT`).
    VBlankStart,
    /// End of Vertical Blanking interval (`INTC` bit 3).
    VBlankEnd,
}

/// A single scheduled hardware event tied to an absolute EE cycle timestamp.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ScheduledEvent {
    /// Absolute EE cycle count at which this event triggers.
    pub trigger_cycle: u64,
    /// Subsystem event discriminant.
    pub kind: HardwareEventKind,
}

impl Default for ScheduledEvent {
    fn default() -> Self {
        Self {
            trigger_cycle: u64::MAX,
            kind: HardwareEventKind::IopTimeslice,
        }
    }
}

/// Fixed-capacity, allocation-free downcounting event scheduler.
pub struct Scheduler {
    /// Current absolute Emotion Engine cycle counter (`294.912 MHz`).
    pub current_ee_cycle: u64,
    /// Cached `trigger_cycle` of the earliest pending event (`u64::MAX` when empty).
    pub next_event_cycle: u64,
    /// Events sorted in descending order of `trigger_cycle` so the earliest due event sits at `events[len - 1]`.
    events: [ScheduledEvent; MAX_SCHEDULED_EVENTS],
    /// Number of active events in `events[..len]`.
    len: usize,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    /// Create a new [`Scheduler`] initialized at cycle 0 with recurring `IopTimeslice`, `HBlank`,
    /// and `VBlankStart` events scheduled.
    pub fn new() -> Self {
        let mut sched = Self {
            current_ee_cycle: 0,
            next_event_cycle: u64::MAX,
            events: [ScheduledEvent::default(); MAX_SCHEDULED_EVENTS],
            len: 0,
        };
        sched.schedule(IOP_TIMESLICE_EE_CYCLES, HardwareEventKind::IopTimeslice);
        sched.schedule(NTSC_HBLANK_EE_CYCLES, HardwareEventKind::HBlank);
        sched.schedule(NTSC_ACTIVE_FRAME_EE_CYCLES, HardwareEventKind::VBlankStart);
        sched
    }

    /// Number of pending events currently in the queue.
    #[inline(always)]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if no events are currently scheduled.
    #[inline(always)]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Schedule an event `delta_ee_cycles` into the future relative to `current_ee_cycle`.
    ///
    /// Maintains descending `trigger_cycle` order in `self.events[..self.len]` so the earliest
    /// event is always at `self.events[self.len - 1]`.
    #[inline]
    pub fn schedule(&mut self, delta_ee_cycles: u64, kind: HardwareEventKind) {
        let trigger_cycle = self.current_ee_cycle.saturating_add(delta_ee_cycles);
        self.schedule_at(trigger_cycle, kind);
    }

    /// Schedule an event at an absolute `trigger_cycle`.
    #[inline]
    pub fn schedule_at(&mut self, trigger_cycle: u64, kind: HardwareEventKind) {
        if self.len >= MAX_SCHEDULED_EVENTS {
            tracing::error!(?kind, trigger_cycle, "Scheduler queue full; dropping event");
            return;
        }

        // Insert into `events[..len]` in descending order of `trigger_cycle`.
        // For equal `trigger_cycle`, newly scheduled events are placed before existing ones so FIFO order is preserved on pop.
        let mut idx = self.len;
        while idx > 0 && self.events[idx - 1].trigger_cycle <= trigger_cycle {
            self.events[idx] = self.events[idx - 1];
            idx -= 1;
        }
        self.events[idx] = ScheduledEvent {
            trigger_cycle,
            kind,
        };
        self.len += 1;
        self.refresh_next_event_cycle();
    }

    /// Remove all pending events matching `kind`.
    pub fn cancel_by_kind(&mut self, kind: HardwareEventKind) {
        let mut write_idx = 0;
        for read_idx in 0..self.len {
            if self.events[read_idx].kind != kind {
                self.events[write_idx] = self.events[read_idx];
                write_idx += 1;
            }
        }
        self.len = write_idx;
        self.refresh_next_event_cycle();
    }

    /// Advance the absolute EE cycle counter by `executed_ee_cycles`.
    #[inline(always)]
    pub fn advance(&mut self, executed_ee_cycles: u64) {
        self.current_ee_cycle = self.current_ee_cycle.wrapping_add(executed_ee_cycles);
    }

    /// Compute the number of EE cycles to execute in the next batch (`1..=max_batch`).
    #[inline(always)]
    pub fn next_batch_cycles(&self, max_batch: u64) -> i32 {
        let remaining = self.next_event_cycle.saturating_sub(self.current_ee_cycle);
        remaining.clamp(1, max_batch.max(1)) as i32
    }

    /// Pop the earliest scheduled event if its `trigger_cycle <= self.current_ee_cycle`.
    #[inline(always)]
    pub fn pop_due_event(&mut self) -> Option<ScheduledEvent> {
        if self.len == 0 {
            return None;
        }
        let last_idx = self.len - 1;
        let candidate = self.events[last_idx];
        if self.current_ee_cycle >= candidate.trigger_cycle {
            self.len = last_idx;
            self.refresh_next_event_cycle();
            Some(candidate)
        } else {
            None
        }
    }

    #[inline(always)]
    fn refresh_next_event_cycle(&mut self) {
        self.next_event_cycle = if self.len > 0 {
            self.events[self.len - 1].trigger_cycle
        } else {
            u64::MAX
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_orders_and_pops_events_earliest_first() {
        let mut sched = Scheduler {
            current_ee_cycle: 0,
            next_event_cycle: u64::MAX,
            events: [ScheduledEvent::default(); MAX_SCHEDULED_EVENTS],
            len: 0,
        };

        sched.schedule(300, HardwareEventKind::HBlank);
        sched.schedule(100, HardwareEventKind::DmacTick);
        sched.schedule(200, HardwareEventKind::IopTimeslice);
        sched.schedule(100, HardwareEventKind::EeTimer(0));

        assert_eq!(sched.next_event_cycle, 100);
        assert_eq!(sched.next_batch_cycles(256), 100);
        assert!(sched.pop_due_event().is_none());

        sched.advance(100);
        // FIFO tie-breaking for cycle 100: DmacTick was scheduled before EeTimer(0).
        assert_eq!(
            sched.pop_due_event(),
            Some(ScheduledEvent {
                trigger_cycle: 100,
                kind: HardwareEventKind::DmacTick,
            })
        );
        assert_eq!(
            sched.pop_due_event(),
            Some(ScheduledEvent {
                trigger_cycle: 100,
                kind: HardwareEventKind::EeTimer(0),
            })
        );
        assert!(sched.pop_due_event().is_none());
        assert_eq!(sched.next_event_cycle, 200);

        sched.cancel_by_kind(HardwareEventKind::IopTimeslice);
        assert_eq!(sched.next_event_cycle, 300);

        sched.advance(200);
        assert_eq!(
            sched.pop_due_event(),
            Some(ScheduledEvent {
                trigger_cycle: 300,
                kind: HardwareEventKind::HBlank,
            })
        );
        assert!(sched.is_empty());
        assert_eq!(sched.next_event_cycle, u64::MAX);
    }

    #[test]
    fn scheduler_same_cycle_fifo_tie_breaking() {
        let mut sched = Scheduler {
            current_ee_cycle: 0,
            next_event_cycle: u64::MAX,
            events: [ScheduledEvent::default(); MAX_SCHEDULED_EVENTS],
            len: 0,
        };

        sched.schedule(50, HardwareEventKind::EeTimer(0));
        sched.schedule(50, HardwareEventKind::EeTimer(1));
        sched.schedule(50, HardwareEventKind::EeTimer(2));
        sched.schedule(50, HardwareEventKind::EeTimer(3));

        sched.advance(50);
        assert_eq!(
            sched.pop_due_event(),
            Some(ScheduledEvent {
                trigger_cycle: 50,
                kind: HardwareEventKind::EeTimer(0),
            })
        );
        assert_eq!(
            sched.pop_due_event(),
            Some(ScheduledEvent {
                trigger_cycle: 50,
                kind: HardwareEventKind::EeTimer(1),
            })
        );
        assert_eq!(
            sched.pop_due_event(),
            Some(ScheduledEvent {
                trigger_cycle: 50,
                kind: HardwareEventKind::EeTimer(2),
            })
        );
        assert_eq!(
            sched.pop_due_event(),
            Some(ScheduledEvent {
                trigger_cycle: 50,
                kind: HardwareEventKind::EeTimer(3),
            })
        );
        assert!(sched.pop_due_event().is_none());
    }

    #[test]
    fn scheduler_event_cancellation() {
        let mut sched = Scheduler {
            current_ee_cycle: 0,
            next_event_cycle: u64::MAX,
            events: [ScheduledEvent::default(); MAX_SCHEDULED_EVENTS],
            len: 0,
        };

        sched.schedule(80, HardwareEventKind::DmacTick);
        sched.schedule(120, HardwareEventKind::EeTimer(1));
        sched.schedule(160, HardwareEventKind::DmacTick);
        sched.schedule(240, HardwareEventKind::HBlank);

        assert_eq!(sched.next_event_cycle, 80);
        sched.cancel_by_kind(HardwareEventKind::DmacTick);
        assert_eq!(sched.len(), 2);
        assert_eq!(sched.next_event_cycle, 120);

        sched.advance(200);
        assert_eq!(
            sched.pop_due_event(),
            Some(ScheduledEvent {
                trigger_cycle: 120,
                kind: HardwareEventKind::EeTimer(1),
            })
        );
        assert!(sched.pop_due_event().is_none());
        assert_eq!(sched.next_event_cycle, 240);
    }
}
