#![no_std]

use limine::framebuffer::Framebuffer;
use crate::font::FONT_PSF;

mod font;


// fn put_pixel(framebuffer: &Framebuffer, x: usize, y: usize, color: u32) {
//     let offset = y * (framebuffer.pitch() as usize) / 4 + x;
//     unsafe {
//         framebuffer.addr().add(offset).cast::<u32>().write(color);
//     }
// }
//
// fn draw_char(framebuffer: &Framebuffer, x: usize, y: usize, c: u8, color: u32) {
//     let font = &FONT_PSF; // Extract font data here (depends on PSF format)
//     let glyph = &font[(c as usize) * 16..((c as usize) + 1) * 16]; // 16-byte character
//
//     for (row, &line) in glyph.iter().enumerate() {
//         for col in 0..8 {
//             if (line >> (7 - col)) & 1 != 0 {
//                 put_pixel(framebuffer, x + col, y + row, color);
//             }
//         }
//     }
// }
//
// pub fn print_text(framebuffer: &Framebuffer, x: usize, y: usize, text: &str, color: u32) {
//     for (i, c) in text.chars().enumerate() {
//         draw_char(framebuffer, x + i * 8, y, c as u8, color);
//     }
// }
pub fn print(
    framebuffer: &Framebuffer,
    x: usize,
    y: usize,
    color: u32,
    scale_w: usize, // Width scale factor
    scale_h: usize, // Height scale factor (increases height)
    ascii: u8,
) {
    let fb_ptr = framebuffer.addr();
    let pitch = framebuffer.pitch();

    // Get character bitmap
    let font_bitmap: &[u8; 8] = FONT_PSF.get(ascii as usize).unwrap();

    // Iterate over each row of the bitmap.
    for (row, &bitmap) in font_bitmap.iter().enumerate() {
        for h in 0..scale_h {
            for col in 0..8 {
                if (bitmap & (1 << col)) != 0 {
                    for dx in 0..scale_w {
                        let pixel_offset = ((y + row * scale_h + h) * pitch as usize) + ((x + col * scale_w + dx) * 4);
                        unsafe {
                            fb_ptr.offset(pixel_offset as isize).cast::<u32>().write(color);
                        }
                    }
                }
            }
        }
    }
}
