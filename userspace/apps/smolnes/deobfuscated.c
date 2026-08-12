// smolnes — Twilight OS framebuffer port.
//
// SPDX-License-Identifier: MIT
// Copyright (c) 2022 Ben Smith (upstream smolnes)
// Copyright (c) 2024 Twilight OS contributors (framebuffer/input port)
//
// This is the upstream smolnes NES emulator (deobfuscated.c) with its SDL2
// backend replaced by a direct /dev/fb0 + /dev/input/event0 backend, matching
// the pattern used by the fbdoom and chip8 ports in this OS.
//
// Upstream: https://github.com/binji/smolnes  (MIT, deobfuscated.c)
//
// The SDL-specific parts that were changed:
//   - ROM loading:  SDL_RWFromFile/SDL_RWread  ->  open()/read()
//   - Keyboard:    SDL_GetKeyboardState + SDL scancodes -> /dev/input/event0
//                   with Linux keycodes mapped to the NES joypad order.
//   - Video:       SDL_CreateRenderer/Texture (BGR565) -> /dev/fb0 (32bpp
//                   0x00RRGGBB), integer-scaled + centered, flushed with
//                   FBIOPAN_DISPLAY.
//   - Timing:      SDL_RenderPresent vsync -> accumulated absolute-monotonic
//                  deadline pacing (see pacing.h and the NMI block in main).

#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#include "pacing.h"

// ---------------------------------------------------------------------------
// Framebuffer + input device definitions (Linux-compatible ioctls, same as
// the fbdoom / chip8 ports in this OS).
// ---------------------------------------------------------------------------

#define FB_PATH "/dev/fb0"
#define KEYBOARD_PATH "/dev/input/event0"
#define FBIOGET_VSCREENINFO 0x4600
#define FBIOGET_FSCREENINFO 0x4602
#define FBIOPAN_DISPLAY 0x4606
#define EV_KEY 1

struct fb_var_screeninfo {
    uint32_t xres;
    uint32_t yres;
    uint32_t bits_per_pixel;
    uint32_t red_offset;
    uint32_t green_offset;
    uint32_t blue_offset;
};

struct fb_fix_screeninfo {
    uint32_t line_length;
    uint32_t smem_len;
};

struct input_event {
    int64_t tv_sec;
    int64_t tv_usec;
    uint16_t type;
    uint16_t code;
    int32_t value;
};

#define PULL mem(++S, 1, 0, 0)
#define PUSH(x) mem(S--, 1, x, 1)

uint8_t *rom, *chrrom,                // Points to the start of PRG/CHR ROM
    prg[4], chr[8],                   // Current PRG/CHR banks
    prgbits = 14, chrbits = 12,       // Number of bits per PRG/CHR bank
    A, X, Y, P = 4, S = ~2, PCH, PCL, // CPU Registers
    addr_lo, addr_hi,                 // Current instruction address
    nomem,  // 1 => current instruction doesn't write to memory
    result, // Temp variable
    val,    // Current instruction value
    cross,  // 1 => page crossing occurred
    tmp,    // Temp variables
    ppumask, ppuctrl, ppustatus, // PPU registers
    ppubuf,                      // PPU buffered reads
    W,                           // Write toggle PPU register
    fine_x,                      // X fine scroll offset, 0..7
    opcode,                      // Current instruction opcode
    nmi_irq,                     // 1 => IRQ occurred
                                 // 4 => NMI occurred
    ntb,                         // Nametable byte
    ptb_lo,                      // Pattern table lowbyte
    vram[2048],                  // Nametable RAM
    palette_ram[64],             // Palette RAM
    ram[8192],                   // CPU RAM
    chrram[8192],                // CHR RAM (only used for some games)
    prgram[8192],                // PRG RAM (only used for some games)
    oam[256],                    // Object Attribute Memory (sprite RAM)
    mask[] = {128, 64, 1, 2,     // Masks used in branch instructions
              1,   0,  0, 1, 4, 0, 0, 4, 0,
              0,   64, 0, 8, 0, 0, 8}, // Masks used in SE*/CL* instructions.
    keys,                              // Joypad shift register
    mirror,                            // Current mirroring mode
    mmc1_bits, mmc1_data, mmc1_ctrl,   // Mapper 1 (MMC1) registers
    mmc3_chrprg[8], mmc3_bits,         // Mapper 4 (MMC3) registers
    mmc3_irq, mmc3_latch,              //
    chrbank0, chrbank1, prgbank,       // Current PRG/CHR bank
    rombuf[1024 * 1024],               // Buffer to read ROM file into
    key_state[8];                      // NES joypad state (8 buttons)

uint16_t scany,          // Scanline Y
    T, V,                // "Loopy" PPU registers
    sum,                 // Sum used for ADC/SBC
    dot,                 // Horizontal position of PPU, from 0..340
    atb,                 // Attribute byte
    shift_hi, shift_lo,  // Pattern table shift registers
    cycles,              // Cycle count for current instruction
    frame_buffer[61440]; // 256x240 pixel frame buffer. Top and bottom 8 rows
                         // are not drawn.

int shift_at = 0;

// Composited sprite layer for the current scanline. Building it once avoids
// scanning OAM and fetching CHR data for every visible pixel.
static uint8_t scanline_sprite_color[256];
static uint8_t scanline_sprite_palette[256];
static uint8_t scanline_sprite_behind_bg[256];
static uint8_t scanline_sprite0[256];

// ---------------------------------------------------------------------------
// NES joypad button order, matching the upstream SDL scancode table:
//   0:A 1:B 2:Select 3:Start 4:Up 5:Down 6:Left 7:Right
// ---------------------------------------------------------------------------

// Map a Linux input keycode (from /dev/input/event0) to a NES joypad index
// (0..7), or -1 if the key is not mapped.
static int keycode_to_nes(uint16_t code) {
    switch (code) {
    case 30: return 0; // A  -> linux KEY_A   (upstream: SDL_SCANCODE_X)
    case 44: return 1; // B  -> linux KEY_Z   (upstream: SDL_SCANCODE_Z)
    case 15: return 2; // Select -> linux KEY_TAB
    case 28: return 3; // Start  -> linux KEY_ENTER
    case 103: return 4; // Up    -> linux KEY_UP
    case 108: return 5; // Down  -> linux KEY_DOWN
    case 105: return 6; // Left  -> linux KEY_LEFT
    case 106: return 7; // Right -> linux KEY_RIGHT
    default: return -1;
    }
}

// Read a byte from CHR ROM or CHR RAM.
uint8_t *get_chr_byte(uint16_t a) {
  return &chrrom[chr[a >> chrbits] << chrbits | a % (1 << chrbits)];
}

// Read a byte from nametable RAM.
uint8_t *get_nametable_byte(uint16_t a) {
  return &vram[mirror == 0   ? a % 1024                  // single bank 0
               : mirror == 1 ? a % 1024 + 1024           // single bank 1
               : mirror == 2 ? a & 2047                  // vertical mirroring
                             : a / 2 & 1024 | a % 1024]; // horizontal mirroring
}

// If `write` is non-zero, writes `val` to the address `hi:lo`, otherwise reads
// a value from the address `hi:lo`.
uint8_t mem(uint8_t lo, uint8_t hi, uint8_t val, uint8_t write) {
  uint16_t addr = hi << 8 | lo;

  switch (hi >>= 4) {
  case 0: case 1: // $0000...$1fff RAM
    return write ? ram[addr] = val : ram[addr];

  case 2: case 3: // $2000..$2007 PPU (mirrored)
    lo &= 7;

    // read/write $2007
    if (lo == 7) {
      tmp = ppubuf;
      uint8_t *rom =
          // Access CHR ROM or CHR RAM
          V < 8192 ? write && chrrom != chrram ? &tmp : get_chr_byte(V)
          // Access nametable RAM
          : V < 16128 ? get_nametable_byte(V)
                      // Access palette RAM
                      : palette_ram + (uint8_t)((V & 19) == 16 ? V ^ 16 : V);
      write ? *rom = val : (ppubuf = *rom);
      V += ppuctrl & 4 ? 32 : 1;
      V %= 16384;
      return tmp;
    }

    if (write)
      switch (lo) {
      case 0: // $2000 ppuctrl
        ppuctrl = val;
        T = T & 0xf3ff | val % 4 << 10;
        break;

      case 1: // $2001 ppumask
        ppumask = val;
        break;

      case 5: // $2005 ppuscroll
        T = (W ^= 1)
          ? fine_x = val & 7, T & ~31 | val / 8
          : T & 0x8c1f | val % 8 << 12 | val * 4 & 0x3e0;
        break;

      case 6: // $2006 ppuaddr
        T = (W ^= 1)
          ? T & 0xff | val % 64 << 8
          : (V = T & ~0xff | val);
      }

    if (lo == 2) { // $2002 ppustatus
      tmp = ppustatus & 0xe0;
      ppustatus &= 0x7f;
      W = 0;
      return tmp;
    }
    break;

  case 4:
    if (write && lo == 20) // $4014 OAM DMA
      for (uint16_t i = 256; i--;)
        oam[i] = mem(i, val, 0, 0);
    // $4016 Joypad 1
    for (tmp = 0, hi = 8; hi--;)
      tmp = tmp * 2 + key_state[(uint8_t[]){
                          0, // A
                          1, // B
                          2, // Select
                          3, // Start
                          4, // Dpad Up
                          5, // Dpad Down
                          6, // Dpad Left
                          7, // Dpad Right
                      }[hi]];
    if (lo == 22) {
      if (write) {
        keys = tmp;
      } else {
        tmp = keys & 1;
        keys /= 2;
        return tmp;
      }
    }
    return 0;

  case 6: case 7: // $6000...$7fff PRG RAM
    addr &= 8191;
    return write ? prgram[addr] = val : prgram[addr];

  default: // $8000...$ffff ROM
    // handle mapper writes
    if (write)
      switch (rombuf[6] >> 4) {
      case 7: // mapper 7
        mirror = !(val / 16);
        prg[0] = val % 8 * 2;
        prg[1] = prg[0] + 1;
        break;

      case 4: { // mapper 4
        uint8_t addr1 = addr & 1;
        switch (hi >> 1) {
        case 4: // Bank select/bank data
          *(addr1 ? &mmc3_chrprg[mmc3_bits & 7] : &mmc3_bits) = val;
          tmp = mmc3_bits >> 5 & 4;
          for (int i = 4; i--;) {
            chr[0 + i + tmp] = mmc3_chrprg[i / 2] & ~!(i % 2) | i % 2;
            chr[4 + i - tmp] = mmc3_chrprg[2 + i];
          }
          tmp = mmc3_bits >> 5 & 2;
          prg[0 + tmp] = mmc3_chrprg[6];
          prg[1] = mmc3_chrprg[7];
          prg[3] = rombuf[4] * 2 - 1;
          prg[2 - tmp] = prg[3] - 1;
          break;
        case 5: // Mirroring
          if (!addr1) {
            mirror = 2 + val % 2;
          }
          break;
        case 6:  // IRQ Latch
          if (!addr1) {
            mmc3_latch = val;
          }
          break;
        case 7:  // IRQ Enable
          mmc3_irq = addr1;
          break;
        }
        break;
      }

      case 3: // mapper 3
        chr[0] = val % 4 * 2;
        chr[1] = chr[0] + 1;
        break;

      case 2: // mapper 2
        prg[0] = val & 31;
        break;

      case 1: // mapper 1
        if (val & 0x80) {
          mmc1_bits = 5;
          mmc1_data = 0;
          mmc1_ctrl |= 12;
        } else if (mmc1_data = mmc1_data / 2 | val << 4 & 16, !--mmc1_bits) {
          mmc1_bits = 5;
          tmp = addr >> 13;
          *(tmp == 4 ? mirror = mmc1_data & 3, &mmc1_ctrl
          : tmp == 5 ? &chrbank0
          : tmp == 6 ? &chrbank1
                     : &prgbank) = mmc1_data;

          // Update CHR banks.
          chr[0] = chrbank0 & ~!(mmc1_ctrl & 16);
          chr[1] = mmc1_ctrl & 16 ? chrbank1 : chrbank0 | 1;

          // Update PRG banks.
          tmp = mmc1_ctrl / 4 % 4 - 2;
          prg[0] = !tmp ? 0 : tmp == 1 ? prgbank : prgbank & ~1;
          prg[1] = !tmp ? prgbank : tmp == 1 ? rombuf[4] - 1 : prgbank | 1;
        }
      }
    return rom[(prg[hi - 8 >> prgbits - 12] & (rombuf[4] << 14 - prgbits) - 1)
                   << prgbits |
               addr & (1 << prgbits) - 1];
  }

  return ~0;
}

// Read a byte at address `PCH:PCL` and increment PC.
uint8_t read_pc() {
  val = mem(PCL, PCH, 0, 0);
  !++PCL && ++PCH;
  return val;
}

// Set N (negative) and Z (zero) flags of `P` register, based on `val`.
uint8_t set_nz(uint8_t val) { return P = P & 125 | val & 128 | !val * 2; }

// ---------------------------------------------------------------------------
// Twilight OS backend: framebuffer + keyboard.
// ---------------------------------------------------------------------------

static int fb_fd = -1;
static uint32_t *fb_ptr = NULL;
static uint32_t screen_w = 0;
static uint32_t screen_h = 0;
static size_t fb_size = 0;
static int kbd_fd = -1;

// BGR565 -> 0x00RRGGBB (32bpp) lookup, built once at startup. The upstream
// emulator renders into a 16-bit BGR565 frame_buffer; the Twilight framebuffer
// is 32bpp 0x00RRGGBB, so we expand each 16-bit pixel.
static uint32_t bgr565_to_xrgb[65536];

static void build_bgr565_table(void) {
    for (int i = 0; i < 65536; i++) {
        int b = (i >> 11) & 0x1f;
        int g = (i >> 5) & 0x3f;
        int r = i & 0x1f;
        // Scale 5/6-bit channels up to 8 bits.
        uint8_t r8 = (r << 3) | (r >> 2);
        uint8_t g8 = (g << 2) | (g >> 4);
        uint8_t b8 = (b << 3) | (b >> 2);
        bgr565_to_xrgb[i] = (r8 << 16) | (g8 << 8) | b8;
    }
}

static void fb_init(void) {
    fb_fd = open(FB_PATH, O_RDWR);
    if (fb_fd < 0) {
        perror("smolnes: open /dev/fb0");
        exit(1);
    }

    struct fb_var_screeninfo vinfo;
    struct fb_fix_screeninfo finfo;
    memset(&vinfo, 0, sizeof(vinfo));
    memset(&finfo, 0, sizeof(finfo));

    if (ioctl(fb_fd, FBIOGET_VSCREENINFO, &vinfo) < 0) {
        perror("smolnes: ioctl VSCREENINFO");
        exit(1);
    }
    if (ioctl(fb_fd, FBIOGET_FSCREENINFO, &finfo) < 0) {
        perror("smolnes: ioctl FSCREENINFO");
        exit(1);
    }

    screen_w = vinfo.xres;
    screen_h = vinfo.yres;
    fb_size = finfo.smem_len;

    fb_ptr = (uint32_t *)mmap(NULL, fb_size, PROT_READ | PROT_WRITE,
                              MAP_SHARED, fb_fd, 0);
    if (fb_ptr == MAP_FAILED) {
        perror("smolnes: mmap framebuffer");
        exit(1);
    }

    memset(fb_ptr, 0, fb_size);
    ioctl(fb_fd, FBIOPAN_DISPLAY, 0);

    printf("smolnes: framebuffer %ux%u, %u bpp, %zu bytes\n",
           screen_w, screen_h, vinfo.bits_per_pixel, fb_size);
}

// Blit the 256x224 visible NES frame (frame_buffer rows 8..231) into the
// 32bpp framebuffer, integer-scaled and centered, then flush to the display.
static void fb_present(void) {
    // The NES frame is 256 wide; the visible region is 224 rows tall (rows
    // 8..231 of the 240-row frame_buffer).
    const int nes_w = 256;
    const int nes_h = 224;

    int scale = (int)screen_w / nes_w;
    int sy = (int)screen_h / nes_h;
    if (sy < scale) scale = sy;
    if (scale < 1) scale = 1;

    int blit_w = nes_w * scale;
    int blit_h = nes_h * scale;
    int off_x = ((int)screen_w - blit_w) / 2;
    int off_y = ((int)screen_h - blit_h) / 2;
    if (off_x < 0) off_x = 0;
    if (off_y < 0) off_y = 0;

    for (int y = 0; y < nes_h; y++) {
        uint16_t *src = frame_buffer + (y + 8) * 256;
        for (int syy = 0; syy < scale; syy++) {
            int screen_y = off_y + y * scale + syy;
            if (screen_y < 0 || (uint32_t)screen_y >= screen_h)
                continue;
            uint32_t *dst = fb_ptr + screen_y * screen_w;
            for (int x = 0; x < nes_w; x++) {
                uint32_t pixel = bgr565_to_xrgb[src[x]];
                for (int sxx = 0; sxx < scale; sxx++) {
                    int screen_x = off_x + x * scale + sxx;
                    if (screen_x >= 0 && (uint32_t)screen_x < screen_w)
                        dst[screen_x] = pixel;
                }
            }
        }
    }

    ioctl(fb_fd, FBIOPAN_DISPLAY, 0);
}

// Poll /dev/input/event0 and update the NES joypad state.
static void kbd_poll(void) {
    struct input_event ev;
    for (;;) {
        ssize_t n = read(kbd_fd, &ev, sizeof(ev));
        if (n != (ssize_t)sizeof(ev))
            return;
        if (ev.type != EV_KEY)
            continue;
        int nes = keycode_to_nes(ev.code);
        if (nes < 0)
            continue;
        // value: 1 = press, 0 = release, 2 = repeat (ignored)
        if (ev.value == 2)
            continue;
        key_state[nes] = ev.value ? 1 : 0;
    }
}

// Read CLOCK_MONOTONIC. Returns 1 on success and writes ns to *out; returns 0
// on failure, in which case *out must not be used. Pacing uses CLOCK_MONOTONIC
// exclusively (never realtime) so wall-clock adjustments cannot shift cadence.
static int monotonic_now_ns_checked(uint64_t *out) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0)
        return 0;
    *out = (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
    return 1;
}

int main(int argc, char **argv) {
  if (argc < 2) {
    fprintf(stderr, "Usage: %s <rom.nes>\n", argv[0]);
    return 1;
  }

  // Load the ROM file into rombuf (up to 1 MiB).
  int romfd = open(argv[1], O_RDONLY);
  if (romfd < 0) {
    perror("smolnes: open ROM");
    return 1;
  }
  ssize_t total = 0;
  while (total < 1024 * 1024) {
    ssize_t n = read(romfd, rombuf + total, 1024 * 1024 - total);
    if (n < 0) {
      perror("smolnes: read ROM");
      close(romfd);
      return 1;
    }
    if (n == 0)
      break;
    total += n;
  }
  close(romfd);

  // Start PRG0 after 16-byte header.
  rom = rombuf + 16;
  // PRG1 is the last bank. `rombuf[4]` is the number of 16k PRG banks.
  prg[1] = rombuf[4] - 1;
  // CHR0 ROM is after all PRG data in the file. `rombuf[5]` is the number of
  // 8k CHR banks. If it is zero, assume the game uses CHR RAM.
  chrrom = rombuf[5] ? rom + (rombuf[4] << 14) : chrram;
  // CHR1 is the last 4k bank.
  chr[1] = rombuf[5] ? rombuf[5] * 2 - 1 : 1;
  // Bit 0 of `rombuf[6]` is 0=>horizontal mirroring, 1=>vertical mirroring.
  mirror = 3 - rombuf[6] % 2;
  if (rombuf[6] / 16 == 4) {
    mem(0, 128, 0, 1); // Update to default mmc3 banks
    prgbits--;         // 8kb PRG banks
    chrbits -= 2;      // 1kb CHR banks
  }

  // Start at address in reset vector, at $FFFC.
  PCL = mem(~3, ~0, 0, 0);
  PCH = mem(~2, ~0, 0, 0);

  // Initialize the Twilight OS framebuffer + keyboard backend.
  build_bgr565_table();
  fb_init();
  kbd_fd = open(KEYBOARD_PATH, O_RDONLY | O_NONBLOCK);
  if (kbd_fd < 0) {
    perror("smolnes: open /dev/input/event0");
    return 1;
  }

  // Frame pacing uses accumulated absolute-monotonic deadlines (see pacing.h):
  // each target is derived from the previous target, never from a pre-sleep
  // timestamp, so render/emulation time consumes the frame budget instead of
  // accumulating as drift. The wait happens immediately before fb_present().

loop:
  cycles = nomem = 0;
  if (nmi_irq)
    goto nmi_irq;

  opcode = read_pc();
  uint8_t opcodelo5 = opcode & 31;
  switch (opcodelo5) {
  case 0:
    if (opcode & 0x80) { // LDY/CPY/CPX imm
      read_pc();
      nomem = 1;
      goto nomemop;
    }

    switch (opcode >> 5) {
    case 0: { // BRK or nmi_irq
      !++PCL && ++PCH;
    nmi_irq:
      PUSH(PCH);
      PUSH(PCL);
      PUSH(P | 32);
      // BRK/IRQ vector is $ffff, NMI vector is $fffa
      uint16_t veclo = ~1 - (nmi_irq & 4);
      PCL = mem(veclo, ~0, 0, 0);
      PCH = mem(veclo + 1, ~0, 0, 0);
      nmi_irq = 0;
      cycles++;
      break;
    }

    case 1: // JSR
      result = read_pc();
      PUSH(PCH);
      PUSH(PCL);
      PCH = read_pc();
      PCL = result;
      break;

    case 2: // RTI
      P = PULL & ~32;
      PCL = PULL;
      PCH = PULL;
      break;

    case 3: // RTS
      PCL = PULL;
      PCH = PULL;
      !++PCL && ++PCH;
      break;
    }

    cycles += 4;
    break;

  case 16: // BPL, BMI, BVC, BVS, BCC, BCS, BNE, BEQ
    read_pc();
    if (!(P & mask[opcode >> 6]) ^ opcode / 32 & 1) {
      cross = PCL + (int8_t)val >> 8;
      PCH += cross;
      PCL += val;
      cycles += cross ? 2 : 1;
    }
    break;

  case 8: case 24:
    switch (opcode >>= 4) {
    case 0: // PHP
      PUSH(P | 48);
      cycles++;
      break;

    case 2: // PLP
      P = PULL & ~16;
      cycles += 2;
      break;

    case 4: // PHA
      PUSH(A);
      cycles++;
      break;

    case 6: // PLA
      set_nz(A = PULL);
      cycles += 2;
      break;

    case 8: // DEY
      set_nz(--Y);
      break;

    case 9: // TYA
      set_nz(A = Y);
      break;

    case 10: // TAY
      set_nz(Y = A);
      break;

    case 12: // INY
      set_nz(++Y);
      break;

    case 14: // INX
      set_nz(++X);
      break;

    default: // CLC, SEC, CLI, SEI, CLV, CLD, SED
      P = P & ~mask[opcode + 3] | mask[opcode + 4];
      break;
    }
    break;

  case 10: case 26:
    switch (opcode >> 4) {
    case 8: // TXA
      set_nz(A = X);
      break;

    case 9: // TXS
      S = X;
      break;

    case 10: // TAX
      set_nz(X = A);
      break;

    case 11: // TSX
      set_nz(X = S);
      break;

    case 12: // DEX
      set_nz(--X);
      break;

    case 14: // NOP
      break;

    default: // ASL/ROL/LSR/ROR A
      nomem = 1;
      val = A;
      goto nomemop;
    }
    break;

  case 1: // X-indexed, indirect
    read_pc();
    val += X;
    addr_lo = mem(val, 0, 0, 0);
    addr_hi = mem(val + 1, 0, 0, 0);
    cycles += 4;
    goto opcode;

  case 2: case 9: // Immediate
    read_pc();
    nomem = 1;
    goto nomemop;

  case 17: // Zeropage, Y-indexed
    addr_lo = mem(read_pc(), 0, 0, 0);
    addr_hi = mem(val + 1, 0, 0, 0);
    cycles++;
    goto add_x_or_y;

  case 4: case 5: case 6:     // Zeropage               +1
  case 20: case 21: case 22:  // Zeropage, X-indexed    +2
    addr_lo = read_pc();
    cross = opcodelo5 > 6;
    if (cross) {
      addr_lo += (opcode & 214) == 150 ? Y : X;  // LDX/STX use Y
    }
    addr_hi = 0;
    cycles -= !cross;
    goto opcode;

  case 12: case 13: case 14: // Absolute               +2
  case 25:                   // Absolute, Y-indexed    +2/3
  case 28: case 29: case 30: // Absolute, X-indexed    +2/3
    addr_lo = read_pc();
    addr_hi = read_pc();
    if (opcodelo5 < 25) goto opcode;
  add_x_or_y:
    val = opcodelo5 < 28 | opcode == 190 ? Y : X;
    cross = addr_lo + val > 255;
    addr_hi += cross;
    addr_lo += val;
    cycles +=
        ((opcode & 224) == 128 | opcode % 16 == 14 & opcode != 190) | cross;
  opcode:
    cycles += 2;
    if (opcode != 76 & (opcode & 224) != 128) {
      val = mem(addr_lo, addr_hi, 0, 0);
    }

  nomemop:
    result = 0;
    switch (opcode & 227) {
    case 1: set_nz(A |= val); break;  // ORA
    case 33: set_nz(A &= val); break; // AND
    case 65: set_nz(A ^= val); break; // EOR
    case 225: // SBC
      val = ~val;
      // fallthrough
    case 97: // ADC
      sum = A + val + P % 2;
      P = P & ~65 | sum > 255 | ((A ^ sum) & (val ^ sum) & 128) / 2;
      set_nz(A = sum);
      break;

    case 34: // ROL
      result = P & 1;
      // fallthrough
    case 2: // ASL
      result |= val * 2;
      P = P & ~1 | val / 128;
      goto memop;

    case 98: // ROR
      result = P << 7;
      // fallthrough
    case 66: // LSR
      result |= val / 2;
      P = P & ~1 | val & 1;
      goto memop;

    case 194: // DEC
      result = val - 1;
      goto memop;

    case 226: // INC
      result = val + 1;
      // fallthrough

    memop:
      set_nz(result);
      // Write result to A or back to memory.
      nomem ? A = result : (cycles += 2, mem(addr_lo, addr_hi, result, 1));
      break;

    case 32: // BIT
      P = P & 61 | val & 192 | !(A & val) * 2;
      break;

    case 64: // JMP
      PCL = addr_lo;
      PCH = addr_hi;
      cycles--;
      break;

    case 96: // JMP indirect
      PCL = val;
      PCH = mem(addr_lo + 1, addr_hi, 0, 0);
      cycles++;
      break;

    default: {
      uint8_t opcodehi3 = opcode / 32;
      uint8_t *reg = opcode % 4 == 2 | opcodehi3 == 7 ? &X
                     : opcode % 4 == 1                ? &A
                                                      : &Y;
      if (opcodehi3 == 4) {  // STY/STA/STX
        mem(addr_lo, addr_hi, *reg, 1);
      } else if (opcodehi3 != 5) {  // CPY/CMP/CPX
        P = P & ~1 | *reg >= val;
        set_nz(*reg - val);
      } else {  // LDY/LDA/LDX
        set_nz(*reg = val);
      }
      break;
    }
    }
  }

  // Update PPU, which runs 3 times faster than CPU. Each CPU instruction
  // takes at least 2 cycles.
  for (tmp = cycles * 3 + 6; tmp--;) {
    if (ppumask & 24) { // If background or sprites are enabled.
      if (scany < 240) {
        if (dot == 0) {
          uint8_t sprite_h = ppuctrl & 32 ? 16 : 8;
          memset(scanline_sprite_color, 0, sizeof(scanline_sprite_color));
          memset(scanline_sprite0, 0, sizeof(scanline_sprite0));

          // Compose from lowest to highest priority so lower OAM indices
          // overwrite later sprites, matching the original first-hit search.
          for (int offset = 252; offset >= 0; offset -= 4) {
            uint16_t sprite_y = scany - oam[offset] - 1;
            if (sprite_y < sprite_h) {
              uint8_t *sprite = oam + offset;
              uint16_t sy = sprite_y ^
                            (sprite[2] & 128 ? sprite_h - 1 : 0);
              uint16_t sprite_tile = sprite[1];
              uint16_t sprite_addr =
                  (ppuctrl & 32
                       ? sprite_tile % 2 << 12 |
                             sprite_tile << 4 & -32 | sy * 2 & 16
                       : (ppuctrl & 8) << 9 | sprite_tile << 4) |
                  sy & 7;
              uint8_t pattern_lo = *get_chr_byte(sprite_addr);
              uint8_t pattern_hi = *get_chr_byte(sprite_addr + 8);

              for (uint16_t x = 0; x < 8; x++) {
                uint16_t screen_x = sprite[3] + x;
                if (screen_x >= 256) continue;
                uint8_t sx = x ^ !(sprite[2] & 64) * 7;
                uint8_t sprite_color =
                    pattern_hi >> sx << 1 & 2 | pattern_lo >> sx & 1;
                if (!sprite_color) continue;
                scanline_sprite_color[screen_x] = sprite_color;
                scanline_sprite_palette[screen_x] =
                    16 | sprite[2] * 4 & 12;
                scanline_sprite_behind_bg[screen_x] = sprite[2] & 32;
                if (offset == 0) scanline_sprite0[screen_x] = 1;
              }
            }
          }
        }

        if (dot - 256 > 63u) {  // dot [0..255,320..340]
          // Draw a pixel to the framebuffer.
          if (dot < 256) {
            // Read color and palette from shift registers.
            uint8_t color = shift_hi >> 14 - fine_x & 2 |
                            shift_lo >> 15 - fine_x & 1,
                    palette = shift_at >> 28 - fine_x * 2 & 12;

            // If sprites are enabled.
            if (ppumask & 16) {
              uint8_t sprite_color = scanline_sprite_color[dot];
              if (sprite_color) {
                // Don't draw the sprite when its priority is behind a
                // non-transparent background pixel.
                if (!(scanline_sprite_behind_bg[dot] && color)) {
                  color = sprite_color;
                  palette = scanline_sprite_palette[dot];
                }
                if (scanline_sprite0[dot]) ppustatus |= 64;
              }
            }

            // Write pixel to framebuffer. Always use palette 0 for color 0.
            // BGR565 palette is used instead of RGBA32 to reduce source code
            // size.
            frame_buffer[scany * 256 + dot] =
                (uint16_t[64]){
                    25356, 34816, 39011, 30854, 24714, 4107,  106,   2311,
                    2468,  2561,  4642,  6592,  20832, 0,     0,     0,
                    44373, 49761, 55593, 51341, 43186, 18675, 434,   654,
                    4939,  5058,  3074,  19362, 37667, 0,     0,     0,
                    ~0,    ~819,  64497, 64342, 62331, 43932, 23612, 9465,
                    1429,  1550,  20075, 36358, 52713, 16904, 0,     0,
                    ~0,    ~328,  ~422,  ~452,  ~482,  58911, 50814, 42620,
                    40667, 40729, 48951, 53078, 61238, 44405}
                    [palette_ram[color ? palette | color : 0]];
          }

          // Update shift registers every cycle.
          if (dot < 336) {
            shift_hi *= 2;
            shift_lo *= 2;
            shift_at *= 4;
          }

          int temp = ppuctrl << 8 & 4096 | ntb << 4 | V >> 12;
          switch (dot & 7) {
          case 1: // Read nametable byte.
            ntb = *get_nametable_byte(V);
            break;
          case 3: // Read attribute byte.
            atb = (*get_nametable_byte(V & 0xc00 | 0x3c0 | V >> 4 & 0x38 |
                                       V / 4 & 7) >>
                   (V >> 5 & 2 | V / 2 & 1) * 2) %
                  4 * 0x5555;
            break;
          case 5: // Read pattern table low byte.
            ptb_lo = *get_chr_byte(temp);
            break;
          case 7: { // Read pattern table high byte.
            uint8_t ptb_hi = *get_chr_byte(temp | 8);
            // Increment horizontal VRAM read address.
            V = V % 32 == 31 ? V & ~31 ^ 1024 : V + 1;
            shift_hi |= ptb_hi;
            shift_lo |= ptb_lo;
            shift_at |= atb;
            break;
          }
          }
        }

        // Increment vertical VRAM address.
        if (dot == 256) {
          V = ((V & 7 << 12) != 7 << 12 ? V + 4096
               : (V & 0x3e0) == 928     ? V & 0x8c1f ^ 2048
               : (V & 0x3e0) == 0x3e0   ? V & 0x8c1f
                                        : V & 0x8c1f | V + 32 & 0x3e0) &
                  // Reset horizontal VRAM address to T value.
                  ~0x41f |
              T & 0x41f;
        }
      }

      // Check for MMC3 IRQ.
      if ((scany + 1) % 262 < 241 && dot == 261 && mmc3_irq && !mmc3_latch--)
        nmi_irq = 1;

      // Reset vertical VRAM address to T value.
      if (scany == 261 && dot - 280 < 25u)  // dot [280..304]
        V = V & 0x841f | T & 0x7be0;
    }

    if (dot == 1) {
      if (scany == 241) {
        // If NMI is enabled, trigger NMI.
        if (ppuctrl & 128)
          nmi_irq = 4;
        ppustatus |= 128;

        // --- Frame pacing ---------------------------------------------------
        // Wait until the next absolute presentation deadline, then present.
        // The wait precedes fb_present() so emulation/rendering work consumes
        // the frame budget rather than being added on top of the target. Each
        // deadline is derived from the previous deadline (never a pre-sleep
        // timestamp), eliminating short/long alternation and relative-sleep
        // drift. Catch-up is bounded: a large time discontinuity (VM pause)
        // rebases to now+period instead of looping through phantom periods.
        static int pacing_started = 0;
        static uint64_t next_present_ns = 0;
#ifdef SMOLNES_PACING_DEBUG
        static uint64_t dbg_prev_present_ns = 0;
#endif

        uint64_t now;
        if (!monotonic_now_ns_checked(&now)) {
          // Clock unreadable: skip pacing this frame but keep video alive.
          // Do not touch next_present_ns; it stays valid for the next frame.
          fb_present();
          kbd_poll();
          goto clear_ppustatus;
        }

        if (!pacing_started) {
          // First frame: seed the deadline one period ahead of now.
          next_present_ns = checked_add_ns(now, SMOLNES_NTSC_FRAME_NS);
          pacing_started = 1;
        } else {
          next_present_ns = advance_deadline_checked(next_present_ns, now,
                                                     SMOLNES_NTSC_FRAME_NS);
        }

        // Poll input before the long wait so a shutdown/exit keypress is not
        // delayed by up to one frame.
        kbd_poll();

        if (now < next_present_ns) {
          struct timespec deadline = {
              .tv_sec  = (time_t)(next_present_ns / 1000000000ull),
              .tv_nsec = (long)(next_present_ns % 1000000000ull),
          };
          // clock_nanosleep returns the error number directly, not via errno.
          int rc = clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &deadline,
                                   NULL);
          if (rc == EINTR) {
            // Interrupted by a signal: re-poll input (a shutdown key may have
            // arrived) and fall through to present immediately rather than
            // blindly retrying and swallowing the input.
            kbd_poll();
          }
          // Other non-zero rc: present as soon as possible.
        }

        // Sample the presentation timestamp immediately after fb_present() so
        // measured lateness has one stable meaning across backends.
        fb_present();
        uint64_t presented_ns;
        if (!monotonic_now_ns_checked(&presented_ns))
          presented_ns = next_present_ns;  // last known-good fallback

#ifdef SMOLNES_PACING_DEBUG
        {
          int64_t lateness = (int64_t)(presented_ns - next_present_ns);
          uint64_t interval = dbg_prev_present_ns
                              ? presented_ns - dbg_prev_present_ns : 0;
          fprintf(stderr,
                  "smolnes pace: deadline=%llu presented=%llu lateness=%lld "
                  "interval=%llu\n",
                  (unsigned long long)next_present_ns,
                  (unsigned long long)presented_ns,
                  (long long)lateness,
                  (unsigned long long)interval);
          dbg_prev_present_ns = presented_ns;
        }
#endif
        kbd_poll();
      }

clear_ppustatus:
      // Clear ppustatus.
      if (scany == 261)
        ppustatus = 0;
    }

    // Increment to next dot/scany. 341 dots per scanline, 262 scanlines per
    // frame. Scanline 261 is represented as -1.
    if (++dot == 341) {
      dot = 0;
      scany++;
      scany %= 262;
    }
  }
  goto loop;
}
