    .global _start
    .extern main
    .extern environ
    .extern __environ
    .section .text
_start:
    pop     %rdi                # rdi = argc
    mov     %rsp, %rsi          # rsi = argv (argv[0] at *rsi)

    # rdx = envp = argv + (argc + 1)
    lea     8(%rsi,%rdi,8), %rdx

    # Publish envp to libc globals:
    mov     %rdx, environ(%rip)
    mov     %rdx, __environ(%rip)

    # Maintain SysV ABI: make RSP%16==8 before 'call' so that inside main it's 0
    sub     $8, %rsp

    call    main                # int main(int argc, char **argv, char **envp)

    add     $8, %rsp

    # exit(main_return_value)
    mov     %eax, %edi          # status -> edi
    mov     $60, %eax           # SYS_exit
    syscall
