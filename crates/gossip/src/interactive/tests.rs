//! The interactive mode with a terminal in memory: keys go in, and what the
//! terminal shows is read back. The session behind it follows the stand-in
//! API server of the core.

use std::time::Duration;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use gossip_core::testing::{
    FakeCluster, deployments, owner, pod_object, pods, replica_sets, workload_object,
};
use gossip_core::{Columns, Filter, Level, Session, SessionOptions, Target};
use tokio::time::timeout;

use super::screen::testing::Tape;
use super::*;
use crate::filters::LineRules;
use crate::render::{TextOptions, Zone};

const NS: &str = "fc-stg";
const API_POD: &str = "fc-api-7f698fb95f-vxfgs";
const WEB_POD: &str = "fc-web-api-5c8b6d7f9-m4q8z";

/// A time `millis` after a fixed moment in the past.
fn at(millis: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 18, 14, 0, 0).unwrap() + TimeDelta::milliseconds(millis)
}

fn target(text: &str) -> Target {
    text.parse().unwrap()
}

/// The deployments `fc-api` and `fc-web-api`, one pod each.
fn seed(fake: &FakeCluster) {
    for (deployment, set, pod, containers) in [
        ("fc-api", "fc-api-7f698fb95f", API_POD, &["api"][..]),
        (
            "fc-web-api",
            "fc-web-api-5c8b6d7f9",
            WEB_POD,
            &["api", "envoy"],
        ),
    ] {
        let labels = [("app", deployment)];
        fake.apply(
            &deployments(NS),
            workload_object(NS, deployment, &labels, None),
        );
        fake.apply(
            &replica_sets(NS),
            workload_object(NS, set, &labels, Some(owner("Deployment", deployment))),
        );
        fake.apply(
            &pods(NS),
            pod_object(NS, pod, &labels, Some(owner("ReplicaSet", set)), containers),
        );
    }
}

/// Text output with nothing in front of the message: no column is shown,
/// so a line is its message and a notice is `-- ` and its message.
fn bare() -> TextOptions {
    TextOptions {
        colour: false,
        fields: FieldChoice::Default,
        long_names: false,
        emoji: false,
        zone: Zone::Utc,
        columns: "time=0,pod=0,container=0,level=0"
            .parse::<ColumnsSetting>()
            .unwrap()
            .resolve(),
    }
}

/// A session, the console in front of it, and the terminal behind that.
struct Run {
    fake: FakeCluster,
    session: Session,
    console: Console<Tape>,
    tape: Tape,
}

impl Run {
    /// Follows `fc-stg/fc-api` with a bare text output.
    async fn start() -> Run {
        Run::start_with(Printer::text_with(bare()), FilterState::default(), None).await
    }

    async fn start_with(printer: Printer, filter: FilterState, pause_buffer: Option<usize>) -> Run {
        let fake = FakeCluster::start().await;
        seed(&fake);
        let session = Session::start(
            &fake.cluster(),
            vec![target("fc-stg/fc-api")],
            filter.filter(),
            SessionOptions {
                hold: Duration::from_millis(20),
                events: false,
                ..SessionOptions::default()
            },
        );
        let tape = Tape::default();
        let setup = Setup {
            printer,
            filter,
            pause_buffer,
        };
        let console = Console::new(tape.clone(), || 80, setup, session.handle(), true);
        Run {
            fake,
            session,
            console,
            tape,
        }
    }

    /// Writes a line to the log of the followed pod.
    fn log(&self, millis: i64, line: &str) {
        self.fake.log(NS, API_POD, "api", at(millis), line);
    }

    /// Hands the next records of the session to the console.
    async fn pump(&mut self, count: usize) {
        for _ in 0..count {
            let record = match timeout(Duration::from_secs(10), self.session.next()).await {
                Ok(Some(record)) => record,
                Ok(None) => panic!("the session ended"),
                Err(_) => panic!(
                    "no record came in time; the screen shows {:?}",
                    self.lines()
                ),
            };
            self.console.record(record).unwrap();
        }
    }

    /// Hands over whatever the session still brings, and checks that it is
    /// no more than a moment's worth: nothing.
    async fn expect_quiet(&mut self) {
        if let Ok(record) = timeout(Duration::from_millis(400), self.session.next()).await {
            panic!(
                "expected nothing more, got {:?}",
                record.map(|record| record.message)
            );
        }
    }

    fn press(&mut self, key: Key) -> Flow {
        self.console.key(key).unwrap()
    }

    /// Types characters, one key each.
    fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            assert_eq!(self.press(Key::Char(character)), Flow::Continue);
        }
    }

    /// Opens the command line, types a command and enters it.
    fn command(&mut self, line: &str) -> Flow {
        self.type_text(":");
        self.type_text(line);
        self.press(Key::Enter)
    }

    /// What the terminal shows, line by line. The last one is the line of
    /// the cursor: the status line, or an empty line while lines flow.
    fn lines(&self) -> Vec<String> {
        self.tape.lines()
    }

    /// The line of the cursor.
    fn status(&self) -> String {
        self.tape.terminal().last_line()
    }
}

// ---- the stream untouched --------------------------------------------------

#[tokio::test]
async fn lines_are_printed_as_they_come_and_no_status_stays() {
    let mut run = Run::start().await;
    run.log(0, "INFO one");
    run.log(100, "WARN two");
    run.pump(2).await;
    assert_eq!(run.lines(), ["INFO one", "WARN two", ""]);
    // Nothing but the lines and their line ends was written: no status, no
    // escape code.
    assert_eq!(run.tape.text(), "INFO one\r\nWARN two\r\n");
}

// ---- the filter ------------------------------------------------------------

#[tokio::test]
async fn level_hides_the_info_lines_from_the_next_record_on_with_one_notice() {
    let mut run = Run::start().await;
    run.log(0, "INFO one");
    run.pump(1).await;

    // Acceptance 2.
    assert_eq!(run.command("level warn"), Flow::Continue);
    run.pump(1).await;
    run.log(1_000, "INFO two");
    run.log(1_100, "WARN three");
    run.pump(1).await;
    run.expect_quiet().await;
    assert_eq!(
        run.lines(),
        [
            "INFO one",
            "-- filter: (kind == event or level >= warn)",
            "WARN three",
            ""
        ]
    );

    // `:level off` brings them back.
    run.command("level off");
    run.pump(1).await;
    run.log(2_000, "INFO four");
    run.pump(1).await;
    assert_eq!(run.lines()[3..], ["-- filter off", "INFO four", ""]);
}

#[tokio::test]
async fn a_slash_is_grep_and_the_rules_join_with_and() {
    let mut run = Run::start().await;
    // `/payment` is `:grep payment`.
    run.press(Key::Char('/'));
    assert_eq!(run.status(), ":grep");
    run.type_text("payment");
    assert_eq!(run.status(), ":grep payment");
    run.press(Key::Enter);
    run.pump(1).await;
    assert_eq!(
        run.lines(),
        ["-- filter: (kind == event or message contains payment)", ""]
    );

    // Another command adds its rule to what is there.
    run.command("level warn");
    run.command("field tenant_id=t_123");
    run.pump(2).await;
    assert_eq!(
        run.lines()[2],
        "-- filter: (kind == event or (level >= warn and message contains payment and \
         f.tenant_id == t_123))"
    );
    for (millis, line) in [
        r#"{"level":"WARN","message":"payment expired","tenant_id":"t_123"}"#,
        r#"{"level":"WARN","message":"payment expired","tenant_id":"t_999"}"#,
        r#"{"level":"INFO","message":"payment done","tenant_id":"t_123"}"#,
        r#"{"level":"WARN","message":"cache miss","tenant_id":"t_123"}"#,
    ]
    .iter()
    .enumerate()
    {
        run.log(1_000 + millis as i64, line);
    }
    run.pump(1).await;
    run.expect_quiet().await;
    assert_eq!(run.lines()[3..], ["tenant_id=t_123  payment expired", ""]);

    // The same command again replaces its own rule; a key alone and a
    // command alone take theirs away.
    run.command("grep cache");
    run.command("field tenant_id");
    run.command("level off");
    run.pump(3).await;
    assert_eq!(
        run.lines()[4..],
        [
            "-- filter: (kind == event or (level >= warn and message contains cache and \
             f.tenant_id == t_123))",
            "-- filter: (kind == event or (level >= warn and message contains cache))",
            "-- filter: (kind == event or message contains cache)",
            ""
        ]
    );
    run.command("grep");
    run.pump(1).await;
    assert_eq!(run.lines()[7], "-- filter off");
}

#[tokio::test]
async fn a_command_adds_to_the_filter_of_the_flags_and_of_the_template() {
    // As after `gossip follow --template t --level warn`, with a template
    // that keeps one tenant.
    let mut rules = LineRules::default();
    rules.set_level(Some(Level::Warn));
    let filter = FilterState {
        base: Some(Filter::field_equals("tenant_id", "t_123").unwrap()),
        rules,
    };
    let mut run = Run::start_with(Printer::text_with(bare()), filter, None).await;

    // `:filter` prints the filter as a template would hold it.
    run.command("filter");
    assert_eq!(
        run.lines(),
        [
            "-- filter = { all = [",
            "  { field = \"f.tenant_id\", op = \"==\", value = \"t_123\" },",
            "  { any = [",
            "    { field = \"kind\", op = \"==\", value = \"event\" },",
            "    { field = \"level\", op = \">=\", value = \"warn\" },",
            "  ] },",
            "] }",
            ""
        ]
    );

    // A command replaces the rule of the flag and narrows the template's.
    run.command("level error");
    run.pump(1).await;
    assert_eq!(
        run.lines()[7],
        "-- filter: (f.tenant_id == t_123 and (kind == event or level >= error))"
    );
    // Taking the rule away leaves the filter of the template.
    run.command("level off");
    run.pump(1).await;
    assert_eq!(run.lines()[8], "-- filter: f.tenant_id == t_123");

    // `:filter off` removes every rule, the template's too.
    run.command("filter off");
    run.pump(1).await;
    assert_eq!(run.lines()[9], "-- filter off");
    run.command("filter");
    assert_eq!(run.lines()[10], "-- no filter");
    // A rule after that is the whole filter.
    run.command("level warn");
    run.pump(1).await;
    assert_eq!(
        run.lines()[11],
        "-- filter: (kind == event or level >= warn)"
    );
}

#[tokio::test]
async fn a_command_that_cannot_be_read_says_so_and_changes_nothing() {
    let mut run = Run::start().await;
    run.command("level warn");
    run.pump(1).await;

    for line in [
        "lvl error",
        "level loud",
        "match (unclosed",
        "field =t_123",
        "field tenant_id",
        "add a/b/c/d/e",
        "events maybe",
        "container a b",
        "columns node=4",
        "fields",
    ] {
        assert_eq!(run.command(line), Flow::Continue, "{line}");
    }
    let lines = run.lines();
    assert_eq!(lines.len(), 12, "{lines:?}");
    assert_eq!(
        lines[1],
        "-- `:lvl` is not a command; `?` lists the commands"
    );
    assert!(lines[2].contains("`loud` is not a level"), "{}", lines[2]);
    assert_eq!(
        lines[3],
        "-- `(unclosed` is not a valid regular expression: regex parse error: (unclosed ^ error: \
         unclosed group"
    );
    assert!(lines[4].contains("names no field"), "{}", lines[4]);
    assert_eq!(lines[5], "-- there is no rule on the field tenant_id");
    assert!(lines[6].contains("levels"), "{}", lines[6]);
    assert!(lines[7].contains("`on` or `off`"), "{}", lines[7]);
    assert!(lines[9].contains("`node` is not a column"), "{}", lines[9]);

    // The session heard none of it: the filter is what it was, and nothing
    // more comes from it.
    run.expect_quiet().await;
    assert_eq!(
        run.console.filter.filter(),
        Some(Filter::Any(vec![
            Filter::kind_is(gossip_core::Kind::Event),
            Filter::level_at_least(Level::Warn),
        ]))
    );
    assert_eq!(run.session.handle().spec().targets.len(), 1);
}

// ---- targets, events, containers ---------------------------------------------

#[tokio::test]
async fn add_drop_and_targets_change_and_show_what_is_followed() {
    let mut run = Run::start().await;
    run.log(0, "api one");
    run.fake.log(NS, WEB_POD, "api", at(100), "web one");
    run.pump(1).await;

    // Acceptance 3: the second deployment comes in without a restart.
    run.command("add fc-stg/fc-web-api");
    run.pump(2).await;
    let mut lines = run.lines();
    lines.sort();
    assert_eq!(
        lines,
        [
            "",
            "-- added fc-stg/fc-web-api; following fc-stg/fc-api (1 pod), fc-stg/fc-web-api (1 pod)",
            "api one",
            "web one"
        ]
    );
    run.command("targets");
    assert_eq!(
        run.lines()[3..],
        ["-- fc-stg/fc-api: 1 pod", "-- fc-stg/fc-web-api: 1 pod", ""]
    );

    // Dropping the first stops it.
    run.command("drop fc-stg/fc-api");
    run.pump(2).await;
    assert_eq!(
        run.lines()[5..],
        [
            format!("-- pod {API_POD} is no longer selected by a target"),
            "-- dropped fc-stg/fc-api; following fc-stg/fc-web-api (1 pod)".to_string(),
            String::new()
        ]
    );
    run.log(1_000, "api two");
    run.fake.log(NS, WEB_POD, "api", at(1_100), "web two");
    run.pump(1).await;
    run.expect_quiet().await;
    assert_eq!(run.lines()[7], "web two");
    run.command("targets");
    assert_eq!(run.lines()[8], "-- fc-stg/fc-web-api: 1 pod");

    // Several targets on one line, and nothing left at the end.
    run.command("add fc-stg/fc-api fc-stg/fc-app");
    run.pump(4).await;
    run.command("drop fc-stg/fc-api fc-stg/fc-app fc-stg/fc-web-api");
    run.pump(5).await;
    run.command("targets");
    assert_eq!(run.status(), "");
    assert_eq!(
        run.lines().iter().rev().nth(1).map(String::as_str),
        Some("-- following nothing")
    );
}

#[tokio::test]
async fn events_and_container_reach_the_session() {
    let mut run = Run::start().await;
    run.command("events on");
    run.pump(1).await;
    run.command("container api*");
    run.pump(1).await;
    run.command("container off");
    run.pump(1).await;
    run.command("events off");
    run.pump(1).await;
    assert_eq!(
        run.lines(),
        [
            "-- events on",
            "-- containers matching api*; following fc-stg/fc-api (1 pod)",
            "-- every container; following fc-stg/fc-api (1 pod)",
            "-- events off",
            ""
        ]
    );
    let spec = run.session.handle().spec();
    assert_eq!((spec.events, spec.container), (Some(false), None));
}

// ---- pause, resume and clear -------------------------------------------------

#[tokio::test]
async fn space_stops_the_scrolling_and_space_again_prints_what_waited() {
    let mut run = Run::start().await;
    run.log(0, "one");
    run.pump(1).await;

    // Acceptance 4: the scrolling stops, and the status line counts.
    run.press(Key::Char(' '));
    assert_eq!(run.lines(), ["one", "paused · 0 lines waiting"]);
    run.log(1_000, "two");
    run.pump(1).await;
    assert_eq!(run.lines(), ["one", "paused · 1 line waiting"]);
    run.log(1_100, "three");
    run.log(1_200, "four");
    run.pump(2).await;
    assert_eq!(run.lines(), ["one", "paused · 3 lines waiting"]);

    // Again: the waiting lines in order, then the live ones, with no gap
    // and no repeat, and no status left behind.
    run.press(Key::Char(' '));
    assert_eq!(run.lines(), ["one", "two", "three", "four", ""]);
    run.log(2_000, "five");
    run.pump(1).await;
    assert_eq!(run.lines(), ["one", "two", "three", "four", "five", ""]);
    run.expect_quiet().await;
}

#[tokio::test]
async fn beyond_the_cap_the_oldest_lines_are_dropped_and_one_notice_says_how_many() {
    // Acceptance 4: `pause_buffer = 100` and 150 lines arriving.
    let mut run = Run::start_with(
        Printer::text_with(bare()),
        FilterState::default(),
        Some(100),
    )
    .await;
    run.log(0, "line 0");
    run.pump(1).await;
    run.press(Key::Char(' '));
    for number in 1..=150 {
        run.log(number * 10, &format!("line {number}"));
    }
    run.pump(150).await;
    assert_eq!(run.status(), "paused · 100 lines waiting · 50 dropped");

    run.press(Key::Char(' '));
    let lines = run.lines();
    // The first line, the notice, the hundred newest, and the empty line
    // of the cursor.
    assert_eq!(lines.len(), 103);
    assert_eq!(lines[1], "-- dropped 50 lines while paused");
    assert_eq!(lines[2], "line 51");
    assert_eq!(lines[101], "line 150");
    let expected: Vec<String> = (51..=150).map(|number| format!("line {number}")).collect();
    assert_eq!(lines[2..102], expected);
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("while paused"))
            .count(),
        1
    );
}

#[tokio::test]
async fn a_cap_of_zero_drops_nothing() {
    let mut run =
        Run::start_with(Printer::text_with(bare()), FilterState::default(), Some(0)).await;
    run.log(0, "line 0");
    run.pump(1).await;
    run.press(Key::Char(' '));
    for number in 1..=40 {
        run.log(number * 10, &format!("line {number}"));
    }
    run.pump(40).await;
    assert_eq!(run.status(), "paused · 40 lines waiting");
    run.press(Key::Char(' '));
    assert_eq!(run.lines().len(), 42);
}

#[tokio::test]
async fn one_can_pause_narrow_and_resume_to_see_only_what_matters() {
    let mut run = Run::start().await;
    run.log(0, "INFO one");
    run.pump(1).await;
    run.press(Key::Char(' '));
    run.log(1_000, "INFO two");
    run.log(1_100, "WARN three");
    run.log(1_200, "INFO four");
    run.log(1_300, "ERROR five");
    run.pump(4).await;
    assert_eq!(run.status(), "paused · 4 lines waiting");

    // Commands work while paused, and the new filter applies to what waits.
    run.command("level warn");
    assert_eq!(run.status(), "paused · 2 lines waiting");
    // The notice of the session waits with the lines.
    run.pump(1).await;
    assert_eq!(run.status(), "paused · 3 lines waiting");
    // What is asked of the console itself is answered at once.
    run.command("filter");
    assert!(
        run.lines()
            .iter()
            .any(|line| line.starts_with("-- filter = "))
    );
    assert_eq!(run.status(), "paused · 3 lines waiting");

    run.press(Key::Char(' '));
    let lines = run.lines();
    assert_eq!(
        lines[lines.len() - 4..],
        [
            "WARN three",
            "ERROR five",
            "-- filter: (kind == event or level >= warn)",
            ""
        ]
    );
}

#[tokio::test]
async fn ctrl_l_clears_the_screen_and_the_next_line_arrives_at_the_top() {
    let mut run = Run::start().await;
    run.log(0, "one");
    run.log(100, "two");
    run.pump(2).await;

    // Acceptance 5.
    run.press(Key::Ctrl('l'));
    assert_eq!(run.lines(), [""]);
    run.log(1_000, "three");
    run.pump(1).await;
    assert_eq!(run.lines(), ["three", ""]);

    // `:clear` is the same.
    run.command("clear");
    assert_eq!(run.lines(), [""]);
}

#[tokio::test]
async fn clearing_while_paused_also_empties_what_waits() {
    let mut run =
        Run::start_with(Printer::text_with(bare()), FilterState::default(), Some(2)).await;
    run.log(0, "one");
    run.pump(1).await;
    run.press(Key::Char(' '));
    for (index, line) in ["two", "three", "four"].iter().enumerate() {
        run.log(1_000 + index as i64 * 100, line);
    }
    run.pump(3).await;
    assert_eq!(run.status(), "paused · 2 lines waiting · 1 dropped");

    run.press(Key::Ctrl('l'));
    assert_eq!(run.lines(), ["paused · 0 lines waiting"]);
    // Resuming has nothing to print, and nothing to say about dropped lines.
    run.press(Key::Char(' '));
    assert_eq!(run.lines(), [""]);
    run.log(2_000, "five");
    run.pump(1).await;
    assert_eq!(run.lines(), ["five", ""]);
}

// ---- the command line ---------------------------------------------------------

#[tokio::test]
async fn typing_a_command_does_not_pause_the_stream() {
    let mut run = Run::start().await;
    run.log(0, "one");
    run.pump(1).await;
    run.type_text(":level wa");
    assert_eq!(run.lines(), ["one", ":level wa"]);
    assert_eq!(run.tape.terminal().cursor_column(), 9);

    // A line arrives above the command line, which stays as it was typed,
    // cursor and all.
    run.log(1_000, "two");
    run.pump(1).await;
    assert_eq!(run.lines(), ["one", "two", ":level wa"]);
    run.press(Key::Left);
    run.press(Key::Left);
    run.log(1_100, "three");
    run.pump(1).await;
    assert_eq!(run.lines(), ["one", "two", "three", ":level wa"]);
    assert_eq!(run.tape.terminal().cursor_column(), 7);

    // `Esc` closes it without applying, and the status does not stay.
    run.press(Key::Esc);
    assert_eq!(run.lines(), ["one", "two", "three", ""]);
    run.expect_quiet().await;
    assert_eq!(run.console.filter.filter(), None);
}

#[tokio::test]
async fn the_command_line_has_a_history_and_can_be_edited() {
    let mut run = Run::start().await;
    run.command("level warn");
    run.command("grep payment");
    run.pump(2).await;

    run.type_text(":");
    run.press(Key::Up);
    assert_eq!(run.status(), ":grep payment");
    run.press(Key::Up);
    assert_eq!(run.status(), ":level warn");
    // Change the level and enter it as a new command.
    run.press(Key::Ctrl('w'));
    run.type_text("erro");
    run.press(Key::Backspace);
    run.type_text("or");
    run.press(Key::Home);
    run.press(Key::Right);
    assert_eq!(run.status(), ":level error");
    assert_eq!(run.tape.terminal().cursor_column(), 2);
    run.press(Key::End);
    run.press(Key::Enter);
    run.pump(1).await;
    assert_eq!(
        run.lines()[2],
        "-- filter: (kind == event or (level >= error and message contains payment))"
    );
    // `Ctrl-U` empties the line.
    run.type_text(":anything");
    run.press(Key::Ctrl('u'));
    assert_eq!(run.status(), ":");
}

#[tokio::test]
async fn the_command_line_takes_the_place_of_the_pause_status_and_gives_it_back() {
    let mut run = Run::start().await;
    run.press(Key::Char(' '));
    assert_eq!(run.status(), "paused · 0 lines waiting");
    run.type_text(":lev");
    assert_eq!(run.status(), ":lev");
    // Space is a character here, not the pause.
    run.type_text(" ");
    assert_eq!(run.console.keyboard.prompt().unwrap().0, "lev ");
    run.press(Key::Esc);
    assert_eq!(run.status(), "paused · 0 lines waiting");
}

// ---- quit and help ---------------------------------------------------------------

#[tokio::test]
async fn q_ctrl_c_and_the_quit_command_quit() {
    let mut run = Run::start().await;
    assert_eq!(run.press(Key::Char('q')), Flow::Quit);
    assert_eq!(run.press(Key::Ctrl('c')), Flow::Quit);
    assert_eq!(run.command("q"), Flow::Quit);
    // `Ctrl-C` also while the command line is open; `q` is a letter there.
    run.type_text(":q");
    assert_eq!(run.status(), ":q");
    assert_eq!(run.press(Key::Ctrl('c')), Flow::Quit);

    // Leaving takes the status line away, so the shell starts on an empty
    // line.
    run.console.leave().unwrap();
    assert_eq!(run.lines(), [""]);
    assert_eq!(run.tape.terminal().cursor_column(), 0);
}

#[tokio::test]
async fn a_question_mark_prints_the_keys_and_the_commands_as_notices() {
    let mut run = Run::start().await;
    run.press(Key::Char('?'));
    let lines = run.lines();
    assert_eq!(lines.len(), command::HELP.len() + 1);
    assert_eq!(lines[0], "-- keys:");
    assert!(
        lines
            .iter()
            .take(command::HELP.len())
            .all(|line| line.starts_with("-- "))
    );
    assert!(lines.iter().any(|line| line.contains(":level <lvl>")));
    // `:help` is the same list.
    run.command("help");
    assert_eq!(run.lines().len(), 2 * command::HELP.len() + 1);
}

#[tokio::test]
async fn any_other_key_is_ignored() {
    let mut run = Run::start().await;
    for key in [
        Key::Char('x'),
        Key::Enter,
        Key::Esc,
        Key::Up,
        Key::Backspace,
        Key::Ctrl('z'),
    ] {
        assert_eq!(run.press(key), Flow::Continue);
    }
    assert_eq!(run.tape.text(), "");
}

// ---- fields and columns -----------------------------------------------------------

#[tokio::test]
async fn fields_and_columns_change_how_the_next_line_looks() {
    let options = TextOptions {
        columns: Columns::default(),
        long_names: true,
        ..bare()
    };
    let mut run = Run::start_with(Printer::text_with(options), FilterState::default(), None).await;
    let line =
        r#"{"level":"INFO","message":"done","tenant_id":"t_1","request_id":"8b1c","rows":3}"#;
    run.log(0, line);
    run.pump(1).await;
    assert_eq!(
        run.lines()[0],
        "14:00:00.000 fc-api…vxfgs         api         INFO  tenant_id=t_1 request_id=8b1c rows=3  done"
    );

    // Acceptance 6, from the command line: the pod column whole, and gone.
    run.command("columns time=0,pod=40");
    assert_eq!(
        run.lines()[1],
        "-- columns: time=0:none,pod=40:middle,container=10:end,level=5:end"
    );
    run.command("fields rows,tenant_id");
    assert_eq!(run.lines()[2], "-- fields: rows, tenant_id");
    run.log(1_000, line);
    run.pump(1).await;
    assert_eq!(
        run.lines()[3],
        "fc-api-7f698fb95f-vxfgs                  api         INFO  rows=3 tenant_id=t_1  done"
    );

    run.command("columns pod=0");
    run.command("fields all");
    run.log(2_000, line);
    run.pump(1).await;
    assert_eq!(
        run.lines()[4..7],
        [
            "-- columns: time=0:none,pod=0:middle,container=10:end,level=5:end",
            "-- fields: all",
            "api         INFO  tenant_id=t_1 request_id=8b1c rows=3  done"
        ]
    );

    // Alone, `:columns` says what they are; `default` is what the run
    // started with, for the columns and for the fields.
    run.command("columns");
    assert_eq!(
        run.lines()[7],
        "-- columns: time=0:none,pod=0:middle,container=10:end,level=5:end"
    );
    run.command("columns default");
    run.command("fields default");
    run.log(3_000, line);
    run.pump(1).await;
    let lines = run.lines();
    // The time column is back, for the notices too.
    assert!(
        lines[8].ends_with(" -- columns: time=12:none,pod=20:middle,container=10:end,level=5:end"),
        "{}",
        lines[8]
    );
    assert!(
        lines[9].ends_with(" -- fields: the default"),
        "{}",
        lines[9]
    );
    assert_eq!(
        lines[10],
        "14:00:03.000 fc-api…vxfgs         api         INFO  tenant_id=t_1 request_id=8b1c rows=3  done"
    );
}

#[tokio::test]
async fn json_output_has_no_fields_or_columns_to_change_and_its_notices_are_json() {
    let mut run = Run::start_with(Printer::json(), FilterState::default(), None).await;
    run.command("fields tenant_id");
    run.command("columns pod=40");
    run.command("columns");
    let lines = run.lines();
    assert_eq!(lines.len(), 4);
    for line in &lines[..3] {
        let record: serde_json::Value = serde_json::from_str(line).expect("a JSON record");
        assert_eq!(record["kind"], "notice");
        assert!(
            record["message"]
                .as_str()
                .unwrap()
                .contains("those of the text output")
        );
    }
    // The rest works as in any output.
    run.command("level warn");
    run.pump(1).await;
    let record: serde_json::Value = serde_json::from_str(&run.lines()[3]).unwrap();
    assert_eq!(
        record["message"],
        "filter: (kind == event or level >= warn)"
    );
}

#[tokio::test]
async fn the_notices_of_the_raw_output_do_not_stagger_the_lines() {
    let mut run = Run::start_with(Printer::raw(), FilterState::default(), None).await;
    run.log(0, "  a raw line  ");
    run.pump(1).await;
    run.command("level warn");
    run.pump(1).await;
    run.command("nonsense");
    assert_eq!(
        run.lines(),
        [
            "  a raw line",
            "-- filter: (kind == event or level >= warn)",
            "-- `:nonsense` is not a command; `?` lists the commands",
            ""
        ]
    );
    // The line itself is byte for byte what the container wrote.
    assert!(run.tape.text().starts_with("  a raw line  \r\n"));
}

// ---- when it is on ---------------------------------------------------------------

#[test]
fn the_interactive_mode_is_on_only_with_a_terminal_on_both_sides() {
    use clap::Parser;

    use crate::cli::{Cli, Command};

    let args = |flags: &[&str]| {
        let cli = Cli::try_parse_from(["gossip", "follow", "fc-stg"].iter().chain(flags)).unwrap();
        match cli.command {
            Command::Follow(args) => *args,
            other => panic!("expected the follow command, got {other:?}"),
        }
    };
    // (flags, stdin is a terminal, stdout is a terminal) -> on
    for (flags, stdin, stdout, on) in [
        (&[][..], true, true, true),
        // Not a terminal on either side: as before.
        (&[], false, true, false),
        (&[], true, false, false),
        (&[], false, false, false),
        // `--no-interactive` turns it off.
        (&["--no-interactive"], true, true, false),
        // `--interactive` forces it on when only stdout is a terminal, and
        // never without one.
        (&["--interactive"], false, true, true),
        (&["--interactive"], true, true, true),
        (&["--interactive"], true, false, false),
        (&["--interactive"], false, false, false),
    ] {
        assert_eq!(
            wanted(&args(flags), stdin, stdout),
            on,
            "{flags:?} stdin {stdin} stdout {stdout}"
        );
    }
}
