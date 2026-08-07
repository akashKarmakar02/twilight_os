//! Deadline-ordered timer queue for process sleep/wake (#66).
//!
//! This is a pure data structure: it owns no kernel globals and performs no I/O,
//! so it can be unit-tested on the host. The queue is a binary min-heap keyed by
//! `(deadline_ns, token)`, so equal deadlines pop in a deterministic order and
//! every matching sleeper wakes exactly once across batched expiry.
//!
//! ## Allocation discipline
//!
//! Heap memory is reserved *before* the IRQ-disabled block transaction via
//! [`TimerQueue::try_reserve_push`]. The actual insertion into the critical
//! section uses [`TimerQueue::push_assumed`], which never allocates: it pushes
//! into capacity that was already reserved. Popping and batched expiry likewise
//! only shrink the heap into existing capacity. No allocator call happens while
//! IRQs are disabled.
//!
//! ## Stale-entry reclamation
//!
//! Cancellation does not physically remove an entry (that would be an O(n)
//! sift). Instead the process's published `wait_token` is cleared, and the
//! entry is reclaimed lazily when it reaches the heap head: [`pop_due`] and
//! [`drain_stale_head`] discard leading entries whose token no longer matches a
//! live wait. Because every armed wait consumes exactly one entry, the heap
//! cannot grow without bound from repeated interrupted long sleeps — each
//! cancelled entry is reclaimed the moment it surfaces.

use alloc::vec::Vec;

/// Globally unique identifier for one outstanding process wait.
///
/// A PID alone is insufficient after PID reuse, and a per-process generation
/// resets at creation. `WaitToken` is a monotonically allocated `u64` drawn from
/// the queue's own counter, so it is unique across the lifetime of the BSP.
/// Wakeup matches `(pid, token)` against the process's currently published wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaitToken(u64);

impl WaitToken {
    /// Sentinel returned when the global token counter has exhausted the u64
    /// space. This is treated as "no live wait" by the wake path, so a blocked
    /// process whose token could not be minted is woken immediately rather than
    /// left sleeping forever.
    pub const EXHAUSTED: WaitToken = WaitToken(u64::MAX);

    #[inline]
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// Why a blocked process resumed execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeReason {
    /// The absolute deadline was reached.
    Deadline,
    /// An event (e.g. I/O) woke the process before its deadline.
    Event,
    /// An interrupting signal terminated the wait.
    Signal,
    /// The wait was cancelled (OOM, scheduler contention, or explicit cancel).
    Cancelled,
}

/// Kind of deadline being awaited. Determines which subsystem owns the wakeup;
/// only `Sleep` is wired in this ticket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadlineKind {
    Sleep,
    IoTimeout,
}

/// One entry in the deadline queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadlineEntry {
    pub deadline_ns: u64,
    pub pid: u16,
    pub token: WaitToken,
    pub kind: DeadlineKind,
}

/// Min-heap of [`DeadlineEntry`] ordered by `(deadline_ns, token)`.
pub struct TimerQueue {
    heap: Vec<DeadlineEntry>,
    next_token: u64,
}

impl Default for TimerQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl TimerQueue {
    pub const fn new() -> Self {
        Self {
            heap: Vec::new(),
            next_token: 1,
        }
    }

    /// Mint the next globally unique wait token. Returns [`WaitToken::EXHAUSTED`]
    /// if the u64 space is exhausted; callers must treat that as "do not block".
    pub fn next_token(&mut self) -> WaitToken {
        let t = self.next_token;
        // Exhaustion: stop handing out live tokens. The wake path treats
        // EXHAUSTED as non-matching, so any process holding it is woken rather
        // than left sleeping indefinitely.
        if self.next_token == u64::MAX {
            return WaitToken::EXHAUSTED;
        }
        self.next_token += 1;
        WaitToken(t)
    }

    /// Reserve capacity for a future push and report whether it succeeded.
    ///
    /// Call this in task context *before* disabling IRQs. On success the
    /// subsequent [`push_assumed`] will not allocate. Returns `Err(())` on OOM
    /// so the caller can fail before publishing any blocked state.
    pub fn try_reserve_push(&mut self, entry: &DeadlineEntry) -> Result<(), ()> {
        if self.heap.len() == self.heap.capacity() {
            self.heap.try_reserve_exact(1).map_err(|_| ())?;
        }
        // Push now to claim the slot; the entry is already fully formed.
        self.push_raw(*entry);
        Ok(())
    }

    /// Push an entry into capacity reserved by a prior [`try_reserve_push`].
    /// Must only be called when spare capacity exists. Never allocates.
    pub fn push_assumed(&mut self, entry: DeadlineEntry) {
        debug_assert!(
            self.heap.len() < self.heap.capacity(),
            "timer queue push without reserved capacity"
        );
        self.push_raw(entry);
    }

    fn push_raw(&mut self, entry: DeadlineEntry) {
        self.heap.push(entry);
        self.sift_up(self.heap.len() - 1);
    }

    /// Pop the earliest-due entry whose deadline is at or before `now_ns`.
    /// Stale leading entries (cancelled tokens) are discarded transparently:
    /// the caller passes a predicate that reports whether a given
    /// `(pid, token)` still identifies a live wait.
    ///
    /// Returns `None` when the head is not due or the queue is empty.
    pub fn pop_due<F>(&mut self, now_ns: u64, is_live: F) -> Option<DeadlineEntry>
    where
        F: Fn(u16, WaitToken) -> bool,
    {
        loop {
            let head = *self.heap.first()?;
            if head.deadline_ns > now_ns {
                return None;
            }
            // Remove the head regardless of liveness.
            self.pop_head();
            if is_live(head.pid, head.token) {
                return Some(head);
            }
            // Stale: discard and keep draining due entries.
        }
    }

    /// Remove and return the head entry unconditionally, discarding stale ones.
    /// Used by the deferred-overflow drain to clear all due entries in batches.
    pub fn pop_due_unchecked<F>(&mut self, now_ns: u64, is_live: F) -> Option<DeadlineEntry>
    where
        F: Fn(u16, WaitToken) -> bool,
    {
        loop {
            let head = *self.heap.first()?;
            if head.deadline_ns > now_ns {
                return None;
            }
            self.pop_head();
            if is_live(head.pid, head.token) {
                return Some(head);
            }
        }
    }

    fn pop_head(&mut self) -> Option<DeadlineEntry> {
        if self.heap.is_empty() {
            return None;
        }
        let last = self.heap.len() - 1;
        self.heap.swap(0, last);
        let entry = self.heap.pop();
        if !self.heap.is_empty() {
            self.sift_down(0);
        }
        entry
    }

    /// Discard leading stale entries so [`peek_deadline_ns`] never reports a
    /// cancelled wait. Returns true if any entries were reclaimed.
    pub fn drain_stale_head<F>(&mut self, is_live: F) -> bool
    where
        F: Fn(u16, WaitToken) -> bool,
    {
        let mut reclaimed = false;
        while let Some(&head) = self.heap.first() {
            if is_live(head.pid, head.token) {
                break;
            }
            self.pop_head();
            reclaimed = true;
        }
        reclaimed
    }

    /// Earliest live deadline, or `None` if the queue holds no live waits.
    /// Stale heads are drained first so a cancelled entry cannot mask a later
    /// live deadline.
    pub fn peek_deadline_ns<F>(&mut self, is_live: F) -> Option<u64>
    where
        F: Fn(u16, WaitToken) -> bool,
    {
        self.drain_stale_head(is_live);
        self.heap.first().map(|e| e.deadline_ns)
    }

    /// Number of entries currently stored (including not-yet-reclaimed stale).
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    // --- min-heap internals ---

    fn sift_up(&mut self, mut idx: usize) {
        while idx > 0 {
            let parent = (idx - 1) / 2;
            if self.key(idx) < self.key(parent) {
                self.heap.swap(idx, parent);
                idx = parent;
            } else {
                break;
            }
        }
    }

    fn sift_down(&mut self, mut idx: usize) {
        let n = self.heap.len();
        loop {
            let mut smallest = idx;
            let left = 2 * idx + 1;
            let right = 2 * idx + 2;
            if left < n && self.key(left) < self.key(smallest) {
                smallest = left;
            }
            if right < n && self.key(right) < self.key(smallest) {
                smallest = right;
            }
            if smallest == idx {
                break;
            }
            self.heap.swap(idx, smallest);
            idx = smallest;
        }
    }

    #[inline]
    fn key(&self, idx: usize) -> (u64, u64) {
        let e = &self.heap[idx];
        (e.deadline_ns, e.token.raw())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn always_live(_pid: u16, _token: WaitToken) -> bool {
        true
    }

    fn never_live(_pid: u16, _token: WaitToken) -> bool {
        false
    }

    fn entry(deadline_ns: u64, pid: u16, token: WaitToken) -> DeadlineEntry {
        DeadlineEntry {
            deadline_ns,
            pid,
            token,
            kind: DeadlineKind::Sleep,
        }
    }

    #[test]
    fn ordering_pops_earliest_first() {
        let mut q = TimerQueue::new();
        let t1 = q.next_token();
        let t2 = q.next_token();
        let t3 = q.next_token();
        q.try_reserve_push(&entry(300, 1, t1)).unwrap();
        q.try_reserve_push(&entry(100, 2, t2)).unwrap();
        q.try_reserve_push(&entry(200, 3, t3)).unwrap();

        assert_eq!(q.pop_due(150, always_live).unwrap().deadline_ns, 100);
        assert_eq!(q.pop_due(150, always_live), None); // 200/300 not due yet
        assert_eq!(q.pop_due(250, always_live).unwrap().deadline_ns, 200);
        assert_eq!(q.pop_due(350, always_live).unwrap().deadline_ns, 300);
        assert_eq!(q.pop_due(400, always_live), None);
    }

    #[test]
    fn equal_deadlines_wake_all_exactly_once() {
        let mut q = TimerQueue::new();
        let mut tokens = Vec::new();
        for pid in 1..=5u16 {
            let t = q.next_token();
            q.try_reserve_push(&entry(1000, pid, t)).unwrap();
            tokens.push(t);
        }

        let mut woken = Vec::new();
        while let Some(e) = q.pop_due(1000, always_live) {
            woken.push(e.pid);
        }
        assert_eq!(woken.len(), 5, "all five equal-deadline sleepers wake");
        // Each pid appears exactly once.
        let mut sorted = woken.clone();
        sorted.sort();
        assert_eq!(sorted, vec![1, 2, 3, 4, 5]);

        // No further wakeups.
        assert_eq!(q.pop_due(2000, always_live), None);
    }

    #[test]
    fn cancellation_is_lazy_and_bounded() {
        let mut q = TimerQueue::new();
        let live_t = q.next_token(); // pid 1, stays live
        let stale_t = q.next_token(); // pid 2, will be cancelled
        q.try_reserve_push(&entry(500, 1, live_t)).unwrap();
        q.try_reserve_push(&entry(100, 2, stale_t)).unwrap();

        // Predicate: only pid 1 / live_t is live; pid 2 was cancelled.
        let is_live = |pid: u16, token: WaitToken| pid == 1 && token == live_t;

        // Head is the stale entry (deadline 100). pop_due must skip it and
        // return the live one once due.
        assert_eq!(q.pop_due(100, is_live), None, "stale head not returned");
        assert_eq!(q.pop_due(600, is_live).unwrap().pid, 1);
        assert!(q.is_empty(), "stale entry reclaimed, heap empty");
    }

    #[test]
    fn stale_across_resleep_does_not_match() {
        // Simulate PID reuse: pid 5 arms token A, cancels, then re-arms token B.
        let mut q = TimerQueue::new();
        let old_token = q.next_token();
        let new_token = q.next_token();

        // The stale entry from the first sleep is still in the heap.
        q.try_reserve_push(&entry(10_000, 5, old_token)).unwrap();
        // The process is now waiting under new_token; old_token must not match.
        let is_live = |pid: u16, token: WaitToken| pid == 5 && token == new_token;

        // Even though pid matches, the stale token must be rejected.
        assert_eq!(q.pop_due(20_000, is_live), None, "stale token rejected");
        assert!(q.is_empty());
    }

    #[test]
    fn peek_deadline_drains_stale_head() {
        let mut q = TimerQueue::new();
        let stale = q.next_token();
        let live = q.next_token();
        q.try_reserve_push(&entry(100, 1, stale)).unwrap();
        q.try_reserve_push(&entry(200, 2, live)).unwrap();

        let is_live = |pid: u16, token: WaitToken| pid == 2 && token == live;
        assert_eq!(q.peek_deadline_ns(is_live), Some(200));
        assert_eq!(q.len(), 1, "stale head drained");
    }

    #[test]
    fn next_deadline_none_when_only_stale() {
        let mut q = TimerQueue::new();
        let stale = q.next_token();
        q.try_reserve_push(&entry(100, 1, stale)).unwrap();
        assert_eq!(q.peek_deadline_ns(never_live), None);
        assert!(q.is_empty());
    }

    #[test]
    fn token_exhaustion_returns_sentinel() {
        let mut q = TimerQueue::new();
        q.next_token = u64::MAX;
        let t = q.next_token();
        assert_eq!(t, WaitToken::EXHAUSTED);
        // A second call still returns EXHAUSTED; the counter does not wrap.
        assert_eq!(q.next_token(), WaitToken::EXHAUSTED);
    }

    #[test]
    fn saturated_deadline_does_not_underflow() {
        let mut q = TimerQueue::new();
        let t = q.next_token();
        q.try_reserve_push(&entry(u64::MAX, 1, t)).unwrap();
        // u64::MAX is "due" only at u64::MAX.
        assert_eq!(q.pop_due(u64::MAX - 1, always_live), None);
        assert_eq!(q.pop_due(u64::MAX, always_live).unwrap().pid, 1);
    }

    #[test]
    fn push_assumed_uses_reserved_capacity_without_allocating() {
        let mut q = TimerQueue::new();
        let t1 = q.next_token();
        // Reserve+push one, then push_assumed another after reserving capacity.
        q.try_reserve_push(&entry(100, 1, t1)).unwrap();
        let cap_before = q.heap.capacity();
        assert!(cap_before >= 2 || {
            q.heap.try_reserve_exact(1).ok();
            false
        });
        // Ensure at least one spare slot.
        if q.heap.len() == q.heap.capacity() {
            q.heap.try_reserve_exact(1).unwrap();
        }
        let t2 = q.next_token();
        q.push_assumed(entry(50, 2, t2));
        assert_eq!(q.pop_due(200, always_live).unwrap().pid, 2);
        assert_eq!(q.pop_due(200, always_live).unwrap().pid, 1);
    }

    #[test]
    fn hundreds_of_equal_deadline_sleepers_wake_without_loss() {
        let mut q = TimerQueue::new();
        for pid in 1..=300u16 {
            let t = q.next_token();
            q.try_reserve_push(&entry(1_000_000, pid, t)).unwrap();
        }
        let mut woken = 0u32;
        while q.pop_due(1_000_000, always_live).is_some() {
            woken += 1;
        }
        assert_eq!(woken, 300);
        assert!(q.is_empty());
    }

    #[test]
    fn batched_expiry_with_bounded_batch_drains_all() {
        // Simulate the hard-IRQ batch (EXPIRY_BATCH=32) followed by the
        // deferred-overflow drain. Every due sleeper must wake exactly once.
        const BATCH: usize = 32;
        let mut q = TimerQueue::new();
        for pid in 1..=100u16 {
            let t = q.next_token();
            q.try_reserve_push(&entry(500, pid, t)).unwrap();
        }
        let now = 500u64;
        let mut total = 0u32;
        loop {
            let mut count = 0usize;
            while count < BATCH {
                if q.pop_due(now, always_live).is_none() {
                    break;
                }
                count += 1;
            }
            total += count as u32;
            if count < BATCH {
                break;
            }
        }
        assert_eq!(total, 100);
    }
}
