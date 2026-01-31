# Glossary (TwilightFS)

This glossary matches the terms used in the TwilightFS code under `twilight_kernel/src/sys/fs/twilight_fs/`.

## Disk / addressing terms

- **Sector**: the physical/driver I/O unit. Twilight OS assumes 512-byte sectors (`partition::SECTOR_SIZE`).
- **LBA**: “Logical Block Addressing” index of a sector on disk (0, 1, 2, ...).
- **TFS block**: the filesystem I/O unit. TwilightFS uses 2048-byte blocks (`FS_BLOCK_SIZE`), which is **4 sectors**.
- **Zone**: the filesystem allocation unit and what inodes point at. In the current implementation, **zone == TFS block** (`superblock.log_zone_size = 0`).

## Filesystem metadata terms

- **Superblock**: the “header” block (FS block 0) that describes the filesystem geometry.
- **Inode**: a fixed-size record describing a file/directory (ownership, times, size, and block pointers).
- **Inode table**: a packed array of inodes stored on disk after the bitmaps.
- **Directory entry**: a fixed-size `(inode, name)` record stored inside directory data blocks.
- **Bitmap**: a dense bitset where each bit is an allocation state.
  - **imap**: inode bitmap (which inodes are in use).
  - **zmap**: zone bitmap (which data zones are in use).

## Constants (as implemented)

- `FS_BLOCK_SIZE = 2048`
- `bits_per_bitmap_block = FS_BLOCK_SIZE * 8 = 16384`
- `pointers_per_indirect_block = FS_BLOCK_SIZE / 4 = 512` (u32 pointers)
- Current code uses `0..(512 - 1)` in a few loops, so **effective capacity is 511 pointers** per indirect block.

