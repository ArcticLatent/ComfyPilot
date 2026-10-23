//! The app's log file: everything logged also goes to
//! `<data dir>/<APP_ID>/logs/comfypilot.log`, so users launching the app
//! from a desktop shortcut (where stdout isn't visible) still have a log to
//! attach to bug reports.

use crate::app::APP_ID;
use directories::BaseDirs;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const LOG_FILE: &str = "comfypilot.log";
/// A log larger than this is kept as `<LOG_FILE>.1` at startup and a new one
/// is started.
const ROTATE_BYTES: u64 = 5 * 1024 * 1024;

/// The folder the log file lives in.
pub fn log_dir() -> Option<PathBuf> {
    BaseDirs::new().map(|base| base.data_local_dir().join(APP_ID).join("logs"))
}

/// The log file's path.
pub fn log_file_path() -> Option<PathBuf> {
    log_dir().map(|dir| dir.join(LOG_FILE))
}

/// A writer that copies log output to stdout and the log file.
pub struct LogTee {
    file: Option<File>,
}

impl Write for LogTee {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stdout().write_all(buf);
        if let Some(file) = self.file.as_mut() {
            let _ = file.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stdout().flush();
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush();
        }
        Ok(())
    }
}

/// Opens the log file for appending (rotating an oversized one first). Logs
/// still reach stdout when the file can't be opened.
pub fn log_writer() -> LogTee {
    LogTee {
        file: log_dir().and_then(|dir| open_log_file(&dir).ok()),
    }
}

fn open_log_file(dir: &Path) -> io::Result<File> {
    fs::create_dir_all(dir)?;
    let path = dir.join(LOG_FILE);
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > ROTATE_BYTES) {
        let _ = fs::rename(&path, dir.join(format!("{LOG_FILE}.1")));
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// The last `lines` lines of the log file, for diagnostics reports.
pub fn tail(lines: usize) -> String {
    let Some(text) = log_file_path().and_then(|path| fs::read_to_string(path).ok()) else {
        return String::new();
    };
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_logs_are_rotated_and_writes_append() {
        let dir = std::env::temp_dir().join(format!("arctic-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(LOG_FILE), vec![b'x'; (ROTATE_BYTES + 1) as usize]).unwrap();

        let mut tee = LogTee {
            file: Some(open_log_file(&dir).unwrap()),
        };
        tee.write_all(b"first line\n").unwrap();
        tee.flush().unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(LOG_FILE)).unwrap(),
            "first line\n"
        );
        assert_eq!(
            fs::metadata(dir.join(format!("{LOG_FILE}.1")))
                .unwrap()
                .len(),
            ROTATE_BYTES + 1
        );

        let mut tee = LogTee {
            file: Some(open_log_file(&dir).unwrap()),
        };
        tee.write_all(b"second line\n").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(LOG_FILE)).unwrap(),
            "first line\nsecond line\n"
        );
        let _ = fs::remove_dir_all(dir);
    }
}
