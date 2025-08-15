BITS 64
GLOBAL _start

SECTION .data
    ; struct timespec { time_t tv_sec; long tv_nsec; }
    ; On x86-64: time_t = 8 bytes, long = 8 bytes
    req:
        dq 3                  ; tv_sec = 3 seconds
        dq 3000000000         ; tv_nsec = 3000000000 nanoseconds

SECTION .text
_start:
    ; nanosleep(&req, NULL)
    mov     rax, 35            ; syscall number for nanosleep
    lea     rdi, [rel req]     ; pointer to req struct
    xor     rsi, rsi           ; NULL for rem
    syscall

    ; exit(0)
    mov     rax, 60            ; syscall: exit
    xor     rdi, rdi           ; status = 0
    syscall
