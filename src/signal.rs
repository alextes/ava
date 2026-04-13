use std::sync::atomic::{AtomicBool, Ordering};

static RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);

/// install a SIGUSR1 handler that sets the restart flag when received.
#[cfg(unix)]
pub fn install_signal_handler() {
    tokio::spawn(async {
        let mut sig = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1())
            .expect("failed to install SIGUSR1 handler");
        loop {
            sig.recv().await;
            tracing::info!("SIGUSR1 received, will restart after current work completes");
            RESTART_REQUESTED.store(true, Ordering::Relaxed);
        }
    });
}

#[cfg(not(unix))]
pub fn install_signal_handler() {
    // signal-based restart not supported on this platform
}

/// returns true if a restart has been requested via SIGUSR1.
pub fn restart_requested() -> bool {
    RESTART_REQUESTED.load(Ordering::Relaxed)
}

/// exec into a fresh copy of the current binary with `start` arg.
/// on success this function never returns. on failure it clears the flag
/// and returns an error so the caller can continue running.
#[cfg(unix)]
fn resolve_restart_exe() -> Result<std::path::PathBuf, std::io::Error> {
    if let Ok(path) = std::env::var("AVA_EXEC_PATH") {
        return Ok(std::path::PathBuf::from(path));
    }

    let source_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    if source_dir.join("Cargo.toml").exists() {
        return Ok(source_dir.join("target/release/ava"));
    }

    let exe = std::env::current_exe()?;
    let exe_str = exe.to_string_lossy();
    Ok(std::path::PathBuf::from(exe_str.trim_end_matches(" (deleted)")))
}

pub fn do_exec_restart() -> Result<std::convert::Infallible, std::io::Error> {
    use std::os::unix::process::CommandExt;

    let exe = resolve_restart_exe()?;
    tracing::info!(?exe, "exec'ing into new binary");

    let err = std::process::Command::new(&exe)
        .args(["start", "--foreground"])
        .exec();

    // exec() only returns on error
    RESTART_REQUESTED.store(false, Ordering::Relaxed);
    Err(err)
}
