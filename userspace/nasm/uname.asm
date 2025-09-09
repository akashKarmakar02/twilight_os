; uname.asm — Linux-like uname in pure x86-64 NASM (syscall-only)
; Builds: nasm -felf64 uname.asm -o uname.o && ld -m elf_x86_64 -o uname uname.o

BITS 64
default rel

%define SYS_WRITE   1
%define SYS_UNAME   63
%define SYS_EXIT    60

%define STDOUT      1

%define UTSLEN      65
%define U_SYSNAME   (UTSLEN*0)
%define U_NODENAME  (UTSLEN*1)
%define U_RELEASE   (UTSLEN*2)
%define U_VERSION   (UTSLEN*3)
%define U_MACHINE   (UTSLEN*4)
%define U_DOMAIN    (UTSLEN*5)
%define UTS_SIZE    (UTSLEN*6)

%define FL_S  (1 << 0)
%define FL_N  (1 << 1)
%define FL_R  (1 << 2)
%define FL_V  (1 << 3)
%define FL_M  (1 << 4)
%define FL_P  (1 << 5)
%define FL_I  (1 << 6)
%define FL_O  (1 << 7)

section .bss
    utsbuf:     resb UTS_SIZE

section .data
    s_unknown:      db "unknown",0
    s_gnu_linux:    db "GNU/Linux",0
    spc:            db " "
    nl:             db 10

section .text
global _start
_start:
    ; argc / argv
    mov     rdi, [rsp]           ; argc
    lea     rsi, [rsp+8]         ; argv

    xor     ebx, ebx             ; flags = 0

    cmp     rdi, 1
    jle     .no_args

    mov     rcx, 1               ; i = 1
.arg_loop:
    cmp     rcx, rdi
    jge     .end_parse
    mov     rax, [rsi + rcx*8]   ; argv[i]
    test    rax, rax
    jz      .bad_operand

    cmp     byte [rax], '-'
    jne     .bad_operand
    inc     rax                  ; skip '-'

.opt_char:
    mov     dl, [rax]
    cmp     dl, 0
    je      .next_arg

    ; -a
    cmp     dl, 'a'
    jne     .chk_s
    mov     bl, FL_S | FL_N | FL_R | FL_V | FL_M | FL_P | FL_I | FL_O
    jmp     .adv

.chk_s:
    cmp     dl, 's'
    jne     .chk_n
    or      bl, FL_S
    jmp     .adv

.chk_n:
    cmp     dl, 'n'
    jne     .chk_r
    or      bl, FL_N
    jmp     .adv

.chk_r:
    cmp     dl, 'r'
    jne     .chk_v
    or      bl, FL_R
    jmp     .adv

.chk_v:
    cmp     dl, 'v'
    jne     .chk_m
    or      bl, FL_V
    jmp     .adv

.chk_m:
    cmp     dl, 'm'
    jne     .chk_p
    or      bl, FL_M
    jmp     .adv

.chk_p:
    cmp     dl, 'p'
    jne     .chk_i
    or      bl, FL_P
    jmp     .adv

.chk_i:
    cmp     dl, 'i'
    jne     .chk_o
    or      bl, FL_I
    jmp     .adv

.chk_o:
    cmp     dl, 'o'
    jne     .bad_option
    or      bl, FL_O
    ; fallthrough

.adv:
    inc     rax
    jmp     .opt_char

.next_arg:
    inc     rcx
    jmp     .arg_loop

.end_parse:
    test    bl, bl
    jnz     .do_uname

.no_args:
    or      bl, FL_S

.do_uname:
    mov     eax, SYS_UNAME
    lea     rdi, [rel utsbuf]
    syscall
    test    rax, rax
    js      .exit_err

    ; printed_any bool in EDI
    xor     edi, edi

    ; -s
    test    bl, FL_S
    jz      .chkN
    call    maybe_spc
    lea     rdi, [rel utsbuf + U_SYSNAME]
    call    write_uts_field
    mov     edi, 1

.chkN:
    test    bl, FL_N
    jz      .chkR
    call    maybe_spc
    lea     rdi, [rel utsbuf + U_NODENAME]
    call    write_uts_field
    mov     edi, 1

.chkR:
    test    bl, FL_R
    jz      .chkV
    call    maybe_spc
    lea     rdi, [rel utsbuf + U_RELEASE]
    call    write_uts_field
    mov     edi, 1

.chkV:
    test    bl, FL_V
    jz      .chkM
    call    maybe_spc
    lea     rdi, [rel utsbuf + U_VERSION]
    call    write_uts_field
    mov     edi, 1

.chkM:
    test    bl, FL_M
    jz      .chkP
    call    maybe_spc
    lea     rdi, [rel utsbuf + U_MACHINE]
    call    write_uts_field
    mov     edi, 1

.chkP:
    test    bl, FL_P
    jz      .chkI
    call    maybe_spc
    lea     rdi, [rel s_unknown]
    call    write_cstr
    mov     edi, 1

.chkI:
    test    bl, FL_I
    jz      .chkO
    call    maybe_spc
    lea     rdi, [rel s_unknown]
    call    write_cstr
    mov     edi, 1

.chkO:
    test    bl, FL_O
    jz      .done_print
    call    maybe_spc
    lea     rdi, [rel utsbuf + U_SYSNAME]
    call    is_linux
    test    eax, eax
    jz      .print_sysname_os
    lea     rdi, [rel s_gnu_linux]
    call    write_cstr
    jmp     .after_o

.print_sysname_os:
    lea     rdi, [rel utsbuf + U_SYSNAME]
    call    write_uts_field

.after_o:
    mov     edi, 1

.done_print:
    mov     eax, SYS_WRITE
    mov     edi, STDOUT
    lea     rsi, [rel nl]
    mov     edx, 1
    syscall

    xor     edi, edi
    mov     eax, SYS_EXIT
    syscall

.bad_option:
.bad_operand:
.exit_err:
    mov     edi, 1
    mov     eax, SYS_EXIT
    syscall

; -------- helpers --------

maybe_spc:
    test    edi, edi
    jz      .ret
    mov     eax, SYS_WRITE
    mov     edi, STDOUT
    lea     rsi, [rel spc]
    mov     edx, 1
    syscall
.ret:
    ret

write_cstr:
    push    rdi
    mov     rsi, rdi
    xor     ecx, ecx
.find0:
    cmp     byte [rsi], 0
    je      .len_ok
    inc     rsi
    inc     rcx
    jmp     .find0
.len_ok:
    mov     edx, ecx
    pop     rsi
    mov     edi, STDOUT
    mov     eax, SYS_WRITE
    syscall
    ret

write_uts_field:
    push    rdi
    mov     rsi, rdi
    xor     ecx, ecx
.scan:
    cmp     ecx, 64
    je      .got_len
    cmp     byte [rsi], 0
    je      .got_len
    inc     rsi
    inc     rcx
    jmp     .scan
.got_len:
    mov     edx, ecx
    pop     rsi
    mov     edi, STDOUT
    mov     eax, SYS_WRITE
    syscall
    ret

is_linux:
    mov     rsi, rdi
    mov     al, [rsi]
    cmp     al, 'L'
    jne     .no
    mov     al, [rsi+1]
    cmp     al, 'i'
    jne     .no
    mov     al, [rsi+2]
    cmp     al, 'n'
    jne     .no
    mov     al, [rsi+3]
    cmp     al, 'u'
    jne     .no
    mov     al, [rsi+4]
    cmp     al, 'x'
    jne     .no
    mov     al, [rsi+5]
    cmp     al, 0
    jne     .no
    mov     eax, 1
    ret
.no:
    xor     eax, eax
    ret
