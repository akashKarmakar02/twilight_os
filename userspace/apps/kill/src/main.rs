use std::env;
use std::process;

fn signal_number(name: &str) -> Option<i32> {
    if let Ok(n) = name.parse::<i32>() {
        if n > 0 && n < 65 { return Some(n); }
        return None;
    }
    match name.to_uppercase().as_str() {
        "HUP" | "SIGHUP" => Some(1),
        "INT" | "SIGINT" => Some(2),
        "QUIT" | "SIGQUIT" => Some(3),
        "ILL" | "SIGILL" => Some(4),
        "TRAP" | "SIGTRAP" => Some(5),
        "ABRT" | "SIGABRT" => Some(6),
        "BUS" | "SIGBUS" => Some(7),
        "FPE" | "SIGFPE" => Some(8),
        "KILL" | "SIGKILL" => Some(9),
        "USR1" | "SIGUSR1" => Some(10),
        "SEGV" | "SIGSEGV" => Some(11),
        "USR2" | "SIGUSR2" => Some(12),
        "PIPE" | "SIGPIPE" => Some(13),
        "ALRM" | "SIGALRM" => Some(14),
        "TERM" | "SIGTERM" => Some(15),
        "CHLD" | "SIGCHLD" => Some(17),
        "CONT" | "SIGCONT" => Some(18),
        "STOP" | "SIGSTOP" => Some(19),
        "TSTP" | "SIGTSTP" => Some(20),
        _ => None,
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: kill [-<signal>] <pid>...");
        process::exit(1);
    }

    let mut sig: i32 = 15; // SIGTERM
    let mut first_arg = 1;

    if args[1].starts_with('-') && args.len() > 2 {
        match signal_number(&args[1][1..]) {
            Some(n) => {
                sig = n;
                first_arg = 2;
            }
            None => {
                eprintln!("kill: invalid signal specification");
                process::exit(1);
            }
        }
    }

    let mut any_failed = false;
    for arg in &args[first_arg..] {
        let pid: i32 = match arg.parse() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("kill: invalid PID");
                process::exit(1);
            }
        };

        // SYS_kill = 62
        let ret = unsafe {
            let ret: i64;
            core::arch::asm!(
                "syscall",
                in("rax") 62i64,
                in("rdi") pid as i64,
                in("rsi") sig as i64,
                in("rdx") 0i64,
                lateout("rax") ret,
                clobber_abi("C"),
            );
            ret
        };

        if ret < 0 {
            any_failed = true;
        }
    }

    if any_failed {
        process::exit(1);
    }
}
