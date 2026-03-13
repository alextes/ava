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
        let _ = std::process::Command::new("kill")
            .args(["-USR1", &pid.to_string()])
            .status();
        println!("done — ava will restart after finishing current work");
    } else {
        println!("no running ava process found (no PID file), skipping signal");
    }

    #[cfg(not(unix))]
    println!("signal-based restart not supported on this platform — restart ava manually");

    Ok(())
}
