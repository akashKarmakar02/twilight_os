//! Software framebuffer rendering and presentation.
//!
//! Twilight's `/dev/fb0` mapping is the kernel-owned shadow buffer, not the
//! scan-out buffer. Drawing a frame therefore happens entirely off-screen;
//! `FBIOPAN_DISPLAY` copies the completed shadow buffer to video memory.
//!
//! [`Frame`] makes that boundary explicit: callers begin a frame, perform all
//! drawing through it, and present exactly once after composition is complete.

use core::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::fd::AsRawFd;
use std::ptr::{self, NonNull};

const FB_PATH: &str = "/dev/fb0";
const FBIOGET_VSCREENINFO: u64 = 0x4600;
const FBIOGET_FSCREENINFO: u64 = 0x4602;
const FBIOPAN_DISPLAY: u64 = 0x4606;

const PROT_READ: i32 = 0x1;
const PROT_WRITE: i32 = 0x2;
const MAP_SHARED: i32 = 0x01;
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;
const BYTES_PER_PIXEL: usize = 4;
const RED_OFFSET: u32 = 16;
const GREEN_OFFSET: u32 = 8;
const BLUE_OFFSET: u32 = 0;

#[repr(C)]
#[derive(Default)]
struct FbVarScreenInfo {
    xres: u32,
    yres: u32,
    bits_per_pixel: u32,
    red_offset: u32,
    green_offset: u32,
    blue_offset: u32,
}

#[repr(C)]
#[derive(Default)]
struct FbFixScreenInfo {
    line_length: u32,
    smem_len: u32,
}

unsafe extern "C" {
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: isize,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> i32;
    fn ioctl(fd: i32, request: u64, ...) -> i32;
}

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn union(self, other: Rect) -> Rect {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self
            .x
            .saturating_add(self.width)
            .max(other.x.saturating_add(other.width));
        let y1 = self
            .y
            .saturating_add(self.height)
            .max(other.y.saturating_add(other.height));
        Rect {
            x: x0,
            y: y0,
            width: x1.saturating_sub(x0),
            height: y1.saturating_sub(y0),
        }
    }
}

fn validate_geometry(
    width: usize,
    height: usize,
    stride: usize,
    map_bytes: usize,
) -> io::Result<()> {
    if width == 0 || height == 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "framebuffer dimensions must be nonzero",
        ));
    }

    let row_bytes = width
        .checked_mul(BYTES_PER_PIXEL)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "row size overflow"))?;
    if stride < row_bytes || !stride.is_multiple_of(BYTES_PER_PIXEL) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "framebuffer stride is invalid for 32-bit pixels",
        ));
    }

    let expected = height
        .checked_mul(stride)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "framebuffer size overflow"))?;
    if map_bytes < expected {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "framebuffer mapping is smaller than its geometry",
        ));
    }

    Ok(())
}

fn clip_rect(
    rect: Rect,
    output_width: i32,
    output_height: i32,
) -> Option<(usize, usize, usize, usize)> {
    let x0 = rect.x.clamp(0, output_width);
    let y0 = rect.y.clamp(0, output_height);
    let x1 = rect.x.saturating_add(rect.width).clamp(0, output_width);
    let y1 = rect.y.saturating_add(rect.height).clamp(0, output_height);

    (x1 > x0 && y1 > y0).then_some((x0 as usize, y0 as usize, x1 as usize, y1 as usize))
}

fn checked_range_end(offset: usize, len: usize, limit: usize) -> Option<usize> {
    offset.checked_add(len).filter(|&end| end <= limit)
}

/// The mapped Twilight framebuffer shadow buffer.
pub struct Framebuffer {
    file: File,
    width: usize,
    height: usize,
    stride: usize,
    map_bytes: usize,
    pixels: NonNull<u8>,
}

/// An in-progress frame that has not yet been presented.
#[must_use = "a composed frame must be presented"]
pub struct Frame<'a> {
    output: &'a mut Framebuffer,
}

impl Framebuffer {
    pub fn open() -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(FB_PATH)?;
        let fd = file.as_raw_fd();
        let mut var = FbVarScreenInfo::default();
        let mut fix = FbFixScreenInfo::default();

        // SAFETY: ioctl writes to a correctly laid-out framebuffer info struct
        // for the lifetime of this call, and `fd` is a live `/dev/fb0` handle.
        if unsafe { ioctl(fd, FBIOGET_VSCREENINFO, &mut var) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: Same argument as above for fixed framebuffer information.
        if unsafe { ioctl(fd, FBIOGET_FSCREENINFO, &mut fix) } < 0 {
            return Err(io::Error::last_os_error());
        }

        if var.xres == 0 || var.yres == 0 || var.bits_per_pixel != 32 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "unsupported framebuffer mode {}x{} {}bpp",
                    var.xres, var.yres, var.bits_per_pixel
                ),
            ));
        }
        if var.red_offset != RED_OFFSET
            || var.green_offset != GREEN_OFFSET
            || var.blue_offset != BLUE_OFFSET
        {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "unsupported framebuffer channel order r={} g={} b={}",
                    var.red_offset, var.green_offset, var.blue_offset
                ),
            ));
        }
        if i32::try_from(var.xres).is_err() || i32::try_from(var.yres).is_err() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "framebuffer dimensions {}x{} exceed i32 range",
                    var.xres, var.yres
                ),
            ));
        }

        let width = var.xres as usize;
        let height = var.yres as usize;
        let stride = fix.line_length as usize;
        let map_bytes = fix.smem_len as usize;
        validate_geometry(width, height, stride, map_bytes)?;

        // SAFETY: The framebuffer driver accepts a shared writable mapping of
        // `map_bytes`. The result is checked before being wrapped in NonNull
        // and remains owned by this object until Drop calls munmap exactly once.
        let mapped = unsafe {
            mmap(
                ptr::null_mut(),
                map_bytes,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };
        if mapped == MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let Some(pixels) = NonNull::new(mapped.cast::<u8>()) else {
            // SAFETY: `mapped` is a successful mapping that cannot be owned by
            // NonNull, so release it before returning the validation error.
            let _ = unsafe { munmap(mapped, map_bytes) };
            return Err(io::Error::new(
                ErrorKind::OutOfMemory,
                "framebuffer mmap returned null",
            ));
        };

        println!(
            "twland: framebuffer {}x{} stride={} bytes",
            width, height, stride
        );

        Ok(Self {
            file,
            width,
            height,
            stride,
            map_bytes,
            pixels,
        })
    }

    /// The output's dimensions in the integer format used by Wayland.
    pub fn geometry(&self) -> (i32, i32) {
        // `open` rejects dimensions outside the i32 range.
        (self.width as i32, self.height as i32)
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Start composing a new off-screen frame and initialize its background.
    pub fn begin_frame(&mut self, background: u32) -> io::Result<Frame<'_>> {
        let mut frame = Frame { output: self };
        frame.clear(background)?;
        Ok(frame)
    }

    /// Replace the displayed image with a solid color as one complete frame.
    pub fn clear_and_present(&mut self, color: u32) -> io::Result<()> {
        self.begin_frame(color)?.present()
    }

    fn present(&mut self) -> io::Result<()> {
        // Twilight's FBIOPAN_DISPLAY copies the completed mapped shadow buffer
        // to video memory. It is the only operation in this module that makes
        // an in-progress frame visible.
        // SAFETY: `file` is a live framebuffer fd. Twilight ignores the third
        // argument for FBIOPAN_DISPLAY, so a null pointer is the correct value.
        let result = unsafe {
            ioctl(
                self.file.as_raw_fd(),
                FBIOPAN_DISPLAY,
                ptr::null::<c_void>(),
            )
        };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Frame<'_> {
    pub fn width(&self) -> usize {
        self.output.width
    }

    pub fn height(&self) -> usize {
        self.output.height
    }

    pub fn stride(&self) -> usize {
        self.output.stride
    }

    fn clear(&mut self, color: u32) -> io::Result<()> {
        for y in 0..self.output.height {
            let row_offset = y
                .checked_mul(self.output.stride)
                .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "row offset overflow"))?;
            self.fill_pixels(row_offset, self.output.width, color)?;
        }
        Ok(())
    }

    pub fn fill_rect(&mut self, rect: Rect, color: u32) -> io::Result<()> {
        let Some((x0, y0, x1, y1)) =
            clip_rect(rect, self.output.width as i32, self.output.height as i32)
        else {
            return Ok(());
        };

        for y in y0..y1 {
            let offset = y
                .checked_mul(self.output.stride)
                .and_then(|row| row.checked_add(x0 * BYTES_PER_PIXEL))
                .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "fill offset overflow"))?;
            self.fill_pixels(offset, x1 - x0, color)?;
        }
        Ok(())
    }

    /// Copy one already-clipped byte span from raw memory into the off-screen frame.
    ///
    /// # Safety
    ///
    /// `source` must be readable for `len` bytes for the duration of this call.
    /// The source may alias the framebuffer because the copy is overlap-safe.
    pub unsafe fn copy_from_ptr(
        &mut self,
        offset: usize,
        source: *const u8,
        len: usize,
    ) -> io::Result<()> {
        if checked_range_end(offset, len, self.output.map_bytes).is_none() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "framebuffer copy out of bounds",
            ));
        }

        // SAFETY: The caller guarantees that `source` is readable for `len`
        // bytes. The checked destination range lies within the live mmap, and
        // `copy` permits overlap between the source and destination ranges.
        unsafe {
            ptr::copy(source, self.output.pixels.as_ptr().add(offset), len);
        }
        Ok(())
    }

    /// Publish this completed frame. Consuming `self` prevents further drawing
    /// through the same frame after it has become visible.
    pub fn present(self) -> io::Result<()> {
        self.output.present()
    }

    fn fill_pixels(&mut self, byte_offset: usize, count: usize, color: u32) -> io::Result<()> {
        let byte_len = count
            .checked_mul(BYTES_PER_PIXEL)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "fill size overflow"))?;
        if checked_range_end(byte_offset, byte_len, self.output.map_bytes).is_none() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "framebuffer fill out of bounds",
            ));
        }

        // SAFETY: The bounds checks prove the range lies within the mmap.
        // `byte_offset` is formed from a byte stride plus a 4-byte pixel offset,
        // so the pointer is aligned for u32 and the initialized slice is valid.
        let pixels = unsafe {
            std::slice::from_raw_parts_mut(
                self.output.pixels.as_ptr().add(byte_offset).cast::<u32>(),
                count,
            )
        };
        pixels.fill(color);
        Ok(())
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        // SAFETY: `pixels` and `map_bytes` describe the one successful mmap
        // owned by this object, and Drop runs once after all Frame borrows end.
        let _ = unsafe { munmap(self.pixels.as_ptr().cast(), self.map_bytes) };
    }
}

#[cfg(test)]
mod tests {
    use super::{Rect, checked_range_end, clip_rect, validate_geometry};

    #[test]
    fn rect_union_covers_both_inputs() {
        let combined = Rect {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        }
        .union(Rect {
            x: 5,
            y: 35,
            width: 50,
            height: 10,
        });

        assert_eq!(combined.x, 5);
        assert_eq!(combined.y, 20);
        assert_eq!(combined.width, 50);
        assert_eq!(combined.height, 40);
    }

    #[test]
    fn clip_rect_rejects_rectangles_outside_each_edge() {
        let outside = [
            Rect {
                x: -20,
                y: 10,
                width: 10,
                height: 10,
            },
            Rect {
                x: 100,
                y: 10,
                width: 10,
                height: 10,
            },
            Rect {
                x: 10,
                y: -20,
                width: 10,
                height: 10,
            },
            Rect {
                x: 10,
                y: 80,
                width: 10,
                height: 10,
            },
        ];

        for rect in outside {
            assert_eq!(clip_rect(rect, 100, 80), None);
        }
    }

    #[test]
    fn clip_rect_handles_edges_and_extreme_inputs() {
        assert_eq!(
            clip_rect(
                Rect {
                    x: -5,
                    y: 70,
                    width: 20,
                    height: 20,
                },
                100,
                80,
            ),
            Some((0, 70, 15, 80))
        );
        assert_eq!(
            clip_rect(
                Rect {
                    x: 10,
                    y: 10,
                    width: -1,
                    height: 10,
                },
                100,
                80,
            ),
            None
        );
        assert_eq!(
            clip_rect(
                Rect {
                    x: i32::MAX,
                    y: i32::MAX,
                    width: i32::MAX,
                    height: i32::MAX,
                },
                100,
                80,
            ),
            None
        );
    }

    #[test]
    fn validate_geometry_rejects_short_stride_and_mapping() {
        assert!(validate_geometry(100, 80, 399, 32_000).is_err());
        assert!(validate_geometry(100, 80, 400, 31_999).is_err());
        assert!(validate_geometry(100, 80, 400, 32_000).is_ok());
    }

    #[test]
    fn checked_range_end_rejects_overflow_and_out_of_bounds_ranges() {
        assert_eq!(checked_range_end(4, 4, 8), Some(8));
        assert_eq!(checked_range_end(4, 5, 8), None);
        assert_eq!(checked_range_end(usize::MAX, 1, usize::MAX), None);
    }
}
