# I/O paths (TwilightFS)

TwilightFS has two “layers” of file I/O code that are worth keeping separate in your head:

1) **Filesystem helpers** on `TwilightFs` (`read_file`, `write_file`) — mostly sequential, used by early utilities/installer-style code.
2) **VFS node operations** (`TFSVfsNode`) — supports offset reads/writes and is what the rest of the OS interacts with through the VFS.

Relevant code:
- `twilight_kernel/src/sys/fs/twilight_fs/mod.rs`
- `twilight_kernel/src/sys/fs/twilight_fs/inode.rs`
- `twilight_kernel/src/sys/fs/vfs.rs`

## Block I/O primitive: `read_tfs_block` / `write_tfs_block`

Every higher-level operation eventually turns into 2048-byte block reads/writes:
- translate `tfs_block_no` → `device_lba = partition_start + tfs_block_no * 4`
- call the underlying device’s `read_blocks` / `write_blocks`

## VFS reads: `TFSVfsNode::read`

The VFS read path:
- checks file size and clamps reads to EOF
- for “small files” (<= 8 MiB), it tries to serve from the in-memory file cache; if missing, it reads the whole file, caches it, and slices from it
- otherwise, it reads blocks directly by walking:
  - `zones[7]` (direct)
  - `indirect_zones` (single indirect)
  - `double_indirect_zones` (double indirect)

Offset handling:
- computes `start_block = offset / 2048`
- computes `block_offset = offset % 2048`
- skips blocks before `start_block`

## VFS writes: `TFSVfsNode::write`

The VFS write path supports offset writes and allocates blocks on demand:

- **Direct**: uses `zones[block_idx]`, allocates and zeroes a zone if missing.
- **Single indirect**:
  - allocates the indirect pointer block if missing
  - reads it, allocates data zones as needed, writes back updated pointers
- **Double indirect**:
  - allocates the root pointer block if missing
  - allocates indirect blocks as needed
  - allocates data zones as needed

For partial-block writes it preserves existing data:
- read existing block → patch the slice → write the full 2048-byte block back

After writing:
- inode size is updated to `max(old_size, offset + bytes_written)`
- the inode is persisted via `ctx.write_inode_twilight`
- caches are invalidated (`shared.invalidate_all`)

## Truncation: `TFSVfsNode::truncate`

- Shrink: updates `inode.size` and writes the inode back; it does not free zones.
- Extend: writes zero bytes until the file reaches `len` (which triggers allocation through `write`).

## TwilightFs helpers: `write_file` / `read_file`

These functions operate on inode numbers and do their own zone-walk:

- `write_file(inode, data)`:
  - writes from the beginning (no offset argument)
  - allocates direct zones as needed, then single indirect, then double indirect
  - sets `inode.size = bytes_written`

- `read_file(inode)`:
  - reads direct, then single indirect, then double indirect and concatenates into a `Vec<u8>`

If you’re debugging behavior differences, check which path your caller uses.

