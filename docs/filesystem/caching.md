# Caching (TwilightFS)

TwilightFS includes a small in-memory caching layer designed to speed up repeated path lookups and reads of small files.

Relevant code:
- `twilight_kernel/src/sys/fs/twilight_fs/mod.rs` (`TwilightFsShared`, `PathLookupCache`, `FileContentCache`)

## The key idea: a global “generation” counter

`TwilightFsShared` contains:
- `generation: AtomicUsize`

Any operation that mutates filesystem-visible state calls:
- `shared.invalidate_all()` → increments `generation`

Each cache stores the generation number it was built under. When accessed:
- if the generation doesn’t match, the cache clears itself completely

This is a blunt but very effective strategy for a hobby OS:
- no per-inode invalidation bookkeeping
- very low risk of serving stale data after writes
- easy to reason about

## Path lookup cache

- Key: canonical full path (string)
- Value: inode number (`u32`)
- Capacity: 1024
- Eviction: FIFO-ish queue (`VecDeque`) of inserted paths (not a true LRU)

`resolve_path()` fills this cache with intermediate prefixes too, so repeated operations like:
- `open("/usr/bin/ls")`
- `open("/usr/bin/cat")`
benefit from caching `/usr`, `/usr/bin`, etc.

## File content cache

- Key: inode number
- Value: full file contents (`Vec<u8>`)
- Max per file: 8 MiB
- Max total cache: 32 MiB
- Eviction: FIFO-ish queue of inode numbers (not a true LRU)

Used by:
- `TFSVfsNode::read` (if the file is small enough)

