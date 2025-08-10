BITS 64
GLOBAL _start

SECTION .bss
    buffer  resb 128     ; reserve 128 bytes for input

SECTION .data
    prompt      db  "Enter your name: "
    prompt_len  equ $ - prompt

    hello       db  "Hello, "
    hello_len   equ $ - hello

    newline     db  10
    exclamation db  "!", 10

SECTION .text
_start:
    ; --- write "Enter your name: "
    mov     rax, 1              ; syscall: write
    mov     rdi, 1              ; fd = stdout
    mov     rsi, prompt         ; pointer to prompt message
    mov     rdx, prompt_len     ; prompt length
    syscall

    ; --- read user input
    mov     rax, 0              ; syscall: read
    mov     rdi, 0              ; fd = stdin
    mov     rsi, buffer         ; pointer to buffer
    mov     rdx, 128            ; max bytes to read
    syscall                     ; RAX = number of bytes read

    ; save bytes read in r8 (we need to remove the newline)
    mov     r8, rax
    dec     r8                  ; subtract 1 to remove newline from count

    ; --- write "Hello, "
    mov     rax, 1              ; syscall: write
    mov     rdi, 1              ; fd = stdout
    mov     rsi, hello          ; pointer to "Hello, "
    mov     rdx, hello_len      ; length of "Hello, "
    syscall

    ; --- write user input (without the newline)
    mov     rax, 1              ; syscall: write
    mov     rdi, 1              ; fd = stdout
    mov     rsi, buffer         ; pointer to buffer
    mov     rdx, r8             ; number of bytes (without newline)
    syscall

    mov     rax, 1              ; syscall: write
    mov     rdi, 1              ; fd = stdout
    mov     rsi, exclamation    ; pointer to "!\n"
    mov     rdx, 2              ; length of "!\n"
    syscall

    ; --- exit(0)
    mov     rax, 60             ; syscall: exit
    xor     rdi, rdi            ; status = 0
    syscall