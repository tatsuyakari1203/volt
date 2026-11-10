//! Diagnostic logging for developers: a file logger with a hard size bound.
//!
//! Crates emit records through the `log` facade; this module installs the one
//! logger that writes them to `<app log dir>/volt.log`. The file is capped at
//! [`MAX_FILE_BYTES`] and rotated into `volt.log.1`, `volt.log.2`, ... keeping
//! at most [`MAX_FILES`] files in all, so the logs never take more than
//! `MAX_FILES * MAX_FILE_BYTES` (1.5 MiB) of disk, however long the app runs
//! or however much it logs.
//!
//! `tauri-plugin-log` was evaluated and not adopted. It can rotate by size, but
//! the total it keeps could not be tested here without a running app, it adds
//! a plugin registration to the builder and a dependency tree, and the bound
//! is the one property that matters. The writer below is small, bounded by
//! construction and tested against a temp directory.
//!
//! Redaction: callers follow the rule in `docs/architecture.md` (no value
//! data, arguments, command output or file contents in any record). This
//! module writes what it is given and adds nothing.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use log::{LevelFilter, Log, Metadata, Record};
use tauri::Manager;
use volt_core::{Clock, SystemClock};

/// Size at which the active log file is rotated.
pub(crate) const MAX_FILE_BYTES: u64 = 512 * 1024;

/// Most files kept, the active one included.
pub(crate) const MAX_FILES: usize = 3;

/// Name of the active log file.
const FILE_NAME: &str = "volt.log";

/// A log file that never grows past `max_file_bytes` and keeps at most
/// `max_files` files (the active one and `max_files - 1` rotated ones).
#[derive(Debug)]
pub(crate) struct RotatingFile {
    dir: PathBuf,
    max_file_bytes: u64,
    max_files: usize,
    file: File,
    size: u64,
}

impl RotatingFile {
    /// Opens `dir/volt.log` for appending, creating the directory. Files
    /// beyond the bound (left by a build with larger limits) are removed, and
    /// an active file that is already full is rotated first.
    pub(crate) fn open(dir: &Path, max_file_bytes: u64, max_files: usize) -> io::Result<Self> {
        let max_files = max_files.max(1);
        let max_file_bytes = max_file_bytes.max(1);
        fs::create_dir_all(dir)?;
        remove_files_beyond(dir, max_files);
        let path = dir.join(FILE_NAME);
        let size = fs::metadata(&path).map_or(0, |m| m.len());
        let mut this = Self {
            dir: dir.to_path_buf(),
            max_file_bytes,
            max_files,
            file: OpenOptions::new().create(true).append(true).open(&path)?,
            size,
        };
        if this.size >= this.max_file_bytes {
            this.rotate()?;
        }
        Ok(this)
    }

    /// Appends `line` (a newline is added), rotating first when it would not
    /// fit. A line longer than a whole file is cut to fit, so the bound holds
    /// for any input.
    pub(crate) fn write_line(&mut self, line: &str) -> io::Result<()> {
        let mut bytes = line.as_bytes();
        let limit = usize::try_from(self.max_file_bytes.saturating_sub(1)).unwrap_or(usize::MAX);
        if bytes.len() > limit {
            let mut end = limit;
            while end > 0 && !line.is_char_boundary(end) {
                end -= 1;
            }
            bytes = &bytes[..end];
        }
        let needed = bytes.len() as u64 + 1;
        if self.size + needed > self.max_file_bytes {
            self.rotate()?;
        }
        let mut buffer = Vec::with_capacity(bytes.len() + 1);
        buffer.extend_from_slice(bytes);
        buffer.push(b'\n');
        self.file.write_all(&buffer)?;
        self.size += needed;
        Ok(())
    }

    /// `volt.log` becomes `volt.log.1`, `.1` becomes `.2`, and so on; the one
    /// that would pass the limit is dropped. This only ever removes Volt's own
    /// rotated log files.
    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        let rotated = |n: usize| self.dir.join(format!("{FILE_NAME}.{n}"));
        let kept_rotated = self.max_files - 1;
        if kept_rotated == 0 {
            // Only the active file: start it again.
            self.file = File::create(self.dir.join(FILE_NAME))?;
            self.size = 0;
            return Ok(());
        }
        // Drop the oldest, then shift everything up by one.
        let _ = fs::remove_file(rotated(kept_rotated));
        for n in (1..kept_rotated).rev() {
            if rotated(n).exists() {
                fs::rename(rotated(n), rotated(n + 1))?;
            }
        }
        fs::rename(self.dir.join(FILE_NAME), rotated(1))?;
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(FILE_NAME))?;
        self.size = 0;
        Ok(())
    }
}

/// Removes `volt.log.N` files with `N >= max_files`: they are outside the bound.
fn remove_files_beyond(dir: &Path, max_files: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(number) = name
            .to_str()
            .and_then(|n| n.strip_prefix(FILE_NAME))
            .and_then(|rest| rest.strip_prefix('.'))
            .and_then(|n| n.parse::<usize>().ok())
        else {
            continue;
        };
        if number >= max_files {
            // Best effort: a file that cannot be removed only costs disk space.
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The `log` logger: one line per record in a [`RotatingFile`].
#[derive(Debug)]
struct FileLogger {
    file: Mutex<RotatingFile>,
    level: LevelFilter,
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format_line(
            &SystemClock.now().to_iso8601(),
            record.level(),
            record.target(),
            &record.args().to_string(),
        );
        if cfg!(debug_assertions) {
            eprintln!("{line}");
        }
        // A logger has nowhere to report its own failure; losing a line is
        // better than failing the operation being logged.
        let _ = self
            .file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write_line(&line);
    }

    fn flush(&self) {}
}

/// One record as one line: time, level, target, message (control characters
/// become spaces so a record never spans lines).
fn format_line(time: &str, level: log::Level, target: &str, message: &str) -> String {
    let message: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!("{time} {level:<5} {target} {message}")
}

/// Installs the file logger as the process's `log` logger, writing into `dir`.
fn install_in(dir: &Path, level: LevelFilter) -> Result<(), String> {
    let file = RotatingFile::open(dir, MAX_FILE_BYTES, MAX_FILES).map_err(|e| e.to_string())?;
    log::set_boxed_logger(Box::new(FileLogger {
        file: Mutex::new(file),
        level,
    }))
    .map_err(|e| e.to_string())?;
    log::set_max_level(level);
    Ok(())
}

/// Installs the file logger in the app's log directory. Failing to log must
/// not stop the app, so a failure only goes to stderr.
pub(crate) fn install(app: &tauri::AppHandle) {
    let level = if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    let result = app
        .path()
        .app_log_dir()
        .map_err(|e| e.to_string())
        .and_then(|dir| install_in(&dir, level));
    match result {
        Ok(()) => log::info!("Volt {} started", env!("CARGO_PKG_VERSION")),
        Err(error) => eprintln!("Volt could not start file logging: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(dir: &Path) -> Vec<(String, u64)> {
        let mut found: Vec<_> = fs::read_dir(dir)
            .expect("read dir")
            .map(|e| {
                let e = e.expect("entry");
                (
                    e.file_name().to_string_lossy().into_owned(),
                    e.metadata().expect("metadata").len(),
                )
            })
            .collect();
        found.sort();
        found
    }

    fn read(dir: &Path, name: &str) -> String {
        fs::read_to_string(dir.join(name)).expect("read log file")
    }

    #[test]
    fn total_size_and_file_count_stay_bounded_however_much_is_written() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut log = RotatingFile::open(dir.path(), 100, 3).expect("open");
        for n in 0..500 {
            log.write_line(&format!("line number {n:04} with some padding"))
                .expect("write");
            let found = files(dir.path());
            assert!(found.len() <= 3, "at most 3 files, found {found:?}");
            assert!(
                found.iter().all(|(_, size)| *size <= 100),
                "no file over 100 bytes: {found:?}"
            );
        }
        let total: u64 = files(dir.path()).iter().map(|(_, size)| size).sum();
        assert!(total <= 300, "total {total}");
        let names: Vec<_> = files(dir.path()).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["volt.log", "volt.log.1", "volt.log.2"]);
    }

    #[test]
    fn rotation_keeps_the_newest_lines_and_drops_the_oldest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut log = RotatingFile::open(dir.path(), 40, 3).expect("open");
        // Each line is 10 bytes with its newline: four fit in a file.
        for n in 0..20 {
            log.write_line(&format!("line {n:04}")).expect("write");
        }
        let active = read(dir.path(), "volt.log");
        let first_old = read(dir.path(), "volt.log.1");
        let second_old = read(dir.path(), "volt.log.2");
        assert!(
            active.contains("line 0019"),
            "newest line is in the active file"
        );
        assert!(
            first_old < active && second_old < first_old,
            "older content moves up"
        );
        let everything = format!("{second_old}{first_old}{active}");
        assert!(
            !everything.contains("line 0000"),
            "the oldest lines were dropped"
        );
        assert!(everything.contains("line 0019"));
        // What is kept is a contiguous tail: no gaps.
        let numbers: Vec<u32> = everything
            .lines()
            .map(|l| l.trim_start_matches("line ").parse().expect("number"))
            .collect();
        assert!(numbers.windows(2).all(|w| w[1] == w[0] + 1), "{numbers:?}");
        assert_eq!(*numbers.last().expect("lines"), 19);
    }

    #[test]
    fn a_line_longer_than_a_file_is_cut_so_the_bound_holds() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut log = RotatingFile::open(dir.path(), 50, 2).expect("open");
        // Multi-byte characters: the cut must fall on a character boundary.
        log.write_line(&"é".repeat(1000)).expect("write");
        log.write_line("after").expect("write");
        for (name, size) in files(dir.path()) {
            assert!(size <= 50, "{name} is {size} bytes");
            assert!(String::from_utf8(fs::read(dir.path().join(&name)).expect("read")).is_ok());
        }
        assert!(read(dir.path(), "volt.log").contains("after"));
    }

    #[test]
    fn reopening_continues_the_file_and_counts_what_is_already_in_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        {
            let mut log = RotatingFile::open(dir.path(), 100, 3).expect("open");
            log.write_line("first session").expect("write");
        }
        let mut log = RotatingFile::open(dir.path(), 100, 3).expect("reopen");
        log.write_line("second session").expect("write");
        assert_eq!(
            read(dir.path(), "volt.log"),
            "first session\nsecond session\n"
        );
        // A full file from an earlier run is rotated at the next start.
        for _ in 0..10 {
            log.write_line("fill").expect("write");
        }
        drop(log);
        let log = RotatingFile::open(dir.path(), 5, 3).expect("reopen with a smaller limit");
        drop(log);
        assert!(files(dir.path()).iter().all(|(_, size)| *size <= 100));
        assert!(dir.path().join("volt.log.1").exists());
    }

    #[test]
    fn files_beyond_the_limit_are_removed_when_opening() {
        let dir = tempfile::tempdir().expect("temp dir");
        for name in ["volt.log.1", "volt.log.2", "volt.log.7", "other.txt"] {
            fs::write(dir.path().join(name), "old").expect("write");
        }
        drop(RotatingFile::open(dir.path(), 100, 3).expect("open"));
        let names: Vec<_> = files(dir.path()).into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            ["other.txt", "volt.log", "volt.log.1", "volt.log.2"],
            "volt.log.7 is beyond the bound; unrelated files are never touched"
        );
    }

    #[test]
    fn a_single_file_limit_restarts_the_file_instead_of_keeping_old_ones() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut log = RotatingFile::open(dir.path(), 30, 1).expect("open");
        for n in 0..10 {
            log.write_line(&format!("line {n:04}")).expect("write");
        }
        let names: Vec<_> = files(dir.path()).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["volt.log"]);
        assert!(read(dir.path(), "volt.log").contains("line 0009"));
    }

    #[test]
    fn the_logger_writes_one_line_per_record_and_honours_the_level() {
        let dir = tempfile::tempdir().expect("temp dir");
        let logger = FileLogger {
            file: Mutex::new(RotatingFile::open(dir.path(), 10_000, 3).expect("open")),
            level: LevelFilter::Info,
        };
        let emit = |level, message: &str| {
            logger.log(
                &Record::builder()
                    .level(level)
                    .target("volt_core::executor")
                    .args(format_args!("{message}"))
                    .build(),
            );
        };
        emit(log::Level::Info, "Applied registry value X\nsecond line");
        emit(log::Level::Debug, "too detailed");
        emit(log::Level::Error, "boom");

        let text = read(dir.path(), "volt.log");
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2, "debug is filtered out: {text}");
        assert!(
            lines[0].contains("INFO  volt_core::executor Applied registry value X second line")
        );
        assert!(lines[1].contains("ERROR volt_core::executor boom"));
        // The line starts with a UTC ISO 8601 time.
        assert!(lines[0].as_bytes()[10] == b'T' && lines[0].contains('Z'));
    }
}
