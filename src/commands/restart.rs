use super::stop::stop_daemon;
use crate::db::Database;

/// stop the running daemon (if any) then re-exec the current binary with `start`.
pub(crate) fn run_restart(foreground: bool) {
    if let Err(e) = stop_daemon(true) {
        eprintln!("{e}");
        std::process::exit(1);
    }
    if let Err(e) =
        Database::open().and_then(|db| db.record_runtime_event("cli_restart", "ava restart"))
    {
        eprintln!("warning: failed to record restart event: {e}");
    }

    let exe = if let Ok(path) = std::env::var("AVA_EXEC_PATH") {
        std::path::PathBuf::from(path)
    } else {
        std::env::current_exe().unwrap_or_else(|e| {
            eprintln!("failed to determine current executable: {e}");
            std::process::exit(1);
        })
    };

    let mut cmd = std::process::Command::new(exe);
    cmd.arg("start");
    if foreground {
        cmd.arg("--foreground");
    }

    // inherit env so AVA_BROWSER_VISIBLE etc. carry through
    let err = cmd.exec_replace();
    eprintln!("failed to exec: {err}");
    std::process::exit(1);
}

/// extension trait to call exec (replace process) portably
trait CommandExec {
    fn exec_replace(&mut self) -> std::io::Error;
}

impl CommandExec for std::process::Command {
    #[cfg(unix)]
    fn exec_replace(&mut self) -> std::io::Error {
        use std::os::unix::process::CommandExt;
        self.exec()
    }

    #[cfg(not(unix))]
    fn exec_replace(&mut self) -> std::io::Error {
        // on non-unix, just spawn and exit
        match self.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(e) => e,
        }
    }
}
