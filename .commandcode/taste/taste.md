# Coding Style
- Prefer clean, simple, and maintainable code over complex or clever solutions. Confidence: 0.85
- Use serial_println! for kernel-level debugging output. Confidence: 0.65

# Rust
- Use std for userspace apps (not no_std), following the twinit precedent. Confidence: 0.65

# Workflow
- User provides detailed markdown implementation plans as specifications; implement directly from these plans. Confidence: 0.80
- User may request investigation without code changes ("no code update / only find"); honor this by analyzing and reporting without modifying code. Confidence: 0.80
- Use `make twilight-os.iso` from the root GNUmakefile to build the project, not cargo build directly in subdirectories. Confidence: 0.65

# Kernel
- Match Linux behavior exactly as the reference implementation for syscalls, error codes, and OS semantics. When in doubt, follow what Linux does. Confidence: 1.00
