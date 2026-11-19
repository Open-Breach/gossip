//! Recording: every record of a session appended to a file, one JSON object
//! per line, as `--output json` prints them.
//!
//! The records come from the tap of the session, which hands them out
//! before the filter, so a recording holds what the filter hid as well. A
//! thread of its own writes them; the output never waits for the disk.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gossip_core::Record;
use tokio::sync::mpsc;

use crate::interactive::pause::with_thousands;
use crate::render::json_line;

/// How long quitting waits for the last records to reach the file.
const LAST_RECORDS: Duration = Duration::from_secs(3);

/// Where a recording says that it has failed: the disk is full, the file is
/// gone. Whoever prints the output reads the other end and says so.
pub struct Failures {
    pub tx: mpsc::UnboundedSender<String>,
    pub rx: mpsc::UnboundedReceiver<String>,
}

impl Failures {
    pub fn new() -> Failures {
        let (tx, rx) = mpsc::unbounded_channel();
        Failures { tx, rx }
    }
}

/// A recording that is running.
pub struct Recorder {
    path: PathBuf,
    written: Arc<Written>,
    /// Closed by the writing thread when it has written all it was given.
    finished: std::sync::mpsc::Receiver<()>,
}

#[derive(Default)]
struct Written {
    /// The records written since the recording started.
    records: AtomicU64,
    /// The size of the file: what was in it, and what was written since.
    bytes: AtomicU64,
}

impl Recorder {
    /// Starts appending to a file, which is made when it is not there.
    /// Gives the recorder and the tap to hand to the session: the recording
    /// runs for as long as the tap is held, and ends when it is dropped.
    pub fn start(
        path: &Path,
        failures: mpsc::UnboundedSender<String>,
    ) -> io::Result<(Recorder, mpsc::UnboundedSender<Record>)> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let written = Arc::new(Written::default());
        written.bytes.store(
            file.metadata().map_or(0, |file| file.len()),
            Ordering::Relaxed,
        );
        let (tap, records) = mpsc::unbounded_channel();
        let (done, finished) = std::sync::mpsc::channel();
        let thread = {
            let written = written.clone();
            let path = path.to_path_buf();
            move || {
                if let Err(error) = write(file, records, &written) {
                    let _ = failures.send(format!(
                        "the recording to {} has stopped: {error}",
                        path.display()
                    ));
                }
                drop(done);
            }
        };
        std::thread::Builder::new()
            .name("record".to_string())
            .spawn(thread)?;
        let recorder = Recorder {
            path: path.to_path_buf(),
            written,
            finished,
        };
        Ok((recorder, tap))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How many records were written since the recording started.
    pub fn records(&self) -> u64 {
        self.written.records.load(Ordering::Relaxed)
    }

    /// The size of the file, as a reader writes it: `2.1 MB`.
    pub fn size(&self) -> String {
        size_text(self.written.bytes.load(Ordering::Relaxed))
    }

    /// The recording in words: where it goes and how much it holds.
    pub fn describe(&self) -> String {
        format!(
            "recording to {}: {} since it started, {} in the file",
            self.path.display(),
            records_text(self.records()),
            self.size()
        )
    }

    /// Waits for the records that were handed over to reach the file, once
    /// the tap is gone: at the end of the program. It does not wait for
    /// ever; a session that still holds the tap is not waited for.
    pub fn finish(self) {
        // The thread closes its end when it is done; that, or the time.
        let _ = self.finished.recv_timeout(LAST_RECORDS);
    }
}

/// Writes the records as they come, until the tap is dropped. What is
/// written reaches the file whenever no record is waiting, so the file is
/// never far behind.
fn write(
    file: File,
    mut records: mpsc::UnboundedReceiver<Record>,
    written: &Written,
) -> io::Result<()> {
    let mut out = BufWriter::new(file);
    while let Some(record) = records.blocking_recv() {
        let mut next = Some(record);
        while let Some(record) = next {
            let line = json_line(&record).map_err(io::Error::other)?;
            out.write_all(line.as_bytes())?;
            out.write_all(b"\n")?;
            written.records.fetch_add(1, Ordering::Relaxed);
            written
                .bytes
                .fetch_add(line.len() as u64 + 1, Ordering::Relaxed);
            next = records.try_recv().ok();
        }
        out.flush()?;
    }
    out.flush()
}

/// `1 record`, `1,234 records`.
pub fn records_text(count: u64) -> String {
    match count {
        1 => "1 record".to_string(),
        count => format!("{} records", with_thousands(count)),
    }
}

/// A number of bytes as a reader writes it: `512 B`, `1.2 kB`, `2.1 MB`.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["kB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64 / 1000.0;
    let mut unit = 0;
    while size >= 999.95 && unit + 1 < UNITS.len() {
        size /= 1000.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use gossip_core::{Kind, Level};
    use serde_json::json;

    fn path(test: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("gossip-record-{}-{test}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn line(number: u64) -> Record {
        let time = Utc.with_ymd_and_hms(2026, 11, 18, 14, 0, 0).unwrap();
        let mut record = Record::notice("c", "fc-stg", "fc-api-0", None, time, number.to_string());
        record.kind = Kind::Line;
        record.container = Some("api".to_string());
        record.level = Some(Level::Info);
        record
            .fields
            .insert("tenant_id".to_string(), json!("t_123"));
        record
    }

    fn start(path: &Path) -> (Recorder, mpsc::UnboundedSender<Record>, Failures) {
        let failures = Failures::new();
        let (recorder, tap) = Recorder::start(path, failures.tx.clone()).unwrap();
        (recorder, tap, failures)
    }

    #[test]
    fn every_record_is_one_line_of_json_with_every_field() {
        let path = path("lines");
        let (recorder, tap, _failures) = start(&path);
        for number in 1..=3 {
            tap.send(line(number)).unwrap();
        }
        drop(tap);
        recorder.finish();

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(text.ends_with('\n'));
        // What `--output json` prints, line for line, and it reads back as
        // the record it was.
        for (index, written) in lines.iter().enumerate() {
            let record = line(index as u64 + 1);
            assert_eq!(*written, json_line(&record).unwrap());
            assert_eq!(serde_json::from_str::<Record>(written).unwrap(), record);
        }
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        for field in [
            "cluster",
            "namespace",
            "pod",
            "container",
            "time",
            "kind",
            "format",
            "level",
            "message",
            "fields",
            "raw",
            "previous",
            "truncated",
        ] {
            assert!(first.get(field).is_some(), "{field}");
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_recording_is_appended_to_what_the_file_holds() {
        let path = path("append");
        std::fs::write(&path, "{\"an\":\"older line\"}\n").unwrap();
        let (recorder, tap, _failures) = start(&path);
        // The size counts what was there; the records count what is new.
        assert_eq!(recorder.records(), 0);
        assert_eq!(recorder.size(), "20 B");
        tap.send(line(1)).unwrap();
        drop(tap);
        let described = {
            // Wait for the thread, then read the numbers.
            let _ = recorder.finished.recv_timeout(LAST_RECORDS);
            recorder.describe()
        };
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("{\"an\":\"older line\"}\n{\"cluster\":\"c\""));
        assert_eq!(text.lines().count(), 2);
        assert_eq!(
            described,
            format!(
                "recording to {}: 1 record since it started, {} in the file",
                path.display(),
                size_text(text.len() as u64)
            )
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn the_file_is_never_far_behind_while_the_recording_runs() {
        let path = path("live");
        let (recorder, tap, _failures) = start(&path);
        tap.send(line(1)).unwrap();
        // Without closing anything: the line is in the file in a moment.
        let mut waited = 0;
        while recorder.records() < 1 && waited < 200 {
            std::thread::sleep(Duration::from_millis(10));
            waited += 1;
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        assert_eq!(recorder.path(), path.as_path());
        drop(tap);
        recorder.finish();
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_file_that_cannot_be_opened_is_an_error_at_once() {
        let nowhere = std::env::temp_dir()
            .join("gossip-no-such-directory")
            .join("x.jsonl");
        let failures = Failures::new();
        assert!(Recorder::start(&nowhere, failures.tx).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_recording_that_cannot_be_written_says_so_once() {
        // A device that takes no byte.
        let (recorder, tap, mut failures) = start(Path::new("/dev/full"));
        tap.send(line(1)).unwrap();
        tap.send(line(2)).unwrap();
        let _ = recorder.finished.recv_timeout(LAST_RECORDS);
        let message = failures.rx.try_recv().expect("the failure is reported");
        assert!(
            message.starts_with("the recording to /dev/full has stopped: "),
            "{message}"
        );
        assert!(failures.rx.try_recv().is_err());
    }

    #[test]
    fn sizes_and_counts_as_a_reader_writes_them() {
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(999), "999 B");
        assert_eq!(size_text(1_000), "1.0 kB");
        assert_eq!(size_text(1_234), "1.2 kB");
        assert_eq!(size_text(999_949), "999.9 kB");
        assert_eq!(size_text(999_950), "1.0 MB");
        assert_eq!(size_text(2_100_000), "2.1 MB");
        assert_eq!(size_text(3_400_000_000), "3.4 GB");
        assert_eq!(size_text(5_000_000_000_000_000), "5000.0 TB");
        assert_eq!(records_text(0), "0 records");
        assert_eq!(records_text(1), "1 record");
        assert_eq!(records_text(12_345), "12,345 records");
    }
}
