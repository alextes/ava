use crate::config;

pub(crate) fn run_stop() {
    let Some(pid) = config::read_pid_file() else {
        eprintln!("ava is not running (no PID file)");
        std::process::exit(1);
    };

    if !config::check_process_alive(pid) {
        eprintln!("ava is not running (stale PID file, pid {pid})");
        config::remove_pid_file();
        std::process::exit(1);
    }

    // send SIGTERM
    #[cfg(unix)]
    {
        let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            eprintln!("failed to send SIGTERM to pid {pid}: {err}");
            std::process::exit(1);
        }
    }

    #[cfg(not(unix))]
    {
        eprintln!("ava stop is only supported on unix");
        std::process::exit(1);
    }

    // wait briefly for the process to exit
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if !config::check_process_alive(pid) {
            config::remove_pid_file();
            println!("ava stopped (pid {pid})");
            return;
        }
    }

    eprintln!("ava (pid {pid}) did not exit within 2 seconds");
    std::process::exit(1);
}
