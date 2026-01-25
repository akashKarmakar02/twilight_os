use std::env;
use std::fs;
use std::io::{self};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Simple multi-language builder for apps/ directory.
///
/// Behavior:
/// - iterate immediate children of `apps/` (each app must be a directory)
/// - detect project type: Cargo.toml (rust), build.zig or zig.* (zig), Makefile (c/c++), or C files
/// - run appropriate build command (static binaries using musl when applicable)
/// - discover newly created executables inside the app dir and copy them into `../rootfs/bin`
///
/// Usage:
///   cargo run --bin os-userspace-builder -- [apps-dir] [out-bin-dir]
/// Defaults: apps -> "apps" and out -> "../rootfs/bin"

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=.");
    let args: Vec<String> = env::args().collect();
    let apps_dir = Path::new(args.get(1).map(String::as_str).unwrap_or("apps"));
    let out_bin_dir = Path::new(args.get(2).map(String::as_str).unwrap_or("../rootfs/bin"));

    fs::create_dir_all(out_bin_dir)?;

    if !apps_dir.exists() {
        eprintln!("apps directory not found: {}", apps_dir.display());
        std::process::exit(1);
    }

    for entry in fs::read_dir(apps_dir)? {
        let entry = entry?;
        let path = entry.path();
        println!("path: {:?}", path);
        if !path.is_dir() {
            continue;
        }
        let app_name = path.file_name().unwrap().to_string_lossy().to_string();
        println!("\n=== Building app: {} ===", app_name);

        // Snapshot existing executables so we can detect newly created ones
        let before = list_executables(&path)?;

        // Detect project type and run build
        if path.join("Cargo.toml").exists() {
            build_rust(&path)?;
        } else if has_zig_project(&path) {
            build_zig(&path)?;
        } else if path.join("Makefile").exists() || path.join("makefile").exists() {
            build_make(&path)?;

            let app_name = path.file_name().unwrap().to_string_lossy().to_string();

            // common candidate locations (top-level, bin/, build/, target/)
            let mut candidates = vec![
                path.join(&app_name),
                path.join("bin").join(&app_name),
                path.join("build").join(&app_name),
                path.join("out").join(&app_name),
                path.join(format!("target/release/{}", app_name)),
                path.join(format!("target/debug/{}", app_name)),
            ];

            // also check for plain `a.out` if someone compiled without -o
            candidates.push(path.join("a.out"));

            // canonicalize & pick the first candidate that exists and is a regular file
            let mut src_file: Option<PathBuf> = None;
            for c in &candidates {
                if c.exists() && c.is_file() {
                    src_file = fs::canonicalize(c).ok();
                    break;
                }
            }

            // fallback: use the difference between after and before (new executables)
            if src_file.is_none() {
                let after = list_executables(&path)?;
                let new_execs: Vec<_> = after.difference(&before).cloned().collect();
                if !new_execs.is_empty() {
                    // pick first new executable
                    src_file = Some(new_execs[0].clone());
                }
            }

            if let Some(src) = src_file {
                let dst = fs::canonicalize(out_bin_dir).unwrap_or_else(|_| out_bin_dir.to_path_buf()).join(src.file_name().unwrap());
                println!("Copying {} -> {}", src.display(), dst.display());
                if let Err(e) = fs::copy(&src, &dst) {
                    eprintln!("ERROR copying {} -> {} : {}", src.display(), dst.display(), e);
                } else {
                    // set executable perms
                    if let Ok(mut perms) = fs::metadata(&dst).map(|m| m.permissions()) {
                        perms.set_mode(0o755);
                        let _ = fs::set_permissions(&dst, perms);
                        fs::remove_file(src)?;
                    }
                }
            } else {
                eprintln!("ERROR: no binary found for app {} after running make. Tried candidates: {:?}", app_name, candidates);
            }
        } else if has_c_sources(&path) {
            build_c_simple(&path)?;
        } else {
            println!("No recognized build system for {} - skipping", app_name);
            continue;
        }

        // Find new executables and copy them into out_bin_dir
        let after = list_executables(&path)?;
        let new_execs: Vec<_> = after.difference(&before).collect();

        for exe in new_execs {
            let src = exe;
            let filename = src.file_name().unwrap();
            let dst = out_bin_dir.join(filename);
            println!("Copying {} -> {}", src.display(), dst.display());
            fs::copy(src, &dst)?;
            // ensure owner-exec bits are kept
            let mut perms = fs::metadata(&dst)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&dst, perms)?;
        }
    }

    println!("\nAll done.");
    Ok(())
}

use std::collections::HashSet;

fn list_executables(dir: &Path) -> io::Result<HashSet<PathBuf>> {
    let mut set = HashSet::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_file() {
            if let Ok(meta) = fs::metadata(&p) {
                let mode = meta.permissions().mode();
                if mode & 0o111 != 0 {
                    set.insert(p);
                }
            }
        }
    }
    Ok(set)
}

fn has_zig_project(path: &Path) -> bool {
    path.join("build.zig").exists() || path.join("zig.toml").exists()
}

fn has_c_sources(path: &Path) -> bool {
    for entry in fs::read_dir(path).unwrap_or_else(|_| fs::read_dir(path).unwrap()) {
        if let Ok(e) = entry {
            if let Some(ext) = e.path().extension().and_then(|s| s.to_str()) {
                if ext == "c" || ext == "cpp" || ext == "cc" || ext == "cxx" {
                    return true;
                }
            }
        }
    }
    false
}

fn build_rust(path: &Path) -> io::Result<()> {
    println!("Detected Rust project (Cargo.toml). Running cargo build (musl target)");
    // Use x86_64-unknown-linux-musl target; make sure the target is installed on the system
    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("--target")
        .arg("x86_64-unknown-linux-musl")
        .current_dir(path)
        .status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, "cargo build failed"));
    }
    Ok(())
}

fn selected_cc() -> String {
    env::var("CC").unwrap_or_else(|_| "musl-gcc".to_string())
}

fn no_pie_enabled() -> bool {
    // Alpine toolchains typically default to PIE; static userland binaries for Twilight OS should be non-PIE.
    // Allow disabling this behavior: TWILIGHT_NO_PIE=0
    env::var("TWILIGHT_NO_PIE").ok().as_deref() != Some("0")
}

fn cc_for_make(base_cc: &str) -> String {
    // Don't break complex CC values that include args (e.g. "ccache gcc").
    if !no_pie_enabled() {
        return base_cc.to_string();
    }
    if base_cc.split_whitespace().count() != 1 {
        return base_cc.to_string();
    }
    if base_cc == "musl-gcc" {
        return base_cc.to_string();
    }
    // Fix "relocation ... can not be used when making a PIE object" on Alpine by disabling PIE.
    format!("{} -fno-pie -no-pie", base_cc)
}

fn build_zig(path: &Path) -> io::Result<()> {
    println!("Detected Zig project. Running zig build for musl target (if available)");
    // Try a common zig build invocation that targets musl
    let status = Command::new("zig")
        .arg("build")
        .arg("-Drelease-fast")
        .arg("-Dtarget=x86_64-linux-musl")
        .current_dir(path)
        .status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, "zig build failed"));
    }
    Ok(())
}

fn build_make(path: &Path) -> io::Result<()> {
    let cc = selected_cc();
    let make_cc = cc_for_make(&cc);
    if make_cc == cc {
        println!("Detected Makefile. Running make with CC={} (default musl-gcc)", cc);
    } else {
        println!(
            "Detected Makefile. Running make with CC={} (from CC={})",
            make_cc, cc
        );
    }

    // Some external repos (e.g. TinyCC) require a configure step to generate config.mak
    // with the correct compiler/toolchain settings. Environment CC alone is not enough
    // because Makefiles commonly override environment variables.
    maybe_configure_before_make(path, &cc)?;

    let mut cmd = Command::new("make");
    cmd.arg("-C").arg(path).env("CC", &make_cc);
    apply_ccache_env(&mut cmd);
    let status = cmd.status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, "make failed"));
    }
    Ok(())
}

fn maybe_configure_before_make(path: &Path, cc: &str) -> io::Result<()> {
    let app_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    // Currently only TinyCC needs this in-tree configure pass.
    if app_name != "tcc" {
        return Ok(());
    }

    let configure = path.join("configure");
    if !configure.exists() {
        return Ok(());
    }

    // Only run configure if config.mak is missing or isn't set up for the selected CC.
    let config_mak = path.join("config.mak");
    let needle = format!("CC={}", cc);
    let needs_configure = match fs::read_to_string(&config_mak) {
        Ok(s) => !s.lines().any(|l| l.trim() == needle.as_str()),
        Err(_) => true,
    };

    if !needs_configure {
        return Ok(());
    }

    // Ensure `./configure` is executable.
    if let Ok(meta) = fs::metadata(&configure) {
        let mut perms = meta.permissions();
        if perms.mode() & 0o111 == 0 {
            perms.set_mode(perms.mode() | 0o111);
            let _ = fs::set_permissions(&configure, perms);
        }
    }

    println!("Detected tcc configure script. Running ./configure --cc={}", cc);
    let mut cmd = Command::new("./configure");
    cmd.arg(format!("--cc={}", cc))
        .current_dir(path)
        .env("CC", cc);
    apply_ccache_env(&mut cmd);
    let status = cmd.status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, "configure failed"));
    }
    Ok(())
}

fn build_c_simple(path: &Path) -> io::Result<()> {
    let cc = selected_cc();
    println!(
        "No Makefile but found C sources. Compiling single-file apps with {} (default musl-gcc)",
        cc
    );
    let no_pie = no_pie_enabled();
    // Collect top-level .c files and compile each into app-named binary
    let mut c_files = Vec::new();
    for entry in fs::read_dir(path)? {
        let e = entry?;
        if let Some(ext) = e.path().extension().and_then(|s| s.to_str()) {
            if ext == "c" {
                c_files.push(e.path());
            }
        }
    }
    if c_files.is_empty() {
        return Ok(());
    }
    // if there is exactly one C file, produce binary named after directory
    if c_files.len() == 1 {
        let out = path.join(path.file_name().unwrap());
        let mut cmd = Command::new(&cc);
        cmd.arg("-static");
        if no_pie {
            cmd.arg("-fno-pie").arg("-no-pie");
        }
        cmd
            .arg(c_files[0].to_str().unwrap())
            .arg("-o")
            .arg(&out)
            .current_dir(path);
        apply_ccache_env(&mut cmd);
        let status = cmd.status()?;
        if !status.success() {
            return Err(io::Error::new(io::ErrorKind::Other, "musl-gcc failed"));
        }
    } else {
        // multiple C files: try to build with musl-gcc into a.out or a directory binary named after app
        let out = path.join(path.file_name().unwrap());
        let mut args = vec!["-static".to_string()];
        if no_pie {
            args.push("-fno-pie".to_string());
            args.push("-no-pie".to_string());
        }
        for f in &c_files {
            args.push(f.to_string_lossy().to_string());
        }
        args.push("-o".to_string());
        args.push(out.to_string_lossy().to_string());

        let mut cmd = Command::new(&cc);
        cmd.args(&args).current_dir(path);
        apply_ccache_env(&mut cmd);
        let status = cmd.status()?;
        if !status.success() {
            return Err(io::Error::new(io::ErrorKind::Other, "musl-gcc failed"));
        }
    }
    Ok(())
}

fn apply_ccache_env(cmd: &mut Command) {
    // Some setups place `ccache` in front of compilers; the default temp dir can be unwritable
    // under sandboxing. Point it at a workspace-writable directory.
    let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") else {
        return;
    };

    let base = PathBuf::from(manifest_dir).join("target");
    let ccache_dir = base.join("ccache");
    let ccache_tmpdir = base.join("ccache-tmp");
    let _ = fs::create_dir_all(&ccache_dir);
    let _ = fs::create_dir_all(&ccache_tmpdir);

    cmd.env("CCACHE_DIR", ccache_dir);
    cmd.env("CCACHE_TEMPDIR", ccache_tmpdir);
}
