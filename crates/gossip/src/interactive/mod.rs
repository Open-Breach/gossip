//! The interactive mode of `gossip follow`: keys while the stream runs.
//!
//! The model is `less`. The stream scrolls as it always did, and a key
//! opens a one-line command line at the bottom of the terminal, pauses the
//! output, or clears the screen. It is on only with a terminal on both
//! sides; without one, none of this runs.
//!
//! The parts, each of which can be tested without a terminal:
//!
//! - [`keys`]: what a key does, in the stream and on the command line.
//! - [`prompt`]: the line being typed, and the lines typed before.
//! - [`command`]: what a line of the command line asks for.
//! - [`pause`]: the records that wait while the output is paused.
//! - [`screen`]: the one writer of the lines and of the status line.
//! - [`terminal`]: raw mode and the keys of the real terminal.
//!
//! The [`Console`] puts them together. It turns commands into calls on the
//! handle of the session; nothing in the core knows about any of this.

pub mod command;
pub mod keys;
pub mod pause;
pub mod prompt;
pub mod screen;
mod terminal;

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use chrono::Utc;
use gossip_core::save::LAST;
use gossip_core::{
    Columns, ColumnsSetting, Config, ConfigError, DEFAULT_PAUSE_BUFFER, Record, Session,
    SessionHandle, SessionSpec, is_template_name, template_toml,
};

use self::command::Command;
use self::keys::{Action, Key, Keyboard};
use self::pause::{PauseBuffer, lines, with_thousands};
use self::screen::{Screen, Status};
use crate::cli::FollowArgs;
use crate::fields::FieldChoice;
use crate::filters::{FilterState, LineRules};
use crate::follow::{Printer, Rendered};

/// Whether the interactive mode is on: with a terminal on both sides, or,
/// when `--interactive` forces it, with stdout a terminal alone. Without a
/// terminal for the output it is never on, and `--no-interactive` turns it
/// off wherever it would be.
pub fn wanted(args: &FollowArgs, stdin_is_terminal: bool, stdout_is_terminal: bool) -> bool {
    !args.no_interactive && stdout_is_terminal && (stdin_is_terminal || args.interactive)
}

/// What the interactive mode starts with.
pub struct Setup {
    pub printer: Printer,
    /// The filter the session was started with, in its two parts.
    pub filter: FilterState,
    /// How many records wait while the output is paused, as the config file
    /// says; zero is no limit.
    pub pause_buffer: Option<usize>,
    /// The template the session was started from, which `:save` alone
    /// writes back to.
    pub template: Option<String>,
    /// The config file, which `:save` writes and `:load` reads.
    pub config_path: PathBuf,
}

/// Whether to go on after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
}

/// Follows a session with the keyboard at hand, until it is quit.
pub async fn run(mut session: Session, setup: Setup) -> anyhow::Result<()> {
    // Raw mode is taken back when this is dropped: on every way out of
    // this function, and on a panic.
    let raw_mode = terminal::RawMode::enable()?;
    let mut keys = terminal::keys();
    let mut console = Console::new(
        io::stdout(),
        terminal::width,
        setup,
        session.handle(),
        io::stderr().is_terminal(),
    );

    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let terminate = terminated();
    tokio::pin!(terminate);
    let mut keys_open = true;
    let mut killed = false;
    loop {
        tokio::select! {
            // In raw mode `Ctrl-C` is a key; this is the signal from outside.
            _ = &mut interrupt => break,
            _ = &mut terminate => {
                killed = true;
                break;
            }
            record = session.next() => match record {
                Some(record) => console.record(record)?,
                None => break,
            },
            key = keys.recv(), if keys_open => match key {
                Some(key) => {
                    if console.key(key)? == Flow::Quit {
                        break;
                    }
                }
                // Nothing reads the keys any more; the stream goes on.
                None => keys_open = false,
            },
        }
    }
    // On every quit the session is kept as it is now, for `--last`.
    let last = console.last_session();
    let config_path = console.config_path.clone();
    console.leave()?;
    drop(raw_mode);
    if let Some(session) = last
        && let Err(error) = Config::save_template(&config_path, LAST, &session)
    {
        // Said once; quitting goes on.
        eprintln!("gossip: the session was not kept for `--last`: {error}");
    }
    if killed {
        // What the signal would have done, had it not been caught to leave
        // the terminal as it was found.
        std::process::exit(128 + 15);
    }
    Ok(())
}

/// Resolves when the process is asked to terminate (`kill -TERM`).
#[cfg(unix)]
async fn terminated() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut terminate) => {
            terminate.recv().await;
        }
        Err(_) => std::future::pending().await,
    }
}

#[cfg(not(unix))]
async fn terminated() {
    std::future::pending().await
}

/// The interactive mode without its terminal: records and keys come in, and
/// what the terminal shows goes out through the one writer.
pub struct Console<W: Write> {
    screen: Screen<W>,
    printer: Printer,
    keyboard: Keyboard,
    filter: FilterState,
    session: SessionHandle,
    /// The records that wait while the output is paused.
    waiting: PauseBuffer,
    paused: bool,
    /// The columns the run started with, for `:columns default`.
    columns_at_start: Option<Columns>,
    /// The cluster of the session, for the notices made here.
    cluster: String,
    /// Whether what goes beside the output, the notices of the raw output,
    /// goes through the screen: stderr is the same terminal.
    asides_on_screen: bool,
    /// The template the session is: the one it was started from, loaded
    /// from or saved as last. `:save` alone writes back to it.
    template: Option<String>,
    config_path: PathBuf,
    /// The template whose overwriting waits for a `y` or an `n`.
    overwriting: Option<String>,
}

impl<W: Write> Console<W> {
    pub fn new(
        out: W,
        width: impl Fn() -> usize + Send + 'static,
        mut setup: Setup,
        session: SessionHandle,
        asides_on_screen: bool,
    ) -> Console<W> {
        let columns_at_start = setup.printer.text().map(|renderer| renderer.columns());
        let cluster = session.spec().context.unwrap_or_default();
        Console {
            screen: Screen::new(out, width),
            printer: setup.printer,
            keyboard: Keyboard::default(),
            filter: setup.filter,
            session,
            waiting: PauseBuffer::new(setup.pause_buffer.unwrap_or(DEFAULT_PAUSE_BUFFER)),
            paused: false,
            columns_at_start,
            cluster,
            asides_on_screen,
            template: setup.template,
            config_path: setup.config_path,
            overwriting: None,
        }
    }

    /// Takes a record of the stream: prints it, or keeps it while the
    /// output is paused.
    pub fn record(&mut self, record: Record) -> anyhow::Result<()> {
        if self.paused {
            self.waiting.push(record);
            self.show_status()
        } else {
            self.print(std::slice::from_ref(&record))
        }
    }

    /// Takes a key.
    pub fn key(&mut self, key: Key) -> anyhow::Result<Flow> {
        match self.keyboard.press(key) {
            Action::Nothing => return Ok(Flow::Continue),
            Action::Quit => return Ok(Flow::Quit),
            Action::Pause => self.pause_or_resume()?,
            Action::Clear => self.clear()?,
            Action::Help => self.help()?,
            Action::Prompt => {}
            Action::Answer(yes) => self.answered(yes)?,
            Action::Run(line) => {
                // The command line is closed; its answer comes beneath
                // what is on the screen.
                self.show_status()?;
                if self.run(&line)? == Flow::Quit {
                    return Ok(Flow::Quit);
                }
            }
        }
        self.show_status()?;
        Ok(Flow::Continue)
    }

    /// Takes the status line away, as the last thing before the program
    /// ends.
    pub fn leave(&mut self) -> anyhow::Result<()> {
        Ok(self.screen.leave()?)
    }

    /// What the status line shows: the command line while it is open, the
    /// pause while the output is paused, and nothing while lines flow.
    fn status(&self) -> Option<Status> {
        if let Some(question) = self.keyboard.question() {
            return Some(Status::plain(question));
        }
        if let Some((text, before_cursor)) = self.keyboard.prompt() {
            return Some(Status {
                text: format!(":{text}"),
                before_cursor: Some(format!(":{before_cursor}")),
            });
        }
        self.paused.then(|| {
            let mut text = format!("paused · {} waiting", lines(self.waiting.len() as u64));
            if self.waiting.dropped() > 0 {
                let dropped = with_thousands(self.waiting.dropped());
                text.push_str(&format!(" · {dropped} dropped"));
            }
            Status::plain(text)
        })
    }

    fn show_status(&mut self) -> anyhow::Result<()> {
        let status = self.status();
        Ok(self.screen.set_status(status)?)
    }

    /// Prints records, in one go.
    fn print(&mut self, records: &[Record]) -> anyhow::Result<()> {
        let mut out = Vec::with_capacity(records.len());
        for record in records {
            match self.printer.render(record)? {
                Rendered::Out(line) => out.push(line),
                Rendered::Aside(line) if self.asides_on_screen => out.push(line),
                // Stderr leads somewhere else, where a line ends as lines do.
                Rendered::Aside(line) => eprintln!("{line}"),
            }
        }
        if !out.is_empty() {
            self.screen.lines(out.iter().map(String::as_str))?;
        }
        Ok(())
    }

    /// Says something from here, as a notice: the answer to a command that
    /// the core has no part in. It is printed at once, paused or not, since
    /// it is what was just asked for.
    fn say(&mut self, messages: &[String]) -> anyhow::Result<()> {
        let now = Utc::now();
        let notices: Vec<Record> = messages
            .iter()
            .map(|message| Record::notice(&self.cluster, "", "", None, now, message))
            .collect();
        self.print(&notices)
    }

    fn say_one(&mut self, message: impl Into<String>) -> anyhow::Result<()> {
        self.say(&[message.into()])
    }

    /// `Space`: pauses the output, not the session; again, prints what
    /// waited, in order, and goes on with what comes.
    fn pause_or_resume(&mut self) -> anyhow::Result<()> {
        if !self.paused {
            self.paused = true;
            return Ok(());
        }
        self.paused = false;
        let (records, dropped) = self.waiting.take();
        // The lines flow again, so the status goes first.
        self.show_status()?;
        if dropped > 0 {
            // Where the gap is: before the oldest line that was kept.
            self.say_one(format!("dropped {} while paused", lines(dropped)))?;
        }
        self.print(&records)
    }

    /// `Ctrl-L` and `:clear`. While paused this also empties what waits:
    /// clearing means "I have seen enough".
    fn clear(&mut self) -> anyhow::Result<()> {
        if self.paused {
            self.waiting.clear();
        }
        Ok(self.screen.clear()?)
    }

    fn help(&mut self) -> anyhow::Result<()> {
        let help: Vec<String> = command::HELP.iter().map(|line| line.to_string()).collect();
        self.say(&help)
    }

    /// Applies one line of the command line. What cannot be read is said,
    /// and changes nothing.
    fn run(&mut self, line: &str) -> anyhow::Result<Flow> {
        let command = match command::parse(line) {
            Ok(command) => command,
            Err(error) => {
                self.say_one(error)?;
                return Ok(Flow::Continue);
            }
        };
        match command {
            Command::Level(level) => {
                self.filter.rules.set_level(level);
                self.apply_filter();
            }
            Command::Grep(text) => {
                self.filter.rules.set_grep(text.as_deref());
                self.apply_filter();
            }
            Command::Match(regex) => match self.filter.rules.set_match(regex.as_deref()) {
                Ok(()) => self.apply_filter(),
                Err(error) => self.say_one(on_one_line(&format!("{error:#}")))?,
            },
            Command::Field {
                key,
                value: Some(value),
            } => match self.filter.rules.set_field(&key, &value) {
                Ok(()) => self.apply_filter(),
                Err(error) => self.say_one(on_one_line(&format!("{error:#}")))?,
            },
            Command::Field { key, value: None } => {
                if self.filter.rules.remove_field(&key) {
                    self.apply_filter();
                } else {
                    self.say_one(format!("there is no rule on the field {key}"))?;
                }
            }
            Command::ShowFilter => {
                let text = match self.filter.filter() {
                    Some(filter) => {
                        format!("filter = {}", gossip_core::filter_toml(&filter))
                    }
                    None => "no filter".to_string(),
                };
                self.say_one(text)?;
            }
            Command::FilterOff => {
                self.filter.clear();
                self.apply_filter();
            }
            // The session answers these with a notice of its own, when the
            // cluster has been asked.
            Command::Add(targets) => {
                for target in targets {
                    self.session.add_target(target);
                }
            }
            Command::Drop(targets) => {
                for target in targets {
                    self.session.remove_target(target);
                }
            }
            Command::Events(events) => self.session.set_events(events),
            Command::Container(container) => self.session.set_container(container),
            Command::Targets => {
                let mut targets: Vec<String> = self
                    .session
                    .targets()
                    .iter()
                    .map(|status| match status.pods {
                        1 => format!("{}: 1 pod", status.target),
                        pods => format!("{}: {pods} pods", status.target),
                    })
                    .collect();
                if targets.is_empty() {
                    targets.push("following nothing".to_string());
                }
                self.say(&targets)?;
            }
            Command::Fields(setting) => {
                let fields = FieldChoice::from_setting(setting.as_deref());
                let message = match &fields {
                    FieldChoice::Default => "fields: the default".to_string(),
                    FieldChoice::All => "fields: all".to_string(),
                    FieldChoice::Named(names) if names.is_empty() => "fields: none".to_string(),
                    FieldChoice::Named(names) => format!("fields: {}", names.join(", ")),
                };
                match self.printer.text() {
                    Some(renderer) => {
                        renderer.set_fields(fields);
                        self.say_one(message)?;
                    }
                    None => self.say_one(NOT_TEXT)?,
                }
            }
            Command::ShowColumns => self.set_columns(|columns, _| columns)?,
            Command::ColumnsDefault => self.set_columns(|_, at_start| at_start)?,
            Command::Columns(setting) => self.set_columns(|columns, _| setting.over(columns))?,
            Command::Save(None) => match self.template.clone() {
                Some(name) => self.save(&name)?,
                // Without a template to write back to, the name is asked
                // for: the command line opens again, to take it.
                None => self.keyboard.open_with("save "),
            },
            Command::Save(Some(name)) => self.save_as(name)?,
            Command::Load(name) => self.load(&name)?,
            Command::Templates => match self.config() {
                Ok(config) if config.templates.is_empty() => self.say_one(format!(
                    "the config file {} holds no templates",
                    self.config_path.display()
                ))?,
                Ok(config) => {
                    let table = crate::templates::list(&config).render();
                    let lines: Vec<String> = table.lines().map(str::to_string).collect();
                    self.say(&lines)?;
                }
                Err(error) => self.say_one(error.to_string())?,
            },
            Command::ShowTemplate(name) => {
                let text = self
                    .config()
                    .map_err(|error| error.to_string())
                    .and_then(|config| {
                        crate::templates::find(&config, &name)
                            .map(|template| template_toml(&name, template))
                            .map_err(|error| error.to_string())
                    });
                match text {
                    Ok(text) => self.say_one(text.trim_end())?,
                    Err(error) => self.say_one(error)?,
                }
            }
            Command::Clear => self.clear()?,
            Command::Quit => return Ok(Flow::Quit),
            Command::Help => self.help()?,
            #[cfg(debug_assertions)]
            Command::Panic => panic!("`:panic` was entered"),
        }
        Ok(Flow::Continue)
    }

    /// The config file as it is on disk right now; another terminal may
    /// have written to it since this one started.
    fn config(&self) -> Result<Config, ConfigError> {
        if !self.config_path.exists() {
            return Ok(Config::default());
        }
        Config::load_from(&self.config_path)
    }

    /// The session as it is now, as a template holds it: what the session
    /// follows, with the fields and the columns of the output.
    fn session_spec(&mut self) -> SessionSpec {
        let mut spec = self.session.spec();
        // The filter as it was last asked for; the session has the same,
        // unless the change is still on its way to it.
        spec.filter = self.filter.filter();
        spec.output = self.printer.output();
        // Only the columns that differ from those of the config file, so
        // that the rest keeps following it.
        let base = self
            .config()
            .map(|config| config.columns)
            .unwrap_or_default();
        if let Some(renderer) = self.printer.text() {
            spec.fields = renderer.fields_shown().to_setting();
            spec.columns = renderer.columns().differing_from(base.resolve());
        }
        spec
    }

    /// The session as it is when the program quits, to be kept for
    /// `--last`. None when nothing is followed: there is nothing to resume.
    pub fn last_session(&mut self) -> Option<SessionSpec> {
        let spec = self.session_spec();
        (!spec.targets.is_empty()).then_some(spec)
    }

    /// `:save <name>`: writes the session as a template of that name. One
    /// that is there is written over only after a `y`.
    fn save_as(&mut self, name: String) -> anyhow::Result<()> {
        if !is_template_name(&name) {
            return self.say_one(ConfigError::TemplateName(name).to_string());
        }
        if name == LAST {
            return self.say_one(
                "`last` is where gossip itself keeps the session when it quits; save under \
                 another name",
            );
        }
        match self.config() {
            Ok(config) if config.templates.contains_key(&name) => {
                self.keyboard
                    .ask(format!("overwrite the template {name}? y/n"));
                self.overwriting = Some(name);
                Ok(())
            }
            Ok(_) => self.save(&name),
            // A file that cannot be read is not written to.
            Err(error) => self.say_one(format!("not saved: {error}")),
        }
    }

    /// The answer to the question on the status line.
    fn answered(&mut self, yes: bool) -> anyhow::Result<()> {
        let Some(name) = self.overwriting.take() else {
            return Ok(());
        };
        if yes {
            self.save(&name)
        } else {
            self.say_one(format!("the template {name} was left as it is"))
        }
    }

    fn save(&mut self, name: &str) -> anyhow::Result<()> {
        let spec = self.session_spec();
        match Config::save_template(&self.config_path, name, &spec) {
            Ok(()) => {
                self.template = Some(name.to_string());
                self.say_one(format!(
                    "saved as the template {name} in {}",
                    self.config_path.display()
                ))
            }
            Err(error) => self.say_one(format!("not saved: {error}")),
        }
    }

    /// `:load <name>`: replaces the running session with a template. What
    /// the session follows changes through its handle; the filter, the
    /// fields and the columns of the template take the place of what the
    /// flags and the commands had made of them.
    fn load(&mut self, name: &str) -> anyhow::Result<()> {
        let config = match self.config() {
            Ok(config) => config,
            Err(error) => return self.say_one(error.to_string()),
        };
        let template = match crate::templates::find(&config, name) {
            Ok(template) => template.clone(),
            Err(error) => return self.say_one(error.to_string()),
        };
        if let Some(context) = &template.context
            && *context != self.cluster
        {
            return self.say_one(format!(
                "the template {name} is for the cluster {context}, and this session follows the \
                 cluster {}; nothing changed",
                self.cluster
            ));
        }
        // What the template leaves out comes from the config file, as with
        // `--template`; the cluster is the one of this session.
        let spec = SessionSpec {
            context: None,
            ..template.or(config.base())
        };
        if spec.targets.is_empty() {
            return self.say_one(format!("the template `{name}` has no targets"));
        }
        let settings = spec.clone().resolve();
        self.filter = FilterState {
            base: settings.filter.clone(),
            rules: LineRules::default(),
        };
        if self.paused {
            self.waiting.narrow(settings.filter.as_ref());
        }
        if let Some(renderer) = self.printer.text() {
            renderer.set_fields(FieldChoice::from_setting(settings.fields.as_deref()));
            renderer.set_columns(settings.columns);
            self.columns_at_start = Some(settings.columns);
        }
        self.template = (name != LAST).then(|| name.to_string());
        self.say_one(format!("loading the template {name}"))?;
        // The session answers when the cluster has been asked.
        self.session.load(spec);
        Ok(())
    }

    /// Hands the filter as it is now to the session, which answers with a
    /// notice, and applies it to what waits while the output is paused.
    fn apply_filter(&mut self) {
        let filter = self.filter.filter();
        if self.paused {
            self.waiting.narrow(filter.as_ref());
        }
        self.session.set_filter(filter);
    }

    /// Changes the columns of the text output, from the columns as they
    /// are and as they were at the start, and says what they are now.
    fn set_columns(
        &mut self,
        change: impl FnOnce(Columns, Columns) -> Columns,
    ) -> anyhow::Result<()> {
        let at_start = self.columns_at_start;
        let Some((renderer, at_start)) = self.printer.text().zip(at_start) else {
            return self.say_one(NOT_TEXT);
        };
        let columns = change(renderer.columns(), at_start);
        renderer.set_columns(columns);
        self.say_one(format!("columns: {}", ColumnsSetting::from(columns)))
    }
}

/// An error that spreads over several lines, as that of a regular
/// expression does, on one: a notice is one line.
fn on_one_line(text: &str) -> String {
    let parts: Vec<&str> = text.lines().map(str::trim).collect();
    parts.join(" ")
}

/// The answer to a command about the text output where there is none.
const NOT_TEXT: &str =
    "the fields and the columns are those of the text output; this run prints something else";

#[cfg(test)]
mod tests;
