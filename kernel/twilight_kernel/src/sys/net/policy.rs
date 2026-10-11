//! Pure deadline policy for deferred network work. Scheduler deadlines are
//! absolute nanoseconds; smoltcp delays are relative microseconds. This module
//! owns conversion and retry pacing, not clock reads, queues or hardware.

/// Clamp immediate work to a future retry boundary so a busy NIC cannot cause a
/// past-deadline syscall/worker spin. Saturation preserves ordering at u64::MAX.
pub fn protocol_deadline(now_ns: u64, delay_us: Option<u64>, retry_ns: u64) -> Option<u64> {
    delay_us.map(|delay| now_ns.saturating_add(delay.saturating_mul(1000).max(retry_ns)))
}

#[cfg(test)]
mod tests {
    use super::protocol_deadline;

    #[test]
    fn relative_delay_is_added_to_uptime() {
        assert_eq!(
            protocol_deadline(12_000_000_000, Some(5000), 1_000_000),
            Some(12_005_000_000)
        );
    }

    #[test]
    fn immediate_work_yields_instead_of_spinning() {
        assert_eq!(
            protocol_deadline(12_000_000_000, Some(0), 1_000_000),
            Some(12_001_000_000)
        );
    }

    #[test]
    fn idle_stack_does_not_add_periodic_wakes() {
        assert_eq!(protocol_deadline(12_000_000_000, None, 1_000_000), None);
    }

    #[test]
    fn conversion_and_addition_saturate() {
        assert_eq!(
            protocol_deadline(12_000_000_000, Some(u64::MAX), 1_000_000),
            Some(u64::MAX)
        );
        assert_eq!(
            protocol_deadline(u64::MAX - 1, Some(0), 1_000_000),
            Some(u64::MAX)
        );
    }
}

/// Prefer a vacant close slot, otherwise recycle the oldest eligible completed
/// connection. Active data/FIN teardown must never be selected by the predicate.
pub fn closing_slot<T: Copy, const N: usize>(
    slots: &[Option<(T, u64)>; N],
    mut reclaimable: impl FnMut(T) -> bool,
) -> Option<usize> {
    if let Some(index) = slots.iter().position(Option::is_none) {
        return Some(index);
    }
    slots
        .iter()
        .enumerate()
        .filter_map(|(index, slot)| {
            let (handle, deadline) = (*slot)?;
            reclaimable(handle).then_some((index, deadline))
        })
        .min_by_key(|(_, deadline)| *deadline)
        .map(|(index, _)| index)
}

#[cfg(test)]
mod close_tests {
    use super::closing_slot;

    #[test]
    fn free_slot_preserves_timewait() {
        assert_eq!(
            closing_slot(&[Some((1, 10)), None], |_| panic!("unneeded reclaim")),
            Some(1)
        );
    }

    #[test]
    fn pressure_reclaims_oldest_completed_connection_only() {
        let slots = [Some((1, 10)), Some((2, 30)), Some((3, 20))];
        assert_eq!(closing_slot(&slots, |handle| handle != 1), Some(2));
    }

    #[test]
    fn active_teardown_is_never_recycled() {
        assert_eq!(
            closing_slot(&[Some((1, 10)), Some((2, 20))], |_| false),
            None
        );
    }
}
