# TwilightFS (TFS)

TwilightFS is the native on-disk filesystem used by Twilight OS. It is designed to be simple, robust enough for a hobby OS, and fast enough for normal developer workflows. The implementation lives under `twilight_kernel/src/sys/fs/twilight_fs/` and integrates with the VFS layer via `TFSVfsNode`.

## At a glance
- Block size: 2048 bytes (`FS_BLOCK_SIZE`)
- Partition type: custom MBR type `0x99` (see `partition::TWILIGHT_PARTITION_TYPE`)
- Addressing: blocks map to 4 x 512-byte sectors on disk
- Root inode: inode 1
- Directory entry: fixed-size 64 bytes (u32 inode + 60-byte name)

## Disk layout
TwilightFS follows a classic superblock + bitmap + inode table layout.

```
LBA (device sectors)
+--------------------------+
| MBR / partition table    |
+--------------------------+
| TwilightFS partition     |
|  (start LBA = offset)    |
|                          |
|  FS block 0: Superblock  |
|  FS block 1..N: imap      |
|  FS block N..M: zmap      |
|  FS block M..K: inode tbl |
|  FS block K..end: data    |
+--------------------------+

FS block size = 2048 bytes = 4 sectors (512 bytes each)
```

The filesystem is located by scanning the MBR for partition type `0x99`. If found, `fs_block_offset` is set to the partition start LBA. If not found, the filesystem is assumed to start at LBA 0.

## Superblock
The superblock lives at FS block 0 and is validated by magic `TFS0` and subversion 0.

Key fields:
- `ninodes`: total inode count (computed as 1 inode per 16 KiB of disk).
- `imap_blocks`: inode bitmap blocks.
- `zmap_blocks`: data zone bitmap blocks.
- `first_data_zone`: zone index of the first data block.
- `log_zone_size`: 0 (zone == block).
- `block_size`: 2048.
- `zones`: total zones available on the device.

See `superblock.rs` for the exact layout and the formatting logic.

## Inodes and block mapping
The on-disk inode format (`Inode`) includes fixed direct zones and indirect pointers:

```
Inode {
  mode, nlinks, uid, gid,
  size,
  access_time, modified_time, created_time,
  zones[7],            // direct
  indirect_zones,      // single indirect
  double_indirect_zones,
  triple_indirect_zones
}
```

Mapping strategy:
- Direct: 7 direct blocks
- Single indirect: one block of u32 block numbers
- Double indirect: two-level tree of blocks
- Triple indirect: reserved in the inode, but not yet implemented in read/write paths

The read path in `inode.rs` walks direct, single, then double-indirect zones. If a zone is `0`, it terminates early.

## Directory entries
Each directory entry is a fixed 64 bytes:
- `inode`: u32
- `name`: [u8; 60]

Names are fixed-size and NUL-padded. This keeps lookup simple and minimizes per-entry metadata overhead.

## Allocation
TwilightFS uses two bitmaps:
- `imap`: tracks allocated inodes
- `zmap`: tracks allocated data zones

Allocation scans bitmaps linearly and sets the first free bit. The layout is:

```
[superblock][imap blocks][zmap blocks][inode table][data zones...]
```

Important functions:
- `allocate_inode()` / `dealloc_inode()`
- `allocate_zone()` / `dealloc_zone()`

## Caching
TwilightFS implements two small in-memory caches under `TwilightFsShared`:

1) Path lookup cache
- Capacity: 1024 entries
- Key: full canonical path
- Value: inode number
- Generation-based invalidation on filesystem changes

2) File content cache
- Max per file: 8 MiB
- Max total: 32 MiB
- Stores full file contents for small files
- Generation-based invalidation on writes or metadata changes

The cache generation is incremented on operations that mutate the filesystem so stale entries are dropped.

## VFS integration
`TFSVfsNode` implements VFS node operations. Reads follow the zone mapping logic above. Writes allocate zones as needed and update inode size and timestamps. After updates, the filesystem cache generation is invalidated.

## Known limits and TODOs
- Triple-indirect zones are defined but not yet used.
- No journal or copy-on-write; power loss can corrupt recent metadata changes.
- No extent-based allocation; scanning is linear.

These are reasonable tradeoffs for a teaching OS and can be evolved later.
