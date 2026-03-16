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
pub fn do_exec_restart() -> Result<std::convert::Infallible, std::io::Error> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe()?;
    tracing::info!(?exe, "exec'ing into new binary");

    let err = std::process::Command::new(&exe)
        .args(["start", "--foreground"])
        .exec();

    // exec() only returns on error
    RESTART_REQUESTED.store(false, Ordering::Relaxed);
    Err(err)
}
