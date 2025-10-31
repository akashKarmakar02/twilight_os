use crate::arch::x86_64::io::delay;
use crate::driver::timer::pit::uptime;
use crate::sys::fs::vfs::{BlockDev, VfsNodeOps};
use alloc::vec;
use alloc::vec::Vec;
use limine::framebuffer::Framebuffer;
use spin::Once;

#[allow(static_mut_refs)]
pub static mut FRAMEBUFFER: Once<TwilightFrameBuffer> = Once::new();
pub struct TwilightFrameBuffer {
    pub video_buf: &'static mut [u32],
    pub height: u64,
    pub width: u64,
    pub pitch: u64,
    pub pixel_buf: Vec<u32>,
}

impl TwilightFrameBuffer {
    pub fn new(fb: &Framebuffer) -> Self {
        let width = fb.width() as usize;
        let height = fb.height() as usize;
        let bits_per_pixel = fb.bpp() as usize;
        let byte_len = width * height * (bits_per_pixel / 8);

        let framebuffer = unsafe {
            core::slice::from_raw_parts_mut::<u32>(
                fb.addr().cast::<u32>(),
                byte_len / 4,
            )
        };

        assert_eq!(framebuffer.len(), (width * height));

        Self {
            video_buf: framebuffer,
            width: width as u64,
            height: height as u64,
            pitch: fb.pitch(),                    // bytes per scanline in VRAM
            pixel_buf: vec![0; (width * height) as usize], // compact RGBx buffer (u32 per pixel)
        }
    }

    pub fn width(&self) -> u64 {
        self.width
    }
    pub fn height(&self) -> u64 {
        self.height
    }
    pub fn pitch(&self) -> u64 {
        self.pitch
    }

    #[inline]
    fn idx(&self, x: u64, y: u64) -> usize {
        (y * self.width + x) as usize
    }

    pub fn set_pixel(&mut self, x: u64, y: u64, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let i = self.idx(x, y);
        self.pixel_buf[i] = color;
    }

    pub fn clear_buf(&mut self, color: u32) {
        // fill CPU-side backbuffer only
        self.pixel_buf.fill(color);
    }

    pub fn fill_rect_buf(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) {
        if w <= 0 || h <= 0 {
            return;
        }
        // clip to screen
        let x0 = x.max(0) as u64;
        let y0 = y.max(0) as u64;
        let x1 = (x + w).clamp(0, self.width as i64) as u64;
        let y1 = (y + h).clamp(0, self.height as i64) as u64;

        for yy in y0..y1 {
            let row_start = self.idx(x0, yy);
            let row_end = self.idx(x1 - 1, yy) + 1;
            self.pixel_buf[row_start..row_end].fill(color);
        }
    }

    pub fn sync_full(&mut self) {
        self.video_buf.copy_from_slice(self.pixel_buf.as_slice());
    }

    // Optional: keep sync_partial but fix bounds (end is exclusive)
    pub fn sync_partial(&mut self, pixel_start: u64, pixel_count: u64) {
        let total = self.pixel_buf.len() as u64;
        let start = pixel_start.min(total);
        let end = (start + pixel_count).min(total);
        if start >= end {
            return;
        }

        self.video_buf[(start as usize)..(end as usize)].copy_from_slice(&self.pixel_buf.as_slice()[(start as usize)..(end as usize)]);
    }

    pub fn scroll_up(&mut self, lines: u64, fill_color: u32) {
        let h = self.height as usize;
        let w = self.width as usize;
        let scroll = lines.min(self.height) as usize;

        if scroll == 0 {
            return;
        }

        if scroll >= h {
            self.clear_buf(fill_color);
            return;
        }

        let src_start = scroll * w;
        let dst_start = 0;

        // SAFER copy: use a manual copy to avoid overlap issues with copy_within
        for i in 0..(h - scroll) * w {
            let val = self.pixel_buf[src_start + i];
            if val != 0 {}
            self.pixel_buf[dst_start + i] = self.pixel_buf[src_start + i];
        }

        // Fill the bottom cleared area
        let fill_start = (h - scroll) * w;
        self.pixel_buf[fill_start..].fill(fill_color);
    }

    /// Scroll the framebuffer content down by `lines` pixels.
    /// The emptied top region is filled with `fill_color`.
    pub fn scroll_down(&mut self, lines: u64, fill_color: u32) {
        if lines == 0 || lines >= self.height {
            self.clear_buf(fill_color);
            return;
        }

        let w = self.width as usize;
        let h = self.height as usize;
        let scroll = lines as usize;

        let src_start = 0;
        let dst_start = scroll * w;

        // Copy downwards safely to prevent overlap corruption
        for i in 0..(h - scroll) * w {
            self.pixel_buf[dst_start + i] = self.pixel_buf[src_start + i];
        }
        // Fill top area
        self.pixel_buf[0..(scroll * w)].fill(fill_color);
    }

    pub fn animate_bouncing_rect(&mut self, duration_ms: u64) {
        // Tweakables
        let bg: u32 = 0x111111;
        let fg: u32 = 0x35a7ff;
        let w: i64 = 50;
        let h: i64 = 30;
        let mut x: i64 = 10;
        let mut y: i64 = 10;
        let mut vx: i64 = 3;
        let mut vy: i64 = 2;

        let fps_target = 144u64; // true 60 FPS
        let frame_ms = 1000 / fps_target; // 16 ms

        self.clear_buf(bg);
        self.sync_full();

        let start_ms = uptime_ms();
        let mut next_tick = start_ms + frame_ms;

        // Frame loop (strict fixed-step)
        while uptime_ms().saturating_sub(start_ms) < duration_ms {
            let now = uptime_ms();

            // sleep if early (leave 1ms margin)
            if now + 2 < next_tick {
                let to_sleep = (next_tick - now).saturating_sub(1) as usize;
                if to_sleep > 0 {
                    delay(to_sleep);
                }
                continue;
            }

            // process the number of frames we owe
            let mut did_frame = false;
            while next_tick <= now {
                // --- build frame in backbuffer ---
                self.clear_buf(bg);
                x += vx;
                y += vy;

                // bounce
                if x <= 0 {
                    x = 0;
                    vx = -vx;
                }
                if y <= 0 {
                    y = 0;
                    vy = -vy;
                }
                if x + w >= self.width as i64 {
                    x = (self.width as i64 - w).max(0);
                    vx = -vx;
                }
                if y + h >= self.height as i64 {
                    y = (self.height as i64 - h).max(0);
                    vy = -vy;
                }

                self.fill_rect_buf(x, y, w, h, fg);

                // push to VRAM once per produced frame
                self.sync_full();

                next_tick += frame_ms;
                did_frame = true;
            }
            // If we got massively behind (e.g., breakpoint), resync gently
            if !did_frame && now > next_tick + 8 * frame_ms {
                next_tick = now + frame_ms;
            }
        }
    }

    pub fn animate_boot_screen(&mut self, duration_ms: u64) {
        // Palette
        let bg: u32 = 0x0c0e12; // deep charcoal
        let fg: u32 = 0x35a7ff; // accent cyan
        let fg_dim: u32 = 0x1f6aa8; // dim accent
        let frame: u32 = 0x0f131a; // widget frame
        let bar_bg: u32 = 0x141821; // bar track
        let dot_idle: u32 = 0x17202a; // idle dots

        let w = self.width as i64;
        let h = self.height as i64;

        // Layout (scales with resolution)
        let logo_size = (h.min(w) / 6).max(60); // px
        let logo_w = logo_size;
        let logo_h = (logo_size as f32 * 0.9) as i64;

        let bar_w = (w as f32 * 0.40) as i64; // 40% of width
        let bar_h = (h as f32 * 0.022) as i64;
        let bar_gap = (h as f32 * 0.04) as i64; // gap under logo

        let center_x = w / 2;
        let center_y = h / 2;

        let logo_x = center_x - (logo_w / 2);
        let logo_y = center_y - (logo_h / 2) - bar_gap;

        let bar_x = center_x - (bar_w / 2);
        let bar_y = center_y + (logo_h / 2) - (bar_h / 2);

        let dots_y = bar_y + bar_h + (bar_h / 1.max(1)) + 6;
        let dot_size = (bar_h as f32 * 0.55) as i64;
        let dot_gap = dot_size + 6;
        let dots_start_x = center_x - (dot_size * 3 + dot_gap * 2) / 2;

        // Timing
        let fps_target = 60u64;
        let frame_ms = 1000 / fps_target;

        self.clear_buf(bg);
        self.sync_full();

        let t0 = uptime_ms();
        let mut next_tick = t0 + frame_ms;

        // helper: logo drawing (block "T")
        let draw_logo = |fb: &mut Self, x: i64, y: i64, w: i64, h: i64, color: u32| {
            // Outer “badge” with subtle frame
            let pad = (w as f32 * 0.08) as i64;
            fb.fill_rect_buf(x - pad, y - pad, w + 2 * pad, h + 2 * pad, frame);

            // Background inside badge
            fb.fill_rect_buf(x, y, w, h, bg);

            // The “T”
            let stem_w = (w as f32 * 0.22) as i64;
            let cap_h = (h as f32 * 0.22) as i64;

            // Cap
            fb.fill_rect_buf(x, y, w, cap_h, color);
            // Stem
            fb.fill_rect_buf(x + (w - stem_w) / 2, y, stem_w, h, color);

            // Accent underline
            let ul_h = 2.max((h as f32 * 0.03) as i64);
            fb.fill_rect_buf(x, y + h + (ul_h * 2), w, ul_h, fg_dim);
        };

        // helper: progress bar (track + fill + thin frame)
        let draw_progress = |fb: &mut Self, x: i64, y: i64, w: i64, h: i64, pct01: f32| {
            // Frame
            fb.fill_rect_buf(x - 2, y - 2, w + 4, h + 4, frame);
            // Track
            fb.fill_rect_buf(x, y, w, h, bar_bg);
            // Fill
            let fill_w = ((w as f32) * pct01.clamp(0.0, 1.0)) as i64;
            if fill_w > 0 {
                fb.fill_rect_buf(x, y, fill_w, h, fg);
            }
            // Subtle inner gloss
            let gloss_h = (h as f32 * 0.30) as i64;
            fb.fill_rect_buf(x, y, w, gloss_h, 0x0e1218);
        };

        // helper: 3 pulsing dots
        let draw_dots = |fb: &mut Self, t_ms: u64| {
            for i in 0..3 {
                let phase = ((t_ms / 200) % 3) as i64;
                let on = i as i64 == phase;
                let dx = dots_start_x + i as i64 * (dot_size + dot_gap);
                let color = if on { fg } else { dot_idle };
                fb.fill_rect_buf(dx, dots_y, dot_size, dot_size, color);
            }
        };

        // Animation loop
        while uptime_ms().saturating_sub(t0) < duration_ms {
            let now = uptime_ms();

            // sleep if we're early (leave ~1ms margin)
            if now + 2 < next_tick {
                let to_sleep = (next_tick - now).saturating_sub(1) as usize;
                if to_sleep > 0 {
                    delay(to_sleep);
                }
                continue;
            }

            // catch up by whole frames
            while next_tick <= now {
                let elapsed = next_tick.saturating_sub(t0).min(duration_ms) as f32;
                let total = duration_ms as f32;

                // Ease the progress a bit (smoothstep)
                let tlin = if total > 0.0 { elapsed / total } else { 1.0 };
                let t = tlin * tlin * (3.0 - 2.0 * tlin); // smoothstep 0..1

                // Build frame
                self.clear_buf(bg);

                // Logo
                draw_logo(self, logo_x, logo_y, logo_w, logo_h, fg);

                // Progress bar
                draw_progress(self, bar_x, bar_y, bar_w, bar_h, t);

                // Pulsing dots (status)
                draw_dots(self, next_tick - t0);

                // Commit
                self.sync_full();

                next_tick += frame_ms;
            }

            // Massive delay safeguard (e.g., debugger pause)
            if now > next_tick + 8 * frame_ms {
                next_tick = now + frame_ms;
            }
        }

        // Final frame (100%)
        self.clear_buf(bg);
        draw_logo(self, logo_x, logo_y, logo_w, logo_h, fg);
        draw_progress(self, bar_x, bar_y, bar_w, bar_h, 1.0);
        // dots steady “on”
        for i in 0..3 {
            let dx = dots_start_x + i as i64 * (dot_size + dot_gap);
            self.fill_rect_buf(dx, dots_y, dot_size, dot_size, fg);
        }
        self.sync_full();
    }
}

fn uptime_ms() -> u64 {
    (uptime() * 1000.0) as u64
}

impl VfsNodeOps for TwilightFrameBuffer {
    fn read(&self, _device: &mut BlockDev, _offset: usize, _buffer: &mut [u8]) -> Result<Vec<u8>, ()> {
        Err(())
    }

    fn write(&mut self, _device: &mut BlockDev, offset: usize, buffer: &[u8]) -> Result<(), ()> {
        if buffer.len() % 4 != 0 {
            return Err(());
        }

        let buf_u32 = unsafe {
            core::slice::from_raw_parts(buffer.as_ptr() as *const u32, buffer.len() / 4)
        };

        self.video_buf[offset..(offset) + buf_u32.len()].copy_from_slice(buf_u32);
        self.pixel_buf[offset..(offset) + buf_u32.len()].copy_from_slice(buf_u32);

        Ok(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }

    fn ioctl(&mut self, _device: &mut BlockDev, _cmd: u64, _arg: usize) -> Result<i64, ()> {
        Ok(-1)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}

unsafe impl Send for TwilightFrameBuffer {}

unsafe impl Sync for TwilightFrameBuffer {}

pub fn init_framebuffer(fb: &Framebuffer) {
    #[allow(static_mut_refs)]
    unsafe {
        FRAMEBUFFER.call_once(|| TwilightFrameBuffer::new(fb));
    }
}

pub fn get_pitch() -> u64 {
    #[allow(static_mut_refs)]
    unsafe {
        FRAMEBUFFER.get().unwrap().pitch()
    }
}

pub fn convert_color(color: u32) -> [u8; 4] {
    let rgba: [u8; 4] = [
        (color & 0xFF) as u8,         // Red (was Blue)
        ((color >> 8) & 0xFF) as u8,  // Green (unchanged)
        ((color >> 16) & 0xFF) as u8, // Blue (was Red)
        255,                          // Alpha (fully opaque)
    ];

    rgba
}

pub fn get_framebuffer() -> &'static TwilightFrameBuffer {
    #[allow(static_mut_refs)]
    unsafe {
        FRAMEBUFFER.get().unwrap()
    }
}

pub fn get_framebuffer_mut() -> &'static mut TwilightFrameBuffer {
    #[allow(static_mut_refs)]
    unsafe {
        FRAMEBUFFER.get_mut().unwrap()
    }
}
