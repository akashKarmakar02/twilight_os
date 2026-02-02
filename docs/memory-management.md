# Memory Management

This document summarizes how Twilight OS manages physical and virtual memory, the kernel heap, and per-process address spaces.

## Boot-time memory setup
Twilight OS uses Limine to obtain:
- The physical memory map
- The Higher Half Direct Map (HHDM) offset

During `memory::init()`:
- The current CR3 page table is captured as the kernel page table
- The `OffsetPageTable` mapper is initialized using the HHDM offset
- The bitmap frame allocator is initialized from the Limine memory map
- The kernel heap is mapped and the allocator is initialized

Key files:
- `twilight_kernel/src/sys/memory/mod.rs`
- `twilight_kernel/src/sys/memory/bitmap.rs`
- `twilight_kernel/src/sys/memory/heap.rs`

## Physical frame allocator (bitmap)
The physical memory allocator uses a bitmap of 4 KiB frames.

Highlights:
- Tracks up to 32 usable regions (`MAX_REGIONS`)
- Places the bitmap inside the first usable region large enough to hold it
- Uses a `next_free_index` scan pointer for fast allocation
- Supports contiguous allocations for DMA via `allocate_contiguous(num_pages)`

Allocation is O(n) in the number of frames but works well for modest memory sizes.

## Virtual memory mapping
The kernel uses `x86_64::structures::paging::OffsetPageTable` with the HHDM offset.

Important helpers:
- `alloc_pages()`: map and zero pages, optionally writable/executable
- `dealloc_pages()`: unmap and free frames
- `map_mmio()`: map device memory as uncached
- `map_kernel_buffer()`: map a kernel buffer into user space (used by shared memory devices)

Page flags are derived via `make_flags()`:
- Always `PRESENT` and `USER_ACCESSIBLE`
- Adds `WRITABLE` and `NO_EXECUTE` based on parameters

## Kernel heap
The kernel heap uses `linked_list_allocator::LockedHeap`:

- Heap start: `0x4444_4444_0000`
- Heap mapping size: 100 MiB
- The allocator is initialized with `memory_size()` (total RAM computed from the memory map)

Note: mapping 100 MiB but initializing with total RAM means the allocator believes the heap is larger than the mapped region. This is a known mismatch to be aware of in future cleanup.

## DMA buffers (PhysBuf)
Drivers that need physically contiguous memory use `PhysBuf`:
- Allocates contiguous frames from the bitmap allocator
- Returns both physical and virtual (HHDM) addresses
- Used by drivers like VirtIO and xHCI

File: `twilight_kernel/src/sys/memory/phys.rs`

## Per-process address space
Each user process has its own page table. A new page table is created by cloning the kernel page table entries, then user mappings are added.

Key constants:
- Page size: 4096 bytes
- User stack top: `0x0000_7FFF_FFFF_F000`
- User stack size: `0x64000`
- User mmap range: `0x0000_0000_4000_0000` to `0x0000_7FFF_F000_0000`

Per-process memory state is tracked in `ProcMM`:
- `heap_start`: start of the process heap
- `brk_cur`: current program break
- `mapped_heap_end`: highest page currently mapped for the heap
- `mmap_regions`: tracked mmap allocations

## brk(2)
`brk()` adjusts the program break. If the new break grows beyond the mapped heap, new pages are mapped via `alloc_pages()`.

## mmap(2)
`mmap()` supports:
- Anonymous mappings (`MAP_ANONYMOUS`)
- File-backed mappings (if a VFS node supports mmap)
- `MAP_FIXED`
- Basic `PROT_READ/WRITE/EXEC` flags

If a file node does not implement mmap, the kernel falls back to mapping pages and reading file contents into them. Mapped regions are tracked in `ProcMM` so that `munmap()` can undo them.

## munmap(2)
`munmap()` removes a tracked region. Owned mappings free frames; shared mappings are only unmapped from the page table.

## /proc/meminfo
Memory stats are exposed via procfs and are derived from the bitmap allocator:
- Total: `total_frames * 4096`
- Free: `free_frames * 4096`

See `twilight_kernel/src/sys/fs/procfs/nodes.rs`.
