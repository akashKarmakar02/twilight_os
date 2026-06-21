# Future init system plan

This document defines requirements for a future Twilight OS init and service manager. It deliberately does not implement PID 1, a logging daemon, or desktop services.

## Boot model

```text
kernel
  ↓
init (PID 1)
  ↓
system services
  ↓
user services
```

The kernel should launch exactly one userspace bootstrap process as PID 1. PID 1 becomes the root of userspace supervision, adopts orphaned descendants, and reaps zombies. It should remain small enough to recover essential services even when optional configuration is damaged.

## Required behavior

### Service startup

Init must load declarative service definitions, validate them before execution, create pipes and descriptors, fork, establish the requested process group/session, apply environment and working-directory settings, then `execve()` the service. Readiness should initially support simple states such as "process started" and later explicit notification over local IPC.

### Service shutdown

Shutdown should stop dependents before dependencies, send a configurable graceful signal to the service process group, wait for a bounded interval, then use `SIGKILL` if needed. Init must reap every child and unmount or sync storage only after critical services have stopped.

### Service restart and crash recovery

Every service needs a restart policy: never, on failure, or always. Restart loops require attempt counters, exponential backoff, a stable-runtime reset window, and a rate limit. Exit status and terminating signal must be retained for diagnostics. Failure of an optional service must not take down init; repeated failure of a boot-critical service should enter a defined recovery mode.

### Dependency ordering

Definitions should distinguish hard requirements from ordering-only relationships and optional wants. Init must reject dependency cycles with a useful diagnostic. Independent services should start concurrently once their prerequisites are ready. Shutdown ordering is the reverse dependency graph.

### Logging integration

Before a logging daemon exists, init may preserve stdout/stderr on the console or bounded kernel log. The eventual design should create stdout/stderr pipes before `execve()`, pass read ends to a logging service, attach service identity metadata, and define bounded buffering/backpressure behavior. Logging failure must not deadlock all supervised services.

## Proposed service state machine

```text
inactive → starting → running → stopping → inactive
               ↘ failed ↗
```

State transitions should be driven by child events from `waitpid()`, readiness IPC, administrative requests, and timeouts. A service record should retain PID, process-group ID, start generation, current state, last wait status, restart count, and dependency state.

## PID 1 responsibilities

- reap direct and adopted children with `waitpid(-1, ...)` until no completed child remains;
- map child PIDs to service generations without confusing a restarted service with its predecessor;
- place each service in a deliberate process group and signal the whole group on shutdown;
- close unused pipe ends after fork so EOF and hangup remain reliable;
- mark internal descriptors close-on-exec;
- expose a small local administrative IPC endpoint;
- handle shutdown/reboot requests and fatal configuration errors predictably;
- avoid unbounded allocation, unbounded log buffering, and blocking on one misbehaving service.

## Suggested implementation stages

1. A PID 1 reaper that launches one statically configured service and records wait status.
2. Multiple service definitions with process-group supervision and orderly shutdown.
3. Dependency graph validation, readiness notification, and restart policy.
4. A local control protocol and command-line client.
5. Logging-daemon integration with bounded pipe draining.
6. Separate system and per-user service managers for desktop infrastructure.

The process/session, pipe, poll/select, descriptor inheritance, and close-on-exec work described in `daemon_processes.md` forms the kernel ABI foundation for these stages.
