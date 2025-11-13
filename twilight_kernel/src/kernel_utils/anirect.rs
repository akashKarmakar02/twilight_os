use crate::sys::framebuffer::FRAMEBUFFER;

pub fn main() {
    #[allow(static_mut_refs)]
    if let Some(fb) = unsafe { FRAMEBUFFER.get_mut() } {
        fb.animate_bouncing_rect(3000);
        fb.clear_buf(0x101010);
        fb.sync_full();
    }
}
