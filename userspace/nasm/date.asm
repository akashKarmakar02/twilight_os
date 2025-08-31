; build (Linux): nasm -felf64 ist_time.asm && ld -o ist_time ist_time.o
; run: ./ist_time
; Notes:
; - Uses only syscalls: time(2)=201, write(1), exit(60)
; - Timezone fixed to IST (UTC+05:30), no DST
; - Output format: Ddd Mmm DD hh:mm:ss AM/PM IST YYYY\n

BITS 64
GLOBAL _start

SECTION .data
    ; Month lengths for non-leap year
    month_days:    dq 31,28,31,30,31,30,31,31,30,31,30,31

    ; Day-of-week abbreviations (0=Sun)
    d0: db 'Sun'
    d1: db 'Mon'
    d2: db 'Tue'
    d3: db 'Wed'
    d4: db 'Thu'
    d5: db 'Fri'
    d6: db 'Sat'
    days_ptrs:     dq d0, d1, d2, d3, d4, d5, d6

    ; Month abbreviations (0=Jan)
    m0: db 'Jan'
    m1: db 'Feb'
    m2: db 'Mar'
    m3: db 'Apr'
    m4: db 'May'
    m5: db 'Jun'
    m6: db 'Jul'
    m7: db 'Aug'
    m8: db 'Sep'
    m9: db 'Oct'
    mA: db 'Nov'
    mB: db 'Dec'
    months_ptrs:   dq m0, m1, m2, m3, m4, m5, m6, m7, m8, m9, mA, mB

    str_AM:  db 'AM'
    str_PM:  db 'PM'
    str_IST: db 'IST'

SECTION .bss
    outbuf:    resb 64

    var_year:  resq 1
    var_mon:   resq 1     ; 0..11
    var_mday:  resq 1     ; 1..31
    var_dow:   resq 1     ; 0=Sun..6=Sat
    var_hour:  resq 1     ; 0..23
    var_min:   resq 1     ; 0..59
    var_sec:   resq 1     ; 0..59

SECTION .text

; ---------------------------
; uint64 is_leap(uint64 year)
; returns RAX = 1 if leap, 0 otherwise
; ---------------------------
is_leap:
    mov     rax, rdi
    xor     rdx, rdx
    mov     rcx, 4
    div     rcx
    test    rdx, rdx
    jne     .not_leap

    mov     rax, rdi
    xor     rdx, rdx
    mov     rcx, 100
    div     rcx
    test    rdx, rdx
    jne     .leap

    mov     rax, rdi
    xor     rdx, rdx
    mov     rcx, 400
    div     rcx
    test    rdx, rdx
    jne     .not_leap

.leap:
    mov     rax, 1
    ret
.not_leap:
    xor     rax, rax
    ret

; ---------------------------
; void write2(uint val, char **p)  ; writes 2 zero-padded digits
; IN: RAX=val (0..99), RDI=dest ptr
; OUT: RDI advanced by 2
; ---------------------------
write2:
    xor     rdx, rdx
    mov     rcx, 10
    div     rcx                 ; RAX=tens, RDX=ones
    add     al, '0'
    add     dl, '0'
    mov     [rdi], al
    mov     [rdi+1], dl
    add     rdi, 2
    ret

; ---------------------------
; void write4year(uint year, char **p) ; writes YYYY
; IN: RAX=year, RDI=dest
; OUT: RDI advanced by 4
; ---------------------------
write4year:
    xor     rdx, rdx
    mov     rcx, 1000
    div     rcx                 ; AL=thousands, RDX=rem
    add     al, '0'
    mov     [rdi], al
    inc     rdi

    mov     rax, rdx
    xor     rdx, rdx
    mov     rcx, 100
    div     rcx                 ; AL=hundreds, RDX=rem
    add     al, '0'
    mov     [rdi], al
    inc     rdi

    mov     rax, rdx
    xor     rdx, rdx
    mov     rcx, 10
    div     rcx                 ; AL=tens, DL=ones
    add     al, '0'
    mov     [rdi], al
    inc     rdi

    add     dl, '0'
    mov     [rdi], dl
    inc     rdi
    ret

_start:
    cld

    ; --- get current epoch seconds: time(NULL) ---
    mov     rax, 201            ; sys_time
    xor     rdi, rdi            ; NULL
    syscall                     ; RAX = seconds since 1970-01-01 UTC

    ; --- convert to IST (+5:30 => +19800 sec) ---
    add     rax, 19800

    ; days = secs / 86400 ; sod = secs % 86400
    xor     rdx, rdx
    mov     rbx, 86400
    div     rbx                 ; RAX=days since epoch, RDX=secs of day
    mov     r12, rax            ; days
    mov     r13, rdx            ; seconds-in-day (local)

    ; h = sod / 3600 ; r = sod % 3600
    mov     rax, r13
    xor     rdx, rdx
    mov     rbx, 3600
    div     rbx
    mov     [var_hour], rax     ; hour 0..23
    mov     r13, rdx

    ; m = r / 60 ; s = r % 60
    mov     rax, r13
    xor     rdx, rdx
    mov     rbx, 60
    div     rbx
    mov     [var_min], rax
    mov     [var_sec], rdx

    ; dow = (days + 4) % 7   ; (1970-01-01 = Thu)
    mov     rax, r12
    add     rax, 4
    xor     rdx, rdx
    mov     rbx, 7
    div     rbx
    mov     [var_dow], rdx

    ; ---- year & day-of-year ----
    mov     r14, 1970           ; year
    mov     rsi, r12            ; remaining days within current year

.yloop:
    mov     rdi, r14
    call    is_leap
    mov     rbx, 365
    test    rax, rax
    jz      .nonleapY
    mov     rbx, 366
.nonleapY:
    cmp     rsi, rbx
    jb      .yend
    sub     rsi, rbx
    inc     r14
    jmp     .yloop

.yend:
    mov     [var_year], r14

    ; ---- month & mday ----
    ; rsi = day_of_year (0-based)
    mov     rdi, r14
    call    is_leap             ; RAX=1 if leap
    mov     r15, rax            ; save leap flag (0/1)

    xor     r8, r8              ; month index 0..11
.mloop:
    mov     rbx, [month_days + r8*8]   ; base month days
    test    r15, r15
    jz      .noFebAdj
    cmp     r8, 1                      ; Feb?
    jne     .noFebAdj
    inc     rbx                         ; 28 -> 29
.noFebAdj:
    cmp     rsi, rbx
    jb      .mend
    sub     rsi, rbx
    inc     r8
    jmp     .mloop
.mend:
    mov     [var_mon], r8
    mov     rax, rsi
    inc     rax
    mov     [var_mday], rax            ; 1..31

    ; ---- format into outbuf ----
    mov     rdi, outbuf

    ; Day name (3)
    mov     rax, [var_dow]
    mov     rbx, [days_ptrs + rax*8]
    mov     rsi, rbx
    mov     rcx, 3
    rep     movsb
    mov     byte [rdi], ' '
    inc     rdi

    ; Month name (3)
    mov     rax, [var_mon]
    mov     rbx, [months_ptrs + rax*8]
    mov     rsi, rbx
    mov     rcx, 3
    rep     movsb
    mov     byte [rdi], ' '
    inc     rdi

    ; Day of month (2 digits, zero-padded)
    mov     rax, [var_mday]
    call    write2
    mov     byte [rdi], ' '
    inc     rdi

    ; ---- Hour:Minute:Second in 12-hour with AM/PM ----
    mov     rax, [var_hour]     ; hour 0..23

    cmp     rax, 12
    jb      .am_time            ; 0..11 -> AM
    ja      .pm_gt12            ; 13..23 -> PM
    ; ==12 -> PM, keep 12
    mov     rsi, str_PM
    jmp     .have_ampm_ptr

.pm_gt12:
    sub     rax, 12             ; 13..23 -> 1..11
    mov     rsi, str_PM
    jmp     .have_ampm_ptr

.am_time:
    test    rax, rax
    jnz     .am_keep
    mov     rax, 12             ; 0 -> 12 AM
.am_keep:
    mov     rsi, str_AM

.have_ampm_ptr:
    mov     r8, rsi             ; save "AM"/"PM" pointer

    ; write HH:MM:SS
    call    write2              ; hour
    mov     byte [rdi], ':'
    inc     rdi
    mov     rax, [var_min]
    call    write2
    mov     byte [rdi], ':'
    inc     rdi
    mov     rax, [var_sec]
    call    write2

    ; space + AM/PM  (rcx was clobbered by write2; restore it!)
    mov     byte [rdi], ' '
    inc     rdi
    mov     rsi, r8
    mov     rcx, 2
    rep     movsb

    ; space + IST
    mov     byte [rdi], ' '
    inc     rdi
    mov     rsi, str_IST
    mov     rcx, 3
    rep     movsb

    ; space + YYYY
    mov     byte [rdi], ' '
    inc     rdi
    mov     rax, [var_year]
    call    write4year

    ; newline
    mov     byte [rdi], 10
    inc     rdi

    ; ---- write(outbuf, len) ----
    mov     rsi, outbuf
    mov     rax, rdi
    sub     rax, rsi            ; len
    mov     rdx, rax
    mov     rax, 1              ; sys_write
    mov     rdi, 1              ; fd=stdout
    syscall

    ; exit(0)
    mov     rax, 60
    xor     rdi, rdi
    syscall
