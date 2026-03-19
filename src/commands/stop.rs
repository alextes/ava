use crate::config;

/// stop the running daemon. returns Ok(()) if stopped or already not running (when
/// `allow_not_running` is true), Err otherwise.
pub(crate) fn stop_daemon(allow_not_running: bool) -> Result<(), String> {
    let Some(pid) = config::read_pid_file() else {
        if allow_not_running {
            return Ok(());
        }
        return Err("ava is not running (no PID file)".into());
    };

    if !config::check_process_alive(pid) {
        config::remove_pid_file();
        if allow_not_running {
            return Ok(());
        }
        return Err(format!("ava is not running (stale PID file, pid {pid})"));
    }

    // send SIGTERM
    #[cfg(unix)]
    {
        let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            return Err(format!("failed to send SIGTERM to pid {pid}: {err}"));
        }
    }

    #[cfg(not(unix))]
    {
        return Err("ava stop is only supported on unix".into());
    }

    // wait briefly for the process to exit
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if !config::check_process_alive(pid) {
            config::remove_pid_file();
            println!("ava stopped (pid {pid})");
            return Ok(());
        }
    }

    Err(format!("ava (pid {pid}) did not exit within 2 seconds"))
}

pub(crate) fn run_stop() {
    if let Err(e) = stop_daemon(false) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
