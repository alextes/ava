use crate::config;

/// check if ava is already running by reading the PID file and probing the process.
pub fn is_already_running() -> Option<u32> {
    let pid = config::read_pid_file()?;
    if config::check_process_alive(pid) {
        Some(pid)
    } else {
        None
    }
}

/// fork to background, creating a proper unix daemon.
///
/// - forks the process (parent exits with status 0)
/// - calls setsid() to detach from the controlling terminal
/// - redirects stdin/stdout/stderr to /dev/null
///
/// must be called **before** tokio runtime and tracing are initialized.
/// returns `Ok(())` in the child process after daemonizing.
#[cfg(unix)]
pub fn daemonize() -> Result<(), std::io::Error> {
    // SAFETY: we call fork() before any threads are spawned (no tokio runtime yet).
    // this is critical — fork() in a multi-threaded process is undefined behavior.
    let pid = unsafe { libc::fork() };

    if pid < 0 {
        return Err(std::io::Error::last_os_error());
    }

    if pid > 0 {
        // parent: print the child PID and exit
        println!("ava started (pid {pid})");
        std::process::exit(0);
    }

    // child: create a new session
    if unsafe { libc::setsid() } < 0 {
        return Err(std::io::Error::last_os_error());
    }

    // redirect stdin/stdout/stderr to /dev/null
    use std::os::unix::io::AsRawFd;
    let devnull = std::fs::File::open("/dev/null")?;
    let null_fd = devnull.as_raw_fd();

    unsafe {
        libc::dup2(null_fd, libc::STDIN_FILENO);
        libc::dup2(null_fd, libc::STDOUT_FILENO);
        libc::dup2(null_fd, libc::STDERR_FILENO);
    }

    Ok(())
}

#[cfg(not(unix))]
pub fn daemonize() -> Result<(), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "daemonize is only supported on unix",
    ))
}
