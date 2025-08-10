BITS 64
GLOBAL _start

SECTION .bss
    buffer  resb 128     ; reserve 128 bytes for input

SECTION .data
    msg     db  "Hello, world", 10
    msg_len equ $ - msg

SECTION .text
_start:
    ; --- write "Hello, world\n"
    mov     rax, 1              ; syscall: write
    mov     rdi, 1              ; fd = stdout
    mov     rsi, msg            ; pointer to message
    mov     rdx, msg_len        ; message length
    syscall

    ; --- read user input
    mov     rax, 0              ; syscall: read
    mov     rdi, 0              ; fd = stdin
    mov     rsi, buffer         ; pointer to buffer
    mov     rdx, 128            ; max bytes to read
    syscall                     ; RAX = number of bytes read

    ; save bytes read in rdx
    mov     rdx, rax

    ; --- write user input back to stdout
    mov     rax, 1              ; syscall: write
    mov     rdi, 1              ; fd = stdout
    mov     rsi, buffer         ; pointer to buffer
    syscall

    ; --- exit(0)
    mov     rax, 60             ; syscall: exit
    xor     rdi, rdi            ; status = 0
    syscall
