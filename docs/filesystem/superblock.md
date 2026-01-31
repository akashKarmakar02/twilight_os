# Superblock (TwilightFS)

The superblock is stored at **FS block 0** and describes the filesystem’s geometry: how many inodes exist, how big the bitmaps are, and where data starts.

Relevant code:
- `twilight_kernel/src/sys/fs/twilight_fs/superblock.rs`

## On-disk structure

The superblock is a `#[repr(C, packed)]` struct and is written/read via raw byte copies.

Important fields (high-level meaning):
- `ninodes`: total number of inodes available in the filesystem.
- `imap_blocks`: how many FS blocks are used by the inode bitmap.
- `zmap_blocks`: how many FS blocks are used by the zone bitmap.
- `first_data_zone`: the first zone number that refers to file/dir data.
- `log_zone_size`: 0 (zone == block).
- `block_size`: 2048.
- `zones`: total zone count (currently equals total FS block count).
- `magic`: `'T' 'F' 'S' '0'` as a `u32` (little-endian).
- `subversion`: currently `0`.
- `max_size`: currently a placeholder (`0x7FFF_FFFF`).

Validation:
- `magic == TFS0` and `subversion == 0` (`Superblock::is_valid`).

## How formatting computes sizes

Formatting is done by `Superblock::write(device, partition_sector_count)`.

Key steps:

1) **Device geometry**
- Sector size is assumed to be 512 bytes.
- `FS_BLOCK_SIZE = 2048` so `sectors_per_fs_block = 4`.
- Total FS blocks on the partition:
  - `total_blocks = partition_sector_count / 4` (floor)
- Total zones:
  - `total_zones = total_blocks` (because `log_zone_size = 0`)

2) **Choose `ninodes`**
The current policy is “1 inode per 16 KiB of disk”:
- `ninodes = max(total_bytes / 16384, 1)`

3) **Compute bitmap + inode table sizes**
- `bits_per_block = FS_BLOCK_SIZE * 8 = 16384`
- `imap_blocks = ceil(ninodes / bits_per_block)`
- `inode_blocks = ceil(ninodes * size_of::<Inode>() / FS_BLOCK_SIZE)`

4) **Resolve `zmap_blocks` and `first_data_zone`**
`zmap_blocks` depends on how many data zones exist, but how many data zones exist depends on where data begins… so the code runs a small fixed-point iteration:
- start with `zmap_blocks = 0`
- compute:
  - `first_data_block = 1 (super) + imap_blocks + zmap_blocks + inode_blocks`
  - `first_data_zone = first_data_block` (because zone==block)
  - `data_zones = total_zones - first_data_zone`
  - `zmap_blocks = ceil(data_zones / bits_per_block)`
- repeat a few times until stable

## What formatting does not do (yet)

`Superblock::write` writes only the superblock block. It does **not**:
- zero `imap` / `zmap`
- zero the inode table
- create the root inode (`inode #1`)

Root creation currently happens in:
- `twilight_kernel/src/kernel_utils/install.rs` (allocates inode+zone, writes inode #1, adds `.` and `..`).

