.global _start
.extern main
.section .text
_start:
    pop %rdi          # argc
    mov %rsp, %rsi    # argv
    call main         # int main(int,char**)
    mov %rdi, %rax    # return -> rax
    mov $60, %rax     # SYS_exit
    syscall
    hlt
