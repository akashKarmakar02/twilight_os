; echo.asm — Linux x86-64 NASM
; Prints argv[1..] separated by spaces, then newline.

BITS 64
GLOBAL _start

SECTION .text
_start:
    mov     rbx, [rsp]         ; rbx = argc
    lea     r13, [rsp+8]       ; r13 = &argv[0]  (KEEP THIS BASE SAFE)

    cmp     rbx, 1
    jle     .newline           ; no args -> just newline

    mov     r12, 1             ; r12 = i (start at argv[1])

.print_arg:
    mov     r8,  [r13 + r12*8] ; r8 = argv[i]
    mov     rdi, r8            ; strlen(argv[i])
    call    strlen

    ; write(1, argv[i], len)
    mov     rdx, rax           ; len
    mov     rax, 1             ; sys_write
    mov     rdi, 1             ; fd = stdout
    mov     rsi, r8            ; buf
    syscall                    ; rcx/r11 clobbered (ok)

    ; if (i < argc-1) write space
    mov     rax, rbx
    dec     rax                ; rax = argc - 1
    cmp     r12, rax
    jge     .after_space

    mov     rax, 1             ; sys_write
    mov     rdi, 1
    lea     rsi, [rel spc]     ; DO NOT overwrite r13
    mov     rdx, 1
    syscall

.after_space:
    inc     r12
    cmp     r12, rbx
    jl      .print_arg

.newline:
    mov     rax, 1             ; write "\n"
    mov     rdi, 1
    lea     rsi, [rel nl]
    mov     rdx, 1
    syscall

    mov     rax, 60            ; exit(0)
    xor     rdi, rdi
    syscall

; rdi = cstring -> rax = length
strlen:
    xor     rax, rax
.len_loop:
    cmp     byte [rdi+rax], 0
    je      .len_done
    inc     rax
    jmp     .len_loop
.len_done:
    ret

SECTION .rodata
spc: db ' '
nl:  db 10
