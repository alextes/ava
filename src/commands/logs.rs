use std::io::{self, BufRead, Seek, SeekFrom};
use std::path::PathBuf;

use crate::config;

/// find the most recent log file in ~/.ava/.
/// tracing_appender::rolling::daily produces files like ava.log.2026-03-17.
fn find_latest_log() -> Option<PathBuf> {
    let dir = config::ava_home_dir();
    let mut logs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("ava.log"))
        })
        .collect();

    // sorted lexicographically, the dated files sort chronologically
    logs.sort();
    logs.pop()
}

/// read the last `n` lines from a file.
fn tail_lines(path: &PathBuf, n: usize) -> io::Result<Vec<String>> {
    let file = std::fs::File::open(path)?;
    let reader = io::BufReader::new(file);
    let all_lines: Vec<String> = reader.lines().collect::<Result<_, _>>()?;
    let start = all_lines.len().saturating_sub(n);
    Ok(all_lines[start..].to_vec())
}

pub(crate) fn run_logs(lines: usize, follow: bool) {
    let Some(log_path) = find_latest_log() else {
        eprintln!("no log files found in {}", config::ava_home_dir().display());
        std::process::exit(1);
    };

    // print the last N lines
    match tail_lines(&log_path, lines) {
        Ok(tail) => {
            for line in &tail {
                println!("{line}");
            }
        }
        Err(e) => {
            eprintln!("failed to read {}: {e}", log_path.display());
            std::process::exit(1);
        }
    }

    if !follow {
        return;
    }

    // follow mode: seek to end and poll for new data
    let mut file = match std::fs::File::open(&log_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("failed to open {}: {e}", log_path.display());
            std::process::exit(1);
        }
    };

    if let Err(e) = file.seek(SeekFrom::End(0)) {
        eprintln!("failed to seek: {e}");
        std::process::exit(1);
    }

    let mut reader = io::BufReader::new(file);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                // no new data, sleep briefly
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Ok(_) => {
                // strip trailing newline for consistent output
                print!("{line}");
            }
            Err(e) => {
                eprintln!("read error: {e}");
                std::process::exit(1);
            }
        }
    }
}
