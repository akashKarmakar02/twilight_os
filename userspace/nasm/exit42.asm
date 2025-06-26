BITS 64
GLOBAL _start

SECTION .text
_start:
    mov     rax, 60     ; syscall: exit
    mov     rdi, 42     ; status code
    syscall