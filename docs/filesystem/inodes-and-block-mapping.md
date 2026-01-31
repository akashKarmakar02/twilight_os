# Inodes & block mapping (TwilightFS)

This page explains how an inode represents a file/directory on disk and how file offsets map to zones.

Relevant code:
- `twilight_kernel/src/sys/fs/twilight_fs/inode.rs` (`Inode`, `TFSVfsNode::{read,write,truncate}`)
- `twilight_kernel/src/sys/fs/twilight_fs/mod.rs` (`read_inode`, `write_inode`, `read_file`, `write_file`)

## The on-disk inode structure

TwilightFS’s inode is a fixed-size `#[repr(C, packed)]` struct:

```
Inode {
  mode: u16,
  nlinks: u16,
  uid: u32, gid: u32,
  size: u64,
  access_time: u32, modified_time: u32, created_time: u32,
  zones: [u32; 7],
  indirect_zones: u32,
  double_indirect_zones: u32,
  triple_indirect_zones: u32,
}
```

Key fields:
- `mode`: used as both “type + permissions”. The codebase currently uses:
  - regular file: `0o100777`
  - directory: `0o040777` (most mkdir paths), root uses `0o040755` in the installer
- `size`: file size in bytes (directories also track a size: number_of_entries * 64).
- `zones[]`: direct pointers to data zones (each zone is 2048 bytes).
- `indirect_zones`: zone number of a “single-indirect” pointer block.
- `double_indirect_zones`: zone number of a “double-indirect” pointer block.
- `triple_indirect_zones`: present, but not used by current read/write paths.

## How an inode number maps to disk

The inode table starts after:
- FS block 0 (superblock)
- `imap_blocks`
- `zmap_blocks`

So:

```
inode_table_start_block = 1 + imap_blocks + zmap_blocks
```

Then the inode is indexed inside that table:

```
inode_index     = inode_no - 1               // 0-based
inodes_per_block = FS_BLOCK_SIZE / sizeof(Inode)
block_offset    = inode_index / inodes_per_block
byte_offset     = (inode_index % inodes_per_block) * sizeof(Inode)
disk_block      = inode_table_start_block + block_offset
```

This is implemented in `read_inode()` / `write_inode()`.

## Block mapping (offset → zone)

Given a file offset:

```
logical_block = offset / 2048
block_offset  = offset % 2048
```

### Direct blocks
For `logical_block` in `[0..7)`:
- zone = `inode.zones[logical_block]`

### Single indirect blocks
After the 7 direct blocks, the next logical blocks come from `inode.indirect_zones`.

The single-indirect block is a 2048-byte block full of `u32` zone numbers.
With 2048-byte blocks:
- there are 512 `u32` slots
- current code iterates `0..(512 - 1)`, i.e. **511 usable pointers**

So single-indirect can cover **511 additional data blocks**.

### Double indirect blocks
After direct + single-indirect, blocks come from `inode.double_indirect_zones`.

Double-indirect is a two-level tree:
- root block: up to 511 pointers to “indirect blocks”
- each indirect block: up to 511 pointers to data zones

Capacity:
- `511 * 511 = 261121` data blocks

### Triple indirect blocks
`triple_indirect_zones` exists in the inode structure but is not used by the current read/write implementations.

## Maximum file size (current implementation)

Blocks addressable:
- direct: `7`
+- single indirect: `511`
+- double indirect: `261121`

Total blocks: `7 + 511 + 261121 = 261639`

With 2048-byte blocks:
- max file size ≈ `261639 * 2048 = 535,836,672 bytes` ≈ **511 MiB**

This is a *structural* limit from the pointer scheme; it is independent of the superblock’s `max_size` field.

