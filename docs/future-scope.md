# Future Scope

This document tracks planned future work for Twilight OS.

## Kernel & Scheduling

- [ ] Implement fully preemptive scheduling (timer-driven preemption, fair timeslice policy).
- [ ] Add priority-aware scheduler behavior and starvation safeguards.
- [ ] Improve task lifecycle handling (sleep/wakeup, blocking, termination).

## Multicore (SMP)

- [ ] Bring up multicore CPU support (AP startup, per-core state, cross-core coordination).
- [ ] Introduce per-CPU scheduling queues and load balancing.
- [ ] Add synchronization primitives needed for safe SMP operation.

## Filesystem

- [ ] Improve filesystem reliability and crash consistency.
- [ ] Strengthen caching, writeback, and block allocation behavior.
- [ ] Add fsck/recovery tooling and better corruption handling.

## IPC & Unix Features

- [ ] Add anonymous and named pipes.
- [ ] Implement Unix domain sockets.
- [ ] Expand process communication primitives and related syscalls.

## Userspace & Graphics

- [ ] Prepare userspace interfaces needed for GUI stacks.
- [ ] Evaluate and begin X11 porting path.
- [ ] Define compatibility requirements for running common Unix/Linux userland tools.

## Ports & Ecosystem

- [ ] Increase third-party software ports (shells, coreutils, build tools, editors).
- [ ] Document porting workflow and compatibility gaps.
- [ ] Maintain a tracked list of blocked vs working ports.

## Drivers & Hardware Support

- [ ] Expand storage, USB, network, and input driver support.
- [ ] Add broader graphics/display hardware support.
- [ ] Build a driver roadmap with device priority tiers.

## Notes

Use this section for open questions, design constraints, and links to design docs.
