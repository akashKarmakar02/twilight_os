# TwilightFS (TFS)

TwilightFS is Twilight OS’s native, simple on-disk filesystem. This page is the “manager view” (high-level + navigation). The deep dives live in `docs/filesystem/`.

Source of truth:
- On-disk + core logic: `twilight_kernel/src/sys/fs/twilight_fs/`
- VFS integration: `twilight_kernel/src/sys/fs/vfs.rs`
- Installer/mkfs-ish flow: `twilight_kernel/src/kernel_utils/install.rs`

## Quick facts (what you should memorize first)
- Device sector size: 512 bytes (`twilight_kernel/src/sys/fs/partition.rs`)
- TFS block size: 2048 bytes (`FS_BLOCK_SIZE`), i.e. 4 sectors per TFS block (`read_tfs_block` / `write_tfs_block`)
- Partition discovery: MBR partition type `0x99` (`TWILIGHT_PARTITION_TYPE`)
- Root directory: inode **#1** (created during install)
- Directory entry: fixed 64 bytes = `u32 inode` + `name[60]` (NUL padded)
- Allocation: two on-disk bitmaps:
  - `imap` = inode allocation bitmap
  - `zmap` = data-zone (block) allocation bitmap

## What “zone”, “block”, and “sector” mean in TFS
TFS uses 512-byte *sectors* for raw disk I/O. It groups 4 sectors into one 2048-byte TFS *block*. In the current implementation, a *zone* is the same thing as a TFS block (`log_zone_size = 0`), so:

- 1 sector = 512 bytes (hardware/driver unit)
- 1 TFS block = 2048 bytes = 4 sectors (filesystem unit)
- 1 zone = 1 TFS block (allocation + inode pointers)

## On-disk layout (mental model)
Inside the TwilightFS partition, blocks are laid out like a classic “super + bitmaps + inode table + data” filesystem:

```
FS block 0    : superblock
FS block 1..  : imap (inode bitmap)      [superblock.imap_blocks blocks]
next..        : zmap (zone bitmap)       [superblock.zmap_blocks blocks]
next..        : inode table              [enough blocks for superblock.ninodes]
rest          : data zones (file/dir contents)
```

The partition start LBA is stored as an offset, and all TFS block reads/writes add that offset:
- `twilight_kernel/src/sys/fs/twilight_fs/mod.rs` (`FS_BLOCK_OFFSET`, `read_tfs_block`, `write_tfs_block`)

## Why bitmaps?
Bitmaps give you the simplest “free list” possible:
- 1 bit per inode or per data zone (tiny overhead)
- easy to scan + set/clear (great for a hobby OS)
- no complicated pointer structures on disk

In TFS:
- `allocate_inode()` scans `imap` and sets the first 0-bit.
- `allocate_zone()` scans `zmap` and sets the first 0-bit.
- `dealloc_inode()` / `dealloc_zone()` clear bits.

## File size limits (from the inode pointer scheme)
An inode contains:
- 7 direct zone pointers (`zones[7]`)
- 1 single-indirect pointer (`indirect_zones`) → a block full of `u32` zone numbers
- 1 double-indirect pointer (`double_indirect_zones`) → a block of pointers to indirect blocks
- triple-indirect exists in the struct, but is not used by the current read/write paths

With 2048-byte blocks and `u32` pointers:
- pointers per indirect block = `2048 / 4 = 512`
- the current code iterates `0..(512 - 1)`, so capacity is effectively **511** pointers per indirect block

Maximum data blocks addressable (current scheme):
- direct: `7`
- single-indirect: `511`
- double-indirect: `511 * 511 = 261121`
- total: `261639` blocks
- max file size: `261639 * 2048 ≈ 511 MiB`

See details in `docs/filesystem/inodes-and-block-mapping.md`.

## Where to read next (deep dives)
- `docs/filesystem/glossary.md` — terminology + constants used in code
- `docs/filesystem/on-disk-layout.md` — layout, offsets, and addressing
- `docs/filesystem/superblock.md` — how formatting computes sizes
- `docs/filesystem/mkfs-and-initialization.md` — how the installer creates the root tree
- `docs/filesystem/bitmaps.md` — `imap` / `zmap`, indexing, allocation rules
- `docs/filesystem/inodes-and-block-mapping.md` — inode fields, block mapping, limits
- `docs/filesystem/directories.md` — fixed dir entries, lookup, mkdir/touch/rm flows
- `docs/filesystem/io-paths.md` — `write_file` vs VFS `write`, read paths, truncation
- `docs/filesystem/caching.md` — path cache + file content cache and invalidation
- `docs/filesystem/known-issues.md` — sharp edges + TODOs found in current code
