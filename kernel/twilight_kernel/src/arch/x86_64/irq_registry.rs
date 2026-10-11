//! Fixed-capacity shared INTx registration. Owns callback insertion policy only;
//! the IDT module owns IRQ masking and invokes a copied list outside its lock.

/// Install a callback without replacing another device on the same IRQ line.
/// Repeated registration is idempotent; a full line fails without mutation.
pub fn insert<const N: usize>(line: &mut [Option<fn()>; N], handler: fn()) -> Result<(), ()> {
    if line
        .iter()
        .flatten()
        .any(|registered| core::ptr::fn_addr_eq(*registered, handler))
    {
        return Ok(());
    }
    *line.iter_mut().find(|slot| slot.is_none()).ok_or(())? = Some(handler);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::insert;
    use core::sync::atomic::{AtomicUsize, Ordering};
    static CALLED: AtomicUsize = AtomicUsize::new(0);
    fn nic() {
        CALLED.fetch_or(1, Ordering::SeqCst);
    }
    fn usb() {
        CALLED.fetch_or(2, Ordering::SeqCst);
    }
    fn extra() {
        CALLED.fetch_or(4, Ordering::SeqCst);
    }

    #[test]
    fn shared_handlers_survive_and_duplicates_do_not_use_slots() {
        let mut line = [None; 2];
        assert_eq!(insert(&mut line, nic), Ok(()));
        assert_eq!(insert(&mut line, nic), Ok(()));
        assert_eq!(insert(&mut line, usb), Ok(()));
        assert_eq!(insert(&mut line, extra), Err(()));
        for handler in line.into_iter().flatten() {
            handler();
        }
        assert_eq!(CALLED.load(Ordering::SeqCst), 3);
    }
}
