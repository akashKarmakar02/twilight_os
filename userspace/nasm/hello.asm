BITS 64
GLOBAL _start

SECTION .data
    msg     db  "Hello, world", 10     ; 10 is newline '\n'
    msg_len equ $ - msg                ; length of the message

SECTION .text
_start:
    mov     rax, 1          ; syscall: write
    mov     rdi, 1          ; file descriptor: stdout
    mov     rsi, msg        ; pointer to message
    mov     rdx, msg_len    ; message length
    syscall

    ; exit syscall to terminate properly
    mov     rax, 60         ; syscall: exit
    xor     rdi, rdi        ; status: 0
    syscall
