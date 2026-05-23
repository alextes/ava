use crate::error;

pub(crate) fn run_upgrade() -> Result<(), error::Error> {
    let source_dir = env!("CARGO_MANIFEST_DIR");
    let cargo_toml = std::path::Path::new(source_dir).join("Cargo.toml");

    if !cargo_toml.exists() {
        return Err(error::Error::Provider(
            "source directory not found — this binary wasn't built from a local checkout. \
             for installed binaries, re-run the install script to update."
                .to_string(),
        ));
    }

    println!("building from source in {source_dir}...");

    let status = std::process::Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(source_dir)
        .status()
        .map_err(|e| error::Error::Provider(format!("failed to run cargo build: {e}")))?;

    if !status.success() {
        return Err(error::Error::Provider("cargo build failed".to_string()));
    }

    println!("build succeeded");

    #[cfg(unix)]
    if let Some(pid) = crate::config::read_pid_file() {
        println!("signaling running ava (pid {pid}) to restart...");
        let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGUSR1) };
        if ret == 0 {
            if let Err(e) = crate::db::Database::open()
                .and_then(|db| db.record_runtime_event("self_upgrade", "ava upgrade"))
            {
                println!("warning: failed to record restart event: {e}");
            }
            println!("done — ava will restart after finishing current work");
        } else {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ESRCH) {
                println!("pid {pid} not running — restart ava manually");
            } else {
                println!("failed to signal pid {pid}: {err}");
            }
        }
    } else {
        println!("no running ava process found (no PID file), skipping signal");
    }

    #[cfg(not(unix))]
    println!("signal-based restart not supported on this platform — restart ava manually");

    Ok(())
}
