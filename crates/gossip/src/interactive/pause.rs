//! The buffer of a paused output: the records that passed the filter while
//! nothing was printed.
//!
//! Pausing stops the output, not the session. The stream of the core is
//! still read, so nothing backs up into the log streams; what it brings
//! waits here, up to a cap. Beyond the cap the oldest records are dropped
//! and counted.

use std::collections::VecDeque;

use gossip_core::{Filter, Record};

/// The records that wait while the output is paused.
#[derive(Debug)]
pub struct PauseBuffer {
    records: VecDeque<Record>,
    /// How many records are held at most; zero is no limit.
    cap: usize,
    /// How many were dropped since the buffer was last emptied.
    dropped: u64,
}

impl PauseBuffer {
    /// A buffer that holds this many records at most; zero is no limit.
    pub fn new(cap: usize) -> PauseBuffer {
        PauseBuffer {
            records: VecDeque::new(),
            cap,
            dropped: 0,
        }
    }

    /// Takes a record. Beyond the cap the oldest one is dropped for it.
    pub fn push(&mut self, record: Record) {
        self.records.push_back(record);
        if self.cap > 0 && self.records.len() > self.cap {
            self.records.pop_front();
            self.dropped += 1;
        }
    }

    /// How many records wait.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// How many records were dropped because the buffer was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Applies a filter to the records that wait, so that one can pause,
    /// narrow, and resume to see only what matters. No filter keeps all.
    pub fn narrow(&mut self, filter: Option<&Filter>) {
        if let Some(filter) = filter {
            self.records.retain(|record| filter.accepts(record));
        }
    }

    /// Forgets everything: what waits, and what was dropped.
    pub fn clear(&mut self) {
        self.records.clear();
        self.dropped = 0;
    }

    /// Hands out the records that wait, in the order they came, and the
    /// number that was dropped, and is empty again.
    pub fn take(&mut self) -> (Vec<Record>, u64) {
        let dropped = std::mem::take(&mut self.dropped);
        (self.records.drain(..).collect(), dropped)
    }
}

/// A count as a reader writes it: `2,310`.
pub fn with_thousands(count: u64) -> String {
    let digits = count.to_string();
    let mut text = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// `1 line`, `142 lines`.
pub fn lines(count: u64) -> String {
    match count {
        1 => "1 line".to_string(),
        count => format!("{} lines", with_thousands(count)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gossip_core::{Kind, Level};

    fn line(number: u64, level: Level) -> Record {
        let mut record = Record::notice("c", "ns", "pod", None, Utc::now(), number.to_string());
        record.kind = Kind::Line;
        record.level = Some(level);
        record
    }

    fn numbers(records: &[Record]) -> Vec<u64> {
        records
            .iter()
            .map(|record| record.message.parse().unwrap())
            .collect()
    }

    #[test]
    fn what_is_pushed_comes_out_in_order_and_nothing_is_lost_up_to_the_cap() {
        let mut buffer = PauseBuffer::new(100);
        for number in 1..=100 {
            buffer.push(line(number, Level::Info));
        }
        assert_eq!(buffer.len(), 100);
        assert_eq!(buffer.dropped(), 0);
        let (records, dropped) = buffer.take();
        assert_eq!(numbers(&records), (1..=100).collect::<Vec<_>>());
        assert_eq!(dropped, 0);
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn beyond_the_cap_the_oldest_are_dropped_and_counted() {
        // Acceptance 4: a cap of 100 and 150 lines arriving.
        let mut buffer = PauseBuffer::new(100);
        for number in 1..=150 {
            buffer.push(line(number, Level::Info));
        }
        assert_eq!(buffer.len(), 100);
        assert_eq!(buffer.dropped(), 50);
        let (records, dropped) = buffer.take();
        assert_eq!(numbers(&records), (51..=150).collect::<Vec<_>>());
        assert_eq!(dropped, 50);

        // Taking starts the count again.
        buffer.push(line(151, Level::Info));
        assert_eq!(buffer.take().1, 0);
    }

    #[test]
    fn a_cap_of_zero_is_no_cap() {
        let mut buffer = PauseBuffer::new(0);
        for number in 1..=25_000 {
            buffer.push(line(number, Level::Info));
        }
        assert_eq!(buffer.len(), 25_000);
        assert_eq!(buffer.dropped(), 0);
    }

    #[test]
    fn a_cap_of_one_holds_the_newest() {
        let mut buffer = PauseBuffer::new(1);
        for number in 1..=3 {
            buffer.push(line(number, Level::Info));
        }
        let (records, dropped) = buffer.take();
        assert_eq!(numbers(&records), [3]);
        assert_eq!(dropped, 2);
    }

    #[test]
    fn a_new_filter_applies_to_what_waits() {
        let mut buffer = PauseBuffer::new(100);
        for number in 1..=6 {
            let level = if number % 2 == 0 {
                Level::Warn
            } else {
                Level::Info
            };
            buffer.push(line(number, level));
        }
        buffer.push(Record::notice("c", "", "", None, Utc::now(), "7"));

        // No filter keeps everything.
        buffer.narrow(None);
        assert_eq!(buffer.len(), 7);
        // A notice always passes.
        buffer.narrow(Some(&Filter::level_at_least(Level::Warn)));
        let (records, dropped) = buffer.take();
        assert_eq!(numbers(&records), [2, 4, 6, 7]);
        // What a filter takes out was not dropped for lack of room.
        assert_eq!(dropped, 0);
    }

    #[test]
    fn clearing_forgets_what_waits_and_what_was_dropped() {
        let mut buffer = PauseBuffer::new(2);
        for number in 1..=5 {
            buffer.push(line(number, Level::Info));
        }
        buffer.clear();
        assert_eq!((buffer.len(), buffer.dropped()), (0, 0));
        assert_eq!(buffer.take(), (Vec::new(), 0));
    }

    #[test]
    fn counts_as_a_reader_writes_them() {
        assert_eq!(with_thousands(0), "0");
        assert_eq!(with_thousands(142), "142");
        assert_eq!(with_thousands(2_310), "2,310");
        assert_eq!(with_thousands(10_000), "10,000");
        assert_eq!(with_thousands(1_234_567), "1,234,567");
        assert_eq!(lines(1), "1 line");
        assert_eq!(lines(0), "0 lines");
        assert_eq!(lines(2_310), "2,310 lines");
    }
}
