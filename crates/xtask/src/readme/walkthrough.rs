//! The walkthrough of the README, played by the built binary against the
//! stand-in API server (SPEC-release.md 4.2, section 3 of the README).
//!
//! Nothing in the README's walkthrough is typed by hand: every command line
//! is the one that was run here, and every line under it is what the binary
//! printed on the terminal. The cluster is scripted one thing at a time: a
//! line is logged, an event happens, a pod goes; then the script waits for
//! the binary to show it before the next thing happens. So the order of the
//! output is the order of the script, on every run, whatever the machine
//! is busy with.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, ensure};
use chrono::TimeDelta;

use super::stage::{API_POD_1, API_POD_2, API_POD_3, API_SET, Stage, WEB_POD, at, start};
use super::terminal::Terminal;

/// The lines of one block of the README: commands and what they printed.
pub type Lines = Vec<String>;

/// A request line of the `api`, as its JSON logger writes it.
fn request(method: &str, route: &str, status: u16) -> String {
    format!(
        r#"{{"level":"INFO","message":"request finished","http.method":"{method}","http.route":"{route}","http.status_code":{status}}}"#
    )
}

/// A `gossip follow` that runs on the terminal, and what of its output has
/// been taken for a block so far.
struct Follow {
    terminal: Terminal,
    /// The lines before this one have been matched by an `expect`.
    matched: usize,
    /// The lines before this one have been taken.
    taken: usize,
}

impl Follow {
    fn start(stage: &Stage, arguments: &[&str]) -> anyhow::Result<Follow> {
        Ok(Follow {
            terminal: stage.start(arguments)?,
            matched: 0,
            taken: 0,
        })
    }

    /// Waits for a line with this text, after the lines that were waited
    /// for before.
    async fn expect(&mut self, text: &str) -> anyhow::Result<()> {
        let from = self.matched;
        let display = self
            .terminal
            .wait(&format!("a new line with `{text}`"), |display| {
                display
                    .lines()
                    .iter()
                    .skip(from)
                    .any(|line| line.contains(text))
            })
            .await?;
        let found = display
            .lines()
            .iter()
            .skip(from)
            .position(|line| line.contains(text))
            .expect("the line was there a moment ago");
        self.matched = from + found + 1;
        Ok(())
    }

    /// The lines printed since the last time, once nothing more comes.
    async fn take(&mut self) -> anyhow::Result<Lines> {
        // Longer than the session holds a record back to put it in order.
        tokio::time::sleep(Duration::from_millis(400)).await;
        let display = self.terminal.display();
        let lines = display.lines()[self.taken..].to_vec();
        ensure!(
            display.lines().len() == self.matched,
            "the terminal shows lines the walkthrough did not wait for:\n{}",
            display.lines()[self.matched..].join("\n")
        );
        self.taken = display.lines().len();
        Ok(lines)
    }

    /// Types a command on the command line and gives it as the terminal
    /// shows it before `Enter`, then enters it.
    async fn command(&mut self, line: &str) -> anyhow::Result<String> {
        self.terminal.type_text(line)?;
        let shown = self
            .terminal
            .wait(&format!("`{line}` on the command line"), |display| {
                display.status() == line
            })
            .await?
            .status();
        self.terminal.type_text("\r")?;
        Ok(shown)
    }

    /// Presses a key and gives the status line once it reads this.
    async fn status_after(&mut self, key: &str, wanted: &str) -> anyhow::Result<String> {
        self.terminal.type_text(key)?;
        self.status(wanted).await
    }

    /// The status line, once it reads this.
    async fn status(&mut self, wanted: &str) -> anyhow::Result<String> {
        Ok(self.terminal.wait_for_status(wanted).await?.status())
    }

    /// Quits with `q` and waits for the program to end well.
    async fn quit(mut self) -> anyhow::Result<()> {
        self.terminal.type_text("q")?;
        let code = self.terminal.wait_for_exit().await?;
        ensure!(code == Some(0), "gossip ended with {code:?} after `q`");
        Ok(())
    }
}

/// Runs a command that ends by itself, and gives its command line and what
/// it printed.
async fn run(stage: &Stage, arguments: &[&str]) -> anyhow::Result<Lines> {
    let mut terminal = stage.start(arguments)?;
    let code = terminal.wait_for_exit().await?;
    let display = terminal.display();
    ensure!(
        code == Some(0),
        "`{}` ended with {code:?}:\n{}",
        Stage::typed(arguments),
        display.lines().join("\n")
    );
    let mut lines = vec![Stage::typed(arguments)];
    lines.extend_from_slice(display.lines());
    Ok(lines)
}

/// The sample at the top of the README: a line, a pod being killed, the
/// new pod starting.
pub async fn sample(binary: &Path) -> anyhow::Result<Lines> {
    let stage = Stage::set(binary).await?;
    stage.fill_namespaces();
    stage.deployment("api", API_SET, 1, 41);
    stage.pod(API_POD_1, "api", API_SET, start() - TimeDelta::hours(52));
    stage.log(API_POD_1, "api", at(11_482), &request("GET", "/cart", 200));

    stage.set_clock(at(12_000))?;
    let mut follow = Follow::start(&stage, &["follow", "shop/api"])?;
    follow.expect("/cart").await?;

    stage.set_clock(at(14_000))?;
    stage.event(
        "killing",
        API_POD_1,
        Some("api"),
        "Killing",
        "Stopping container api",
        at(14_000),
    );
    follow.expect("Killing").await?;

    stage.set_clock(at(15_000))?;
    stage.pod(API_POD_3, "api", API_SET, at(14_000));
    follow.expect("appeared").await?;
    stage.event(
        "started",
        API_POD_3,
        Some("api"),
        "Started",
        "Started container api",
        at(15_000),
    );
    follow.expect("Started").await?;
    stage.log(
        API_POD_3,
        "api",
        at(15_310),
        r#"{"level":"INFO","message":"listening","address":"0.0.0.0:8080"}"#,
    );
    follow.expect("listening").await?;

    let lines = follow.take().await?;
    follow.quit().await?;
    Ok(lines)
}

/// The nine steps of the walkthrough, in order, each with the name of its
/// block in the README.
pub async fn steps(binary: &Path) -> anyhow::Result<Vec<(&'static str, Lines)>> {
    let stage = Stage::set(binary).await?;
    stage.fill();
    stage.log(
        API_POD_1,
        "api",
        at(-9_000),
        &request("GET", "/products", 200),
    );
    stage.log(API_POD_2, "api", at(-7_500), &request("GET", "/cart", 200));
    stage.log(
        API_POD_1,
        "api",
        at(-4_200),
        r#"{"level":"DEBUG","message":"cache hit","key":"products:42"}"#,
    );
    let mut blocks = Vec::new();

    // 1. Find your way.
    let mut lines = run(&stage, &["list", "contexts"]).await?;
    for arguments in [["list", "namespaces"], ["list", "shop"]] {
        lines.push(String::new());
        lines.extend(run(&stage, &arguments).await?);
    }
    blocks.push(("walk-find", lines));

    // 2. Follow one deployment: its history, then a line as it is logged.
    let arguments = ["follow", "shop/api"];
    let mut follow = Follow::start(&stage, &arguments)?;
    follow.expect("cache hit").await?;
    stage.set_clock(at(3_000))?;
    stage.log(API_POD_1, "api", at(2_482), &request("POST", "/cart", 201));
    follow.expect("14:02:02.482").await?;
    let mut lines = vec![Stage::typed(&arguments)];
    lines.extend(follow.take().await?);
    blocks.push(("walk-follow", lines));

    // 3. What a restart looks like: the pod is told to stop, its
    // replacement is scheduled and started, and then the old one is gone.
    stage.set_clock(at(5_000))?;
    stage.event(
        "killing",
        API_POD_1,
        Some("api"),
        "Killing",
        "Stopping container api",
        at(5_000),
    );
    follow.expect("Killing").await?;
    stage.pending_pod(API_POD_3, "api", API_SET, at(5_000));
    follow.expect("appeared").await?;
    follow.expect("is waiting").await?;
    stage.event(
        "scheduled",
        API_POD_3,
        None,
        "Scheduled",
        &format!("Successfully assigned shop/{API_POD_3} to node-2"),
        at(5_000),
    );
    follow.expect("Scheduled").await?;
    stage.log(
        API_POD_1,
        "api",
        at(5_210),
        r#"{"level":"INFO","message":"shutting down"}"#,
    );
    follow.expect("shutting down").await?;
    stage.end_log(API_POD_1, "api");

    stage.set_clock(at(6_000))?;
    for (name, reason, note) in [
        (
            "pulled",
            "Pulled",
            "Container image \"registry.example/shop/api:1.8.2\" already present on machine",
        ),
        ("created", "Created", "Created container api"),
        ("started", "Started", "Started container api"),
    ] {
        stage.event(name, API_POD_3, Some("api"), reason, note, at(6_000));
        follow.expect(&format!("{reason}: ")).await?;
    }
    stage.pod(API_POD_3, "api", API_SET, at(5_000));
    stage.log(
        API_POD_3,
        "api",
        at(6_340),
        r#"{"level":"INFO","message":"listening","address":"0.0.0.0:8080"}"#,
    );
    follow.expect("listening").await?;
    stage.set_clock(at(8_000))?;
    stage.delete_pod(API_POD_1);
    follow.expect("went away").await?;
    blocks.push(("walk-restart", follow.take().await?));

    // 4. Narrow it: a level, then the filter as it stands.
    stage.set_clock(at(20_000))?;
    let mut lines = vec![follow.command(":level info").await?];
    follow.expect("-- filter: ").await?;
    stage.log(
        API_POD_2,
        "api",
        at(21_000),
        r#"{"level":"DEBUG","message":"cache miss","key":"cart:7"}"#,
    );
    stage.log(
        API_POD_2,
        "api",
        at(21_100),
        &request("GET", "/cart/7", 200),
    );
    follow.expect("/cart/7").await?;
    lines.extend(follow.take().await?);
    stage.set_clock(at(24_000))?;
    lines.push(follow.command(":filter").await?);
    follow.expect("-- filter = ").await?;
    // The filter is printed as the TOML of a template, over four lines.
    for part in ["\"kind\"", "\"level\"", "] }"] {
        follow.expect(part).await?;
    }
    lines.extend(follow.take().await?);
    blocks.push(("walk-narrow", lines));

    // 5. Stop to read: the status line counts what waits.
    let mut lines = vec![follow.status_after(" ", "paused").await?];
    stage.log(
        API_POD_2,
        "api",
        at(31_000),
        &request("GET", "/products", 200),
    );
    stage.log(
        API_POD_3,
        "api",
        at(31_200),
        &request("POST", "/checkout", 502),
    );
    stage.log(
        API_POD_3,
        "api",
        at(31_400),
        r#"{"level":"WARN","message":"payment provider slow","elapsed_ms":2140}"#,
    );
    lines.push(follow.status("3 lines waiting").await?);
    ensure!(
        follow.terminal.display().lines().len() == follow.taken,
        "a line was printed while the output was paused"
    );
    follow.terminal.type_text(" ")?;
    for line in ["14:02:31.000", "14:02:31.200", "14:02:31.400"] {
        follow.expect(line).await?;
    }
    lines.extend(follow.take().await?);
    blocks.push(("walk-pause", lines));

    // 6. Add a second service while the first is followed.
    stage.set_clock(at(40_000))?;
    let mut lines = vec![follow.command(":add shop/web").await?];
    follow.expect("added shop/web").await?;
    stage.log(
        WEB_POD,
        "web",
        at(41_150),
        r#"level=info msg="page rendered" path=/ status=200"#,
    );
    follow.expect("page rendered").await?;
    stage.log(
        WEB_POD,
        "web",
        at(41_900),
        r#"level=warn msg="slow upstream" upstream=api elapsed=2.1s"#,
    );
    follow.expect("slow upstream").await?;
    lines.extend(follow.take().await?);
    stage.set_clock(at(44_000))?;
    lines.push(follow.command(":targets").await?);
    follow.expect("shop/api: ").await?;
    follow.expect("shop/web: ").await?;
    lines.extend(follow.take().await?);
    blocks.push(("walk-add", lines));

    // 7. Keep it.
    stage.set_clock(at(50_000))?;
    let mut lines = vec![follow.command(":save shop").await?];
    follow.expect("saved as the template shop").await?;
    lines.extend(follow.take().await?);
    blocks.push(("walk-save", lines));

    // 8. Come back: quit, then start from what was followed last, and from
    // the template by its name.
    stage.set_clock(at(55_000))?;
    follow.quit().await?;
    let mut lines = Vec::new();
    for (index, arguments) in [&["follow", "--last"][..], &["follow", "--template", "shop"]]
        .into_iter()
        .enumerate()
    {
        stage.set_clock(at(60_000 + 10_000 * index as i64))?;
        let mut follow = Follow::start(&stage, arguments)?;
        // The newest line of the history is the last to be printed.
        follow.expect("slow upstream").await?;
        if index > 0 {
            lines.push(String::new());
        }
        lines.push(Stage::typed(arguments));
        lines.extend(follow.take().await?);
        follow.quit().await?;
    }
    blocks.push(("walk-back", lines));

    // 9. Keep a record: the screen shows what the filter lets through, the
    // file holds everything, and a replay shows it.
    stage.set_clock(at(90_000))?;
    let arguments = [
        "follow",
        "shop/api",
        "--level",
        "warn",
        "--record",
        "api.jsonl",
    ];
    let mut follow = Follow::start(&stage, &arguments)?;
    follow.expect("payment provider slow").await?;
    let mut lines = vec![Stage::typed(&arguments)];
    lines.extend(follow.take().await?);
    follow.quit().await?;
    let recording = stage.work().join("api.jsonl");
    let recorded = std::fs::read_to_string(&recording)
        .with_context(|| format!("no recording at {}", recording.display()))?;
    ensure!(
        recorded.lines().count() > lines.len() - 1,
        "the recording should hold more than the screen showed"
    );
    lines.push(String::new());
    lines.extend(run(&stage, &["replay", "api.jsonl"]).await?);
    blocks.push(("walk-record", lines));

    Ok(blocks)
}
