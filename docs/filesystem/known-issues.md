# Known issues & sharp edges (TwilightFS)

This page documents behaviors that are important when reasoning about the current implementation. It’s intentionally concrete and code-focused.

## Indexing footgun: inode numbers vs inode indices

- `allocate_inode()` returns a **0-based inode index**.
- Most call sites convert it to an on-disk inode number with `+ 1`.
- `dealloc_inode(inode_no)` expects a **1-based inode number** (it does `inode_no - 1` internally).

If you mix these up, you’ll set/clear the wrong bits in `imap`.

## Directory entry parsing bug in `read_dir_entries`

`read_dir_entries()` reconstructs the `name` bytes using `raw[2..62]`.

Given the on-disk layout `(u32 inode) + name[60]`, the name bytes should start at offset 4.
This likely corrupts directory names when using `read_dir_entries`.

File: `twilight_kernel/src/sys/fs/twilight_fs/mod.rs` (`read_dir_entries`)

## File type detection inconsistencies

Different code paths decide “file vs directory” differently:
- equality checks against `0o040777` / `0o100777`
- bitmask checks like `(mode & 0xF000) == 0x4000`
- installer sets root inode mode to `0o040755`, which may not match equality checks

If you see weird “dir shown as file” behavior, start here.

## `read_file` double-indirect size handling

In the double-indirect portion of `read_file`, the code appends an entire 2048-byte block to the output, even when only `to_read` bytes remain.
This can make the returned `Vec<u8>` longer than `inode.size`.

File: `twilight_kernel/src/sys/fs/twilight_fs/mod.rs` (`read_file`)

## `create_dir_entry` and uninitialized blocks

When a directory grows into a new zone, `create_dir_entry` allocates a zone and immediately scans it for `inode == 0` slots.
If the newly allocated zone is not zeroed, the scan can fail to find empty slots.

File: `twilight_kernel/src/sys/fs/twilight_fs/mod.rs` (`create_dir_entry`)

## Removal does not free indirect blocks

`remove_entry` frees only `inode.zones[]` (direct zones). It does not:
- free `indirect_zones` / `double_indirect_zones` blocks
- free any data zones reachable through those pointers

File: `twilight_kernel/src/sys/fs/twilight_fs/mod.rs` (`remove_entry`)

## Formatting does not zero metadata regions

`Superblock::write` only writes the superblock; it does not zero:
- `imap` / `zmap`
- inode table blocks

The installer currently creates the root inode and directories after writing the superblock, but the on-disk state of the bitmaps/inode table depends on the previous contents of the partition unless those blocks are explicitly initialized elsewhere.

Files:
- `twilight_kernel/src/sys/fs/twilight_fs/superblock.rs`
- `twilight_kernel/src/kernel_utils/install.rs`

## Unused / in-progress structures

The `twilight_kernel/src/sys/fs/twilight_fs/` directory also contains some “future design” structs that are not wired into the current filesystem path:
- `blockgroup.rs` (`BlockGroupHeader`)
- `metadata.rs` (`MetadataBlock`, `TreeNode`)

These look like the beginnings of a more ext-like “block group + metadata tree” design, but they are not part of today’s on-disk format.
