# “mkfs” and initial filesystem population (TwilightFS)

TwilightFS formatting and initial population currently happen in the kernel installer logic rather than in a standalone userspace mkfs tool.

Relevant code:
- `twilight_kernel/src/kernel_utils/install.rs`
- `twilight_kernel/src/sys/fs/twilight_fs/mod.rs` (`format_superblock`, allocators, inode/dir helpers)

## Step 1: Create / locate the TwilightFS partition

The installer ensures an MBR partition table exists and then creates (or reuses) a partition entry for TwilightFS.
The filesystem partition type is:
- `0x99` (`TWILIGHT_PARTITION_TYPE`)

Once the partition start LBA is known, TwilightFS stores it as the filesystem offset (`FS_BLOCK_OFFSET`), so block 0 in filesystem terms maps to the partition start on disk.

## Step 2: Write the superblock

The installer calls:
- `format_superblock(&mut disk, partition_start_lba, partition_sector_count)`

This writes the superblock at filesystem block 0 and returns a `TwilightFs` handle configured for that partition.

## Step 3: Create the root inode (inode #1)

Immediately after writing the superblock, the installer:
- allocates one inode bit in `imap`
- allocates one zone bit in `zmap`
- writes inode `#1` as a directory inode pointing at that zone
- inserts `.` and `..` into that root directory

This happens in `install.rs` so you can treat it as “mkfs root initialization”.

## Step 4: Create base directories

The installer then creates a minimal base tree (under `/`), e.g.:
- `/bin`, `/dev`, `/init`, `/home`, `/usr`

This uses the in-kernel helpers:
- `create_dir(parent_inode, name)`

## Step 5: Copy live-system entries into the new FS

Finally, the installer iterates the mounted live system image and preserves each
entry's kind while populating the new filesystem:

- creates intermediate directories if missing (`find_dir_entry` / `create_dir`)
- creates regular file inodes and writes their contents (`create_file` / `write_file`)
- recreates symbolic-link inodes with their original inline targets (`create_symlink`)

Keeping regular files and symlinks as distinct installer entry variants prevents
link-target length metadata from truncating a dereferenced file.
