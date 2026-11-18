//! The reorder buffer: what makes one stream, ordered by time, out of the
//! records of many sources.
//!
//! Records of different containers arrive a little out of order: clocks of
//! nodes differ by milliseconds, and one stream is read before another. The
//! buffer holds every record back for a short while and hands records out
//! in the order of their time. A record that arrives later than that is
//! handed out late; none is ever dropped.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::record::Record;

/// How long a record is held back by default.
pub const DEFAULT_HOLD: Duration = Duration::from_millis(250);

/// How many records the buffer holds at most by default. Beyond that the
/// oldest are handed out before their time, so that a burst of history
/// cannot grow the buffer without bound.
pub const DEFAULT_CAPACITY: usize = 10_000;

/// A record waiting in the buffer.
struct Held {
    time: DateTime<Utc>,
    /// The order of arrival, which breaks ties between equal times.
    sequence: u64,
    arrived: Instant,
    record: Record,
}

impl Held {
    fn key(&self) -> (DateTime<Utc>, u64) {
        (self.time, self.sequence)
    }
}

impl PartialEq for Held {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for Held {}

impl PartialOrd for Held {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Held {
    /// Reversed, so that the heap gives the oldest record first.
    fn cmp(&self, other: &Self) -> Ordering {
        other.key().cmp(&self.key())
    }
}

/// Holds records back for a short while and hands them out ordered by time.
///
/// The buffer has no clock of its own: the caller says what time it is, so
/// that the whole of it can be tested without waiting.
pub struct ReorderBuffer {
    hold: Duration,
    capacity: usize,
    /// The held records, oldest time first.
    heap: BinaryHeap<Held>,
    /// The held records in the order they arrived, to know whose hold runs
    /// out next.
    arrivals: VecDeque<(Instant, DateTime<Utc>, u64)>,
    /// Records handed out before their own hold ran out, which `arrivals`
    /// still lists.
    released_early: HashSet<u64>,
    next_sequence: u64,
}

impl ReorderBuffer {
    pub fn new(hold: Duration) -> Self {
        ReorderBuffer::with_capacity(hold, DEFAULT_CAPACITY)
    }

    pub fn with_capacity(hold: Duration, capacity: usize) -> Self {
        ReorderBuffer {
            hold,
            capacity,
            heap: BinaryHeap::new(),
            arrivals: VecDeque::new(),
            released_early: HashSet::new(),
            next_sequence: 0,
        }
    }

    /// Takes a record that arrived at `now`.
    pub fn push(&mut self, record: Record, now: Instant) {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.arrivals.push_back((now, record.time, sequence));
        self.heap.push(Held {
            time: record.time,
            sequence,
            arrived: now,
            record,
        });
    }

    /// The records that are due at `now`, ordered by time.
    ///
    /// When the hold of a record runs out, it is handed out together with
    /// every held record that is older than it, so that those come first.
    pub fn pop_ready(&mut self, now: Instant) -> Vec<Record> {
        // The newest record, by time, whose hold has run out.
        let mut due: Option<(DateTime<Utc>, u64)> = None;
        while let Some(&(arrived, time, sequence)) = self.arrivals.front() {
            if self.released_early.remove(&sequence) {
                self.arrivals.pop_front();
                continue;
            }
            if now.saturating_duration_since(arrived) < self.hold {
                break;
            }
            self.arrivals.pop_front();
            due = due.max(Some((time, sequence)));
        }

        let mut ready = Vec::new();
        while let Some(oldest) = self.heap.peek() {
            let is_due = due.is_some_and(|due| oldest.key() <= due);
            if !is_due && self.heap.len() <= self.capacity {
                break;
            }
            let held = self.heap.pop().expect("the heap was just peeked");
            if now.saturating_duration_since(held.arrived) < self.hold {
                self.released_early.insert(held.sequence);
            }
            ready.push(held.record);
        }
        if self.heap.is_empty() {
            self.arrivals.clear();
            self.released_early.clear();
        }
        ready
    }

    /// When the next record becomes due, if any is held.
    pub fn next_deadline(&mut self) -> Option<Instant> {
        while let Some(&(arrived, _, sequence)) = self.arrivals.front() {
            if self.released_early.remove(&sequence) {
                self.arrivals.pop_front();
            } else {
                return Some(arrived + self.hold);
            }
        }
        None
    }

    /// Everything that is still held, ordered by time.
    pub fn drain(&mut self) -> Vec<Record> {
        let mut rest = Vec::with_capacity(self.heap.len());
        while let Some(held) = self.heap.pop() {
            rest.push(held.record);
        }
        self.arrivals.clear();
        self.released_early.clear();
        rest
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeDelta, TimeZone};

    const HOLD: Duration = Duration::from_millis(250);

    /// A record whose time is `millis` after a fixed start, named after it.
    fn record(millis: i64) -> Record {
        let time = Utc.with_ymd_and_hms(2026, 11, 18, 14, 2, 11).unwrap()
            + TimeDelta::milliseconds(millis);
        Record::notice("c", "ns", "", None, time, millis.to_string())
    }

    fn named(millis: i64, name: &str) -> Record {
        Record {
            message: name.to_string(),
            ..record(millis)
        }
    }

    fn messages(records: Vec<Record>) -> Vec<String> {
        records.into_iter().map(|record| record.message).collect()
    }

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn a_record_is_held_for_the_hold_and_then_handed_out() {
        let start = Instant::now();
        let mut buffer = ReorderBuffer::new(HOLD);
        buffer.push(record(0), start);
        assert_eq!(buffer.next_deadline(), Some(start + HOLD));
        assert!(buffer.pop_ready(start).is_empty());
        assert!(buffer.pop_ready(start + ms(249)).is_empty());
        assert_eq!(messages(buffer.pop_ready(start + ms(250))), ["0"]);
        assert!(buffer.is_empty());
        assert_eq!(buffer.next_deadline(), None);
    }

    #[test]
    fn records_that_arrive_out_of_order_leave_in_the_order_of_their_time() {
        let start = Instant::now();
        let mut buffer = ReorderBuffer::new(HOLD);
        // Three sources, read one after the other within a few milliseconds.
        buffer.push(record(30), start);
        buffer.push(record(10), start + ms(5));
        buffer.push(record(20), start + ms(10));
        assert_eq!(buffer.len(), 3);
        assert_eq!(
            messages(buffer.pop_ready(start + ms(300))),
            ["10", "20", "30"]
        );
    }

    #[test]
    fn a_record_takes_the_older_ones_with_it_when_its_hold_runs_out() {
        let start = Instant::now();
        let mut buffer = ReorderBuffer::new(HOLD);
        buffer.push(record(50), start);
        // An older record arrives 200 ms later. Its own hold is far from
        // over, but it must come before the one that is due.
        buffer.push(record(40), start + ms(200));
        // A newer one waits for its own hold.
        buffer.push(record(60), start + ms(210));
        assert_eq!(messages(buffer.pop_ready(start + ms(250))), ["40", "50"]);
        assert_eq!(buffer.next_deadline(), Some(start + ms(460)));
        assert!(buffer.pop_ready(start + ms(459)).is_empty());
        assert_eq!(messages(buffer.pop_ready(start + ms(460))), ["60"]);
        assert!(buffer.is_empty());
    }

    #[test]
    fn a_record_that_arrives_too_late_is_handed_out_late_not_dropped() {
        let start = Instant::now();
        let mut buffer = ReorderBuffer::new(HOLD);
        buffer.push(record(1_000), start);
        assert_eq!(messages(buffer.pop_ready(start + ms(250))), ["1000"]);
        // Seconds late, after a reconnect say: older than what went out.
        buffer.push(record(100), start + ms(3_000));
        assert!(buffer.pop_ready(start + ms(3_100)).is_empty());
        assert_eq!(messages(buffer.pop_ready(start + ms(3_250))), ["100"]);
    }

    #[test]
    fn records_with_the_same_time_keep_the_order_they_arrived_in() {
        let start = Instant::now();
        let mut buffer = ReorderBuffer::new(HOLD);
        for name in ["first", "second", "third"] {
            buffer.push(named(0, name), start);
        }
        buffer.push(named(-1, "older"), start);
        assert_eq!(
            messages(buffer.pop_ready(start + HOLD)),
            ["older", "first", "second", "third"]
        );
    }

    #[test]
    fn the_deadline_is_that_of_the_record_that_arrived_first() {
        let start = Instant::now();
        let mut buffer = ReorderBuffer::new(HOLD);
        assert_eq!(buffer.next_deadline(), None);
        buffer.push(record(500), start + ms(10));
        buffer.push(record(100), start + ms(20));
        assert_eq!(buffer.next_deadline(), Some(start + ms(260)));
        // The first to arrive is due; the older one goes with it.
        assert_eq!(messages(buffer.pop_ready(start + ms(260))), ["100", "500"]);
        assert_eq!(buffer.next_deadline(), None);
    }

    #[test]
    fn a_record_that_left_early_does_not_wake_the_buffer_again() {
        let start = Instant::now();
        let mut buffer = ReorderBuffer::new(HOLD);
        buffer.push(record(50), start);
        buffer.push(record(40), start + ms(100));
        buffer.push(record(90), start + ms(200));
        // `40` leaves with `50`, 150 ms before its own hold is over.
        assert_eq!(messages(buffer.pop_ready(start + ms(250))), ["40", "50"]);
        // The next deadline is that of `90`, not the stale one of `40`.
        assert_eq!(buffer.next_deadline(), Some(start + ms(450)));
        // And the stale deadline does not push `90` out before its time.
        assert!(buffer.pop_ready(start + ms(350)).is_empty());
        assert_eq!(messages(buffer.pop_ready(start + ms(450))), ["90"]);
    }

    #[test]
    fn a_steady_stream_is_ordered_across_two_sources() {
        let start = Instant::now();
        let mut buffer = ReorderBuffer::new(HOLD);
        let mut out = Vec::new();
        // Source A is read on time; source B lags 100 ms behind with records
        // that are 5 ms older than A's.
        for step in 0..20u64 {
            let now = start + ms(step * 50);
            buffer.push(named(step as i64 * 50, &format!("a{step}")), now);
            if step >= 2 {
                let lagging = step - 2;
                buffer.push(named(lagging as i64 * 50 - 5, &format!("b{lagging}")), now);
            }
            out.extend(buffer.pop_ready(now));
        }
        out.extend(buffer.drain());
        let times: Vec<DateTime<Utc>> = out.iter().map(|record| record.time).collect();
        let mut sorted = times.clone();
        sorted.sort();
        assert_eq!(times, sorted, "{:?}", messages(out));
        assert_eq!(times.len(), 38);
    }

    #[test]
    fn beyond_its_capacity_the_buffer_hands_out_the_oldest_at_once() {
        let start = Instant::now();
        let mut buffer = ReorderBuffer::with_capacity(HOLD, 3);
        for millis in [50, 10, 40, 20, 30] {
            buffer.push(record(millis), start);
        }
        // Nothing is due yet, but five records are two too many.
        assert_eq!(messages(buffer.pop_ready(start)), ["10", "20"]);
        assert_eq!(buffer.len(), 3);
        assert_eq!(messages(buffer.pop_ready(start + HOLD)), ["30", "40", "50"]);
    }

    #[test]
    fn drain_hands_out_everything_in_order() {
        let start = Instant::now();
        let mut buffer = ReorderBuffer::new(HOLD);
        for millis in [3, 1, 2] {
            buffer.push(record(millis), start);
        }
        assert_eq!(messages(buffer.drain()), ["1", "2", "3"]);
        assert!(buffer.is_empty());
        assert_eq!(buffer.next_deadline(), None);
    }

    #[test]
    fn a_hold_of_zero_passes_records_straight_through() {
        let start = Instant::now();
        let mut buffer = ReorderBuffer::new(Duration::ZERO);
        buffer.push(record(2), start);
        buffer.push(record(1), start);
        assert_eq!(messages(buffer.pop_ready(start)), ["1", "2"]);
    }
}
