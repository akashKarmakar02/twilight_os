# Bitmaps (imap / zmap) (TwilightFS)

TwilightFS uses two on-disk bitmaps:
- `imap`: inode allocation bitmap
- `zmap`: zone (data block) allocation bitmap

Each bitmap is just raw bytes on disk where each bit represents “allocated (1)” or “free (0)”.

Relevant code:
- `twilight_kernel/src/sys/fs/twilight_fs/mod.rs` (`allocate_inode`, `dealloc_inode`, `allocate_zone`, `dealloc_zone`)

## Why a bitmap is a good fit here

For a teaching/hobby filesystem:
- Very small overhead: 1 bit per inode/zone.
- Simple scanning allocator: find the first 0-bit, set it.
- Fast enough for small disks and low fragmentation pressure.

The tradeoff is that allocation is **O(n)** in the size of the bitmap (linear scan).

## Bitmap capacity per FS block

Since `FS_BLOCK_SIZE = 2048`:
- bytes per bitmap block = 2048
- bits per bitmap block = `2048 * 8 = 16384`

So one bitmap FS block can describe:
- 16,384 inodes (for `imap`)
- 16,384 zones (for `zmap`)

## Inode bitmap indexing (imap)

On disk, inodes are treated as **1-based** (root is inode `1`).

To map an inode number to a bit:

```
inode_index = inode_no - 1         // 0-based
bit_index   = inode_index
block_index = bit_index / 16384
byte_index  = (bit_index % 16384) / 8
bit_in_byte = (bit_index % 8)
```

`dealloc_inode(inode_no)` uses exactly this scheme.

Important detail: `allocate_inode()` currently returns a **0-based inode index** (0, 1, 2, ...).
Call sites typically do `+ 1` to get the on-disk inode number.

## Zone bitmap indexing (zmap)

Zones are numbered such that the first data zone is `superblock.first_data_zone`.

To map a zone number to a bit, first make it relative to data start:

```
relative_zone = zone_no - first_data_zone
bit_index     = relative_zone
block_index   = bit_index / 16384
byte_index    = (bit_index % 16384) / 8
bit_in_byte   = (bit_index % 8)
```

`allocate_zone()` returns the **absolute** zone number:
- `first_data_zone + relative_zone`

## Initialization note

The allocator assumes the bitmap blocks contain 0 for “free”. After formatting, those blocks should be zeroed. Today, `Superblock::write` does not explicitly zero them, so the “mkfs” flow must ensure the partition is in a known state (or be extended to zero the metadata region).

