# On-disk layout & addressing (TwilightFS)

TwilightFS is stored inside a single MBR partition (type `0x99`). The filesystem is addressed in 2048-byte “FS blocks”, but the underlying device I/O is in 512-byte sectors.

## Partition discovery and offsets

At mount time, TwilightFS scans the MBR partition table for a partition entry with:
- `partition_type == 0x99` (`TWILIGHT_PARTITION_TYPE`)

If found, the filesystem stores the partition start LBA as an offset (in bytes):
- `set_fs_block_offset_lba(entry.lba_start)`

If not found, it assumes the filesystem starts at LBA 0 (offset 0).

Relevant code:
- `twilight_kernel/src/sys/fs/partition.rs`
- `twilight_kernel/src/sys/fs/twilight_fs/mod.rs` (`detect_twilight_partition*`, `FS_BLOCK_OFFSET`)

## Translating a TFS block number into a device LBA

Because `FS_BLOCK_SIZE = 2048` and sectors are 512 bytes:

- `sectors_per_fs_block = 2048 / 512 = 4`

So:

```
device_lba = partition_start_lba + (tfs_block_no * 4)
```

This translation is done by:
- `read_tfs_block(device, block_no, buf)`
- `write_tfs_block(device, block_no, buf)`

Both functions compute:
- `start_block = (block_no * 4) + fs_block_offset_sectors()`

## Block layout inside the partition

TwilightFS uses a classic layout:

```
FS block 0  : superblock
FS block 1+ : inode bitmap (imap) blocks
next        : zone bitmap (zmap) blocks
next        : inode table blocks
next        : data zones
```

The exact sizes (`imap_blocks`, `zmap_blocks`, `first_data_zone`, `ninodes`) are written into the superblock during formatting.

