# Directories (TwilightFS)

Directories in TwilightFS are just files whose data blocks contain an array of fixed-size directory entries.

Relevant code:
- `twilight_kernel/src/sys/fs/twilight_fs/mod.rs` (`DirEntry`, `create_dir_entry`, `find_dir_entry`, `list_dir`, `create_dir`, `create_file`)

## Directory entry format

Each entry is exactly 64 bytes:

```
struct DirEntry {
  inode: u32,
  name: [u8; 60],
}
```

Rules:
- `inode == 0` means “empty slot”.
- `name` is a UTF-8-ish byte string, NUL padded.
- Names longer than 60 bytes are rejected by most creation paths.

## Where entries are stored

The directory inode’s data zones (usually `zones[0]`, then more direct zones as it grows) contain packed `DirEntry` records. The directory inode’s `size` is treated as “bytes of directory entries in use”, and is updated by `create_dir_entry`.

## Creating an entry (`create_dir_entry`)

High-level flow:
1) Read the parent inode.
2) For each direct zone in `parent_inode.zones[]`:
   - If zone is 0, allocate a zone and write the inode back.
   - Read the zone block into memory.
   - Scan entries until an empty slot (`inode == 0`) is found.
3) Write the new `DirEntry` bytes into that slot.
4) Increase the directory inode’s `size` by 64 bytes and write the inode back.

This is simple and fast for small directories, but note:
- It only grows across **direct zones** for directory contents.
- Newly allocated directory blocks should be zeroed; otherwise “empty slots” may not be detectable if the disk contains garbage.

## Looking up an entry (`find_dir_entry`) and resolving paths (`resolve_path`)

Path resolution:
- starts from inode `1` (root)
- splits the path into components
- for each component:
  - scans the current directory’s direct zones for a matching name
  - returns the child inode number if found

`TwilightFs::resolve_path` also populates a path→inode cache along the way (see `docs/filesystem/caching.md`).

## `mkdir`, `touch`, `rm`, `rmdir`

Creation:
- `create_dir(parent_ino, name)`:
  - allocates a new inode and one data zone
  - writes a directory inode with mode `0o040777`
  - inserts entry into the parent directory
  - writes `.` and `..` entries into the new directory
- `create_file(parent_ino, name)`:
  - allocates a new inode and one data zone
  - writes a file inode with mode `0o100777`
  - inserts entry into the parent directory

Removal:
- `remove_entry(path)`:
  - finds `(parent_dir, name)` by splitting the path
  - clears the matching directory entry slot
  - deallocates the target inode and frees its **direct** zones
  - used by both `rm` and `rmdir` in the current `FileSystem` implementation

Semantics note:
- `rmdir` currently reuses `remove_entry` and does not enforce “directory must be empty” semantics.
- freeing is not recursive and does not free indirect/double-indirect blocks today.

