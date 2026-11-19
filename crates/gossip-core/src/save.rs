//! Writing a template into the config file.
//!
//! A session that is saved is a template. Writing one keeps everything else
//! in the file as it is, comments and order included, and is atomic: the
//! new text goes to a temporary file that then takes the place of the old
//! one. A file the core cannot read is never written over.

use std::io;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table};

use crate::columns::ColumnsSetting;
use crate::config::{Config, ConfigError, FormatSetting, SessionSpec};
use crate::duration::format_duration;
use crate::filter::Filter;

/// The name of the template that holds the session as it was when Gossip
/// last quit. It is written by Gossip itself and is no name to save under.
pub const LAST: &str = "last";

/// Whether a text can be the name of a template that is saved: the name is
/// the key of the template in the config file, so it is made of letters,
/// digits, `-` and `_`.
pub fn is_template_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// One template as it stands in the config file: text that can be pasted
/// back into it.
pub fn template_toml(name: &str, template: &SessionSpec) -> String {
    let mut text = format!("[templates.{}]\n", key(name));
    let mut line = |key: &str, value: String| {
        text.push_str(&format!("{key} = {value}\n"));
    };
    if let Some(context) = &template.context {
        line("context", quoted(context));
    }
    if !template.targets.is_empty() {
        let targets: Vec<String> = template
            .targets
            .iter()
            .map(|target| quoted(&target.to_string()))
            .collect();
        line("targets", format!("[{}]", targets.join(", ")));
    }
    if let Some(container) = &template.container {
        line("container", quoted(container.as_str()));
    }
    if let Some(since) = template.since {
        line("since", quoted(&format_duration(since)));
    }
    if let Some(tail) = template.tail {
        line("tail", tail.to_string());
    }
    if let Some(events) = template.events {
        line("events", events.to_string());
    }
    match &template.format {
        Some(FormatSetting::All(format)) => line("format", quoted(format.as_str())),
        Some(FormatSetting::PerContainer(formats)) => {
            let pairs: Vec<String> = formats
                .iter()
                .map(|(container, format)| {
                    format!("{} = {}", key(container), quoted(format.as_str()))
                })
                .collect();
            line("format", format!("{{ {} }}", pairs.join(", ")));
        }
        None => {}
    }
    if let Some(filter) = &template.filter {
        line("filter", filter_toml(filter));
    }
    if let Some(fields) = &template.fields {
        let fields: Vec<String> = fields.iter().map(|field| quoted(field)).collect();
        line("fields", format!("[{}]", fields.join(", ")));
    }
    if let Some(output) = template.output {
        line("output", quoted(output.as_str()));
    }
    if !template.columns.is_empty() {
        line("columns", columns_toml(&template.columns));
    }
    text
}

/// A filter as the config file writes it: a rule on one line, `all` and
/// `any` with one filter per line.
pub fn filter_toml(filter: &Filter) -> String {
    filter_text(filter, 0)
}

fn filter_text(filter: &Filter, depth: usize) -> String {
    let list = |key: &str, filters: &[Filter]| {
        if filters.is_empty() {
            return format!("{{ {key} = [] }}");
        }
        let mut text = format!("{{ {key} = [\n");
        for filter in filters {
            text.push_str(&"  ".repeat(depth + 1));
            text.push_str(&filter_text(filter, depth + 1));
            text.push_str(",\n");
        }
        text.push_str(&"  ".repeat(depth));
        text.push_str("] }");
        text
    };
    match filter {
        Filter::Rule(rule) => {
            let field = quoted(&rule.field().to_string());
            let op = quoted(rule.op().as_str());
            match rule.value() {
                Some(value) => format!(
                    "{{ field = {field}, op = {op}, value = {} }}",
                    quoted(value)
                ),
                None => format!("{{ field = {field}, op = {op} }}"),
            }
        }
        Filter::All(filters) => list("all", filters),
        Filter::Any(filters) => list("any", filters),
        Filter::Not(filter) => format!("{{ not = {} }}", filter_text(filter, depth)),
    }
}

/// A `columns` table as the config file writes it on one line.
fn columns_toml(columns: &ColumnsSetting) -> String {
    let entries: Vec<String> = columns
        .entries()
        .into_iter()
        .filter(|(_, setting)| !setting.is_empty())
        .map(|(name, setting)| {
            let mut parts = Vec::new();
            if let Some(width) = setting.width {
                parts.push(format!("width = {width}"));
            }
            if let Some(cut) = setting.cut {
                parts.push(format!("cut = {}", quoted(cut.as_str())));
            }
            format!("{name} = {{ {} }}", parts.join(", "))
        })
        .collect();
    format!("{{ {} }}", entries.join(", "))
}

/// A string as TOML writes it.
fn quoted(text: &str) -> String {
    toml::Value::String(text.to_string()).to_string()
}

/// A key as TOML writes it: bare when it can be, quoted otherwise.
fn key(name: &str) -> String {
    if is_template_name(name) {
        name.to_string()
    } else {
        quoted(name)
    }
}

impl Config {
    /// Writes one template into a config file: a new one at the end of the
    /// file, an existing one in its place. Everything else in the file
    /// stays as it is, comments included. The directory and the file are
    /// made when they are not there.
    ///
    /// The write is atomic. A file that cannot be read as a config is not
    /// touched, and the error says what is wrong with it.
    pub fn save_template(
        path: &Path,
        name: &str,
        template: &SessionSpec,
    ) -> Result<(), ConfigError> {
        // A config file that is a link, as in a tree of dotfiles, stays a
        // link: the file it leads to is the one that is written.
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let before = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(source) => return Err(ConfigError::Read { path, source }),
        };
        let after = with_template(&path, &before, name, template)?;
        write_atomically(&path, &after).map_err(|source| ConfigError::Write { path, source })
    }
}

/// The text of a config file with one template written into it.
fn with_template(
    path: &Path,
    text: &str,
    name: &str,
    template: &SessionSpec,
) -> Result<String, ConfigError> {
    if !is_template_name(name) {
        return Err(ConfigError::TemplateName(name.to_string()));
    }
    let parse_error = |source: toml::de::Error| ConfigError::Parse {
        path: path.to_path_buf(),
        source: Box::new(source),
    };
    // What the core cannot read, it does not write over.
    Config::from_toml(text).map_err(parse_error)?;
    let cannot = |why: &str| ConfigError::Unwritable {
        path: path.to_path_buf(),
        why: why.to_string(),
    };

    let mut document: DocumentMut = text
        .parse()
        .map_err(|_| cannot("it cannot be read in a way that keeps its comments"))?;
    let written: DocumentMut = template_toml(name, template)
        .parse()
        .map_err(|_| cannot("the template cannot be written as TOML"))?;
    let Some(Item::Table(mut new)) = written
        .get("templates")
        .and_then(|templates| templates.get(name))
        .cloned()
    else {
        return Err(cannot("the template cannot be written as TOML"));
    };

    let has_content = !document.as_table().is_empty();
    // A new table goes behind every table that is there.
    let end = last_position(document.as_table()).map_or(0, |last| last + 1);
    let templates = document.entry("templates").or_insert_with(|| {
        // No `[templates]` header of its own: the templates are tables.
        let mut templates = Table::new();
        templates.set_implicit(true);
        Item::Table(templates)
    });
    let Some(templates) = templates.as_table_mut() else {
        return Err(cannot(
            "its templates are written as one inline table, which cannot be added to",
        ));
    };
    match templates.get_mut(name) {
        Some(Item::Table(existing)) => replace_keys(existing, &new),
        Some(_) => {
            return Err(cannot(
                "the template is written as an inline table, which cannot be changed in place",
            ));
        }
        None => {
            if has_content {
                // An empty line between what is there and the new table.
                new.decor_mut().set_prefix("\n");
            }
            new.set_position(Some(end));
            templates.insert(name, Item::Table(new));
        }
    }

    let after = document.to_string();
    // The file must read back, and the template in it must be the one that
    // was to be written.
    let read_back = Config::from_toml(&after).map_err(parse_error)?;
    if read_back.templates.get(name) != Some(template) {
        return Err(cannot(
            "the template would not read back as it was to be written",
        ));
    }
    Ok(after)
}

/// Makes a table of the config file hold the keys of another: keys that
/// are gone are taken out, keys that stay keep their place and the comments
/// around them, and new keys go to the end.
fn replace_keys(existing: &mut Table, new: &Table) {
    let gone: Vec<String> = existing
        .iter()
        .map(|(key, _)| key.to_string())
        .filter(|key| !new.contains_key(key))
        .collect();
    for key in gone {
        existing.remove(&key);
    }
    for (key, item) in new.iter() {
        let mut item = item.clone();
        match existing.get_mut(key) {
            Some(old) => {
                // A comment behind the old value stays behind the new one,
                // and the key stays where it is, with the comment above it.
                if let (Some(old), Some(value)) = (old.as_value(), item.as_value_mut()) {
                    *value.decor_mut() = old.decor().clone();
                }
                *old = item;
            }
            None => {
                existing.insert(key, item);
            }
        }
    }
}

/// The place in the file of the table that comes last, among a table and
/// everything in it.
fn last_position(table: &Table) -> Option<isize> {
    let mut last = table.position();
    for (_, item) in table.iter() {
        let inner = match item {
            Item::Table(inner) => last_position(inner),
            Item::ArrayOfTables(tables) => tables.iter().filter_map(last_position).max(),
            _ => None,
        };
        last = last.max(inner);
    }
    last
}

/// Writes a file by way of a temporary file beside it that then takes its
/// place, so that the file is never half written.
fn write_atomically(path: &Path, text: &str) -> io::Result<()> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(directory) = directory {
        std::fs::create_dir_all(directory)?;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("gossip.toml");
    let temporary: PathBuf = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let write = || {
        std::fs::write(&temporary, text)?;
        // The new file has the permissions of the one it replaces.
        if let Ok(metadata) = std::fs::metadata(path) {
            std::fs::set_permissions(&temporary, metadata.permissions())?;
        }
        std::fs::rename(&temporary, path)
    };
    write().inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::record::Level;
    use crate::target::Target;

    /// The example of the spec, section 6, with a few more keys.
    const CONFIG: &str = r#"
[defaults]
context = "aks-vulcan-01"
tail = 20
events = true

[templates.dev-api]
context = "aks-vulcan-01"
targets = ["flowcpt/flowcpt-api", "flowcpt/flowcpt-web-api"]
container = "api"
filter = { all = [
  { field = "level", op = ">=", value = "info" },
  { any = [
    { field = "f.tenant_id", op = "==", value = "t_123" },
    { field = "kind", op = "==", value = "event" },
  ] },
] }
fields = ["tenant_id", "request_id"]

[templates.staging-all]
context = "aks-vulcan-01"
targets = ["flowcpt-stg/*"]
since = "5m"

[templates.everything]
targets = ["flowcpt/cronjob:nightly-sync", "monitoring/prometheus/*/exporter"]
container = "{api,envoy}"
since = "1h30m"
tail = 500
events = false
format = { api = "json", envoy = "text" }
filter = { not = { field = "f.debug", op = "exists" } }
fields = []
output = "json"
columns = { pod = { width = 24, cut = "middle" }, container = { width = 8 }, level = { cut = "none" } }

[templates."with a space"]
targets = ["flowcpt"]
format = "logfmt"
filter = { any = [] }
"#;

    fn config() -> Config {
        Config::from_toml(CONFIG).unwrap()
    }

    fn target(text: &str) -> Target {
        text.parse().unwrap()
    }

    fn path() -> PathBuf {
        PathBuf::from("/etc/gossip/gossip.toml")
    }

    /// A session as `:save` would hand it over.
    fn session() -> SessionSpec {
        SessionSpec {
            context: Some("aks-vulcan-01".to_string()),
            targets: vec![target("fc-stg/fc-api"), target("fc-stg/fc-web-api")],
            tail: Some(20),
            events: Some(true),
            filter: Some(Filter::Any(vec![
                Filter::kind_is(crate::record::Kind::Event),
                Filter::level_at_least(Level::Warn),
            ])),
            ..SessionSpec::default()
        }
    }

    fn saved(text: &str, name: &str, template: &SessionSpec) -> String {
        with_template(&path(), text, name, template).unwrap()
    }

    #[test]
    fn a_template_is_printed_as_the_config_file_writes_it() {
        let config = config();
        assert_eq!(
            template_toml("dev-api", &config.templates["dev-api"]),
            r#"[templates.dev-api]
context = "aks-vulcan-01"
targets = ["flowcpt/flowcpt-api", "flowcpt/flowcpt-web-api"]
container = "api"
filter = { all = [
  { field = "level", op = ">=", value = "info" },
  { any = [
    { field = "f.tenant_id", op = "==", value = "t_123" },
    { field = "kind", op = "==", value = "event" },
  ] },
] }
fields = ["tenant_id", "request_id"]
"#
        );
        assert_eq!(
            template_toml("everything", &config.templates["everything"]),
            r#"[templates.everything]
targets = ["flowcpt/cronjob:nightly-sync", "monitoring/prometheus/*/exporter"]
container = "{api,envoy}"
since = "1h30m"
tail = 500
events = false
format = { api = "json", envoy = "text" }
filter = { not = { field = "f.debug", op = "exists" } }
fields = []
output = "json"
columns = { pod = { width = 24, cut = "middle" }, container = { width = 8 }, level = { cut = "none" } }
"#
        );
    }

    #[test]
    fn what_is_printed_reads_back_as_the_same_template() {
        let config = config();
        for (name, template) in &config.templates {
            let text = template_toml(name, template);
            let read_back = Config::from_toml(&text)
                .unwrap_or_else(|error| panic!("`{name}` does not read back:\n{text}\n{error}"));
            assert_eq!(&read_back.templates[name], template, "{text}");
        }
    }

    #[test]
    fn strings_and_keys_as_toml_writes_them() {
        assert_eq!(quoted("api"), r#""api""#);
        assert_eq!(quoted(r#"say "hi""#), r#"'say "hi"'"#);
        assert_eq!(key("dev-api"), "dev-api");
        assert_eq!(key("with a space"), r#""with a space""#);
    }

    #[test]
    fn the_name_of_a_template_is_letters_digits_dash_and_underscore() {
        for name in ["dev-api", "staging_all", "a", "T2", "last", "2026-11-18"] {
            assert!(is_template_name(name), "{name}");
        }
        for name in [
            "",
            "with a space",
            "dev.api",
            "dev/api",
            "é",
            "a\"b",
            "dev api ",
        ] {
            assert!(!is_template_name(name), "{name}");
        }
        let error = with_template(&path(), "", "dev api", &session()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "`dev api` cannot be the name of a template: a name is made of letters, digits, `-` \
             and `_`"
        );
    }

    #[test]
    fn a_new_template_goes_to_the_end_and_everything_else_stays_as_it_is() {
        let before = r#"# My gossip config.
pause_buffer = 5000   # lines

[defaults]
# The staging cluster, most of the time.
context = "aks-vulcan-01"
tail = 20

[columns]
pod = { width = 24 }  # wide pods

# What I follow when the api misbehaves.
[templates.dev-api]
targets = ["flowcpt/flowcpt-api"]   # just the api
filter = { all = [
  { field = "level", op = ">=", value = "info" },   # no debug
] }

[templates.staging-all]
targets = ["flowcpt-stg/*"]
since = "5m"
"#;
        let after = saved(before, "tenant", &session());
        // Acceptance 7: the other templates and the comments are there, to
        // the byte, and the new template follows them.
        assert_eq!(
            after,
            format!(
                "{before}\n{}",
                r#"[templates.tenant]
context = "aks-vulcan-01"
targets = ["fc-stg/fc-api", "fc-stg/fc-web-api"]
tail = 20
events = true
filter = { any = [
  { field = "kind", op = "==", value = "event" },
  { field = "level", op = ">=", value = "warn" },
] }
"#
            )
        );
        let config = Config::from_toml(&after).unwrap();
        assert_eq!(config.templates["tenant"], session());
        assert_eq!(config.templates.len(), 3);
        assert_eq!(config.pause_buffer, Some(5000));
    }

    #[test]
    fn a_template_that_is_there_is_written_in_its_place() {
        let before = r#"[defaults]
tail = 20

# What I follow when the api misbehaves.
[templates.dev-api]
# The cluster.
context = "aks-vulcan-01"
targets = ["flowcpt/flowcpt-api"]   # just the api
container = "api"
since = "5m"

[templates.staging-all]
targets = ["flowcpt-stg/*"]  # everything

[columns]
pod = { width = 24 }
"#;
        let after = saved(before, "dev-api", &session());
        // The table stays where it was, between the others. The comment
        // above it stays, and so do the comments at the keys that stay.
        // Keys that are gone are taken out, and new ones are added.
        assert_eq!(
            after,
            r#"[defaults]
tail = 20

# What I follow when the api misbehaves.
[templates.dev-api]
# The cluster.
context = "aks-vulcan-01"
targets = ["fc-stg/fc-api", "fc-stg/fc-web-api"]   # just the api
tail = 20
events = true
filter = { any = [
  { field = "kind", op = "==", value = "event" },
  { field = "level", op = ">=", value = "warn" },
] }

[templates.staging-all]
targets = ["flowcpt-stg/*"]  # everything

[columns]
pod = { width = 24 }
"#
        );
        assert_eq!(
            Config::from_toml(&after).unwrap().templates["dev-api"],
            session()
        );
    }

    #[test]
    fn writing_the_same_template_again_changes_nothing() {
        let once = saved(CONFIG, LAST, &session());
        let twice = saved(&once, LAST, &session());
        assert_eq!(once, twice);
        // And one that differs changes only its own lines.
        let other = SessionSpec {
            targets: vec![target("fc-stg/fc-app")],
            ..session()
        };
        let changed = saved(&twice, LAST, &other);
        let differing: Vec<(&str, &str)> = twice
            .lines()
            .zip(changed.lines())
            .filter(|(before, after)| before != after)
            .collect();
        assert_eq!(
            differing,
            [(
                r#"targets = ["fc-stg/fc-api", "fc-stg/fc-web-api"]"#,
                r#"targets = ["fc-stg/fc-app"]"#
            )]
        );
    }

    #[test]
    fn an_empty_file_and_a_file_of_comments_take_a_template() {
        let template = template_toml("tenant", &session());
        assert_eq!(saved("", "tenant", &session()), template);
        let after = saved("# nothing yet\n", "tenant", &session());
        assert!(after.contains("# nothing yet"), "{after}");
        assert_eq!(
            Config::from_toml(&after).unwrap().templates["tenant"],
            session()
        );
    }

    #[test]
    fn every_key_a_template_holds_is_written_and_read_back() {
        let everything = SessionSpec {
            context: Some("aks-vulcan-01".to_string()),
            targets: vec![target("flowcpt/cronjob:nightly-sync")],
            container: Some("{api,envoy}".parse().unwrap()),
            since: Some(Duration::from_secs(5400)),
            tail: Some(500),
            events: Some(false),
            format: Some(FormatSetting::All(crate::parser::FormatChoice::Logfmt)),
            filter: Some(Filter::Not(Box::new(
                Filter::field_equals("debug", "true").unwrap(),
            ))),
            fields: Some(vec!["tenant_id".to_string(), "a b".to_string()]),
            output: Some(crate::config::Output::Json),
            columns: "pod=24:start,level=0".parse().unwrap(),
        };
        let after = saved(CONFIG, "everything-2", &everything);
        let config = Config::from_toml(&after).unwrap();
        assert_eq!(config.templates["everything-2"], everything);
        // The templates that were there are untouched.
        for (name, template) in &self::config().templates {
            assert_eq!(&config.templates[name], template, "{name}");
        }
        assert!(after.starts_with(CONFIG));
    }

    #[test]
    fn a_file_the_core_cannot_read_is_never_written_over() {
        // A typo in a key, which the core refuses.
        let broken = "[templates.dev-api]\ntragets = [\"flowcpt\"]\n";
        let error = with_template(&path(), broken, "tenant", &session()).unwrap_err();
        assert!(matches!(error, ConfigError::Parse { .. }), "{error}");
        let text = error.to_string();
        assert!(text.contains("/etc/gossip/gossip.toml"), "{text}");
        assert!(text.contains("line 2"), "{text}");
        assert!(text.contains("tragets"), "{text}");
        // And one that is no TOML at all.
        let error = with_template(&path(), "[templates", "tenant", &session()).unwrap_err();
        assert!(matches!(error, ConfigError::Parse { .. }), "{error}");
    }

    #[test]
    fn templates_written_as_inline_tables_are_not_changed() {
        let inline = "templates = { dev-api = { targets = [\"flowcpt\"] } }\n";
        let error = with_template(&path(), inline, "tenant", &session()).unwrap_err();
        assert!(matches!(error, ConfigError::Unwritable { .. }), "{error}");
        assert!(error.to_string().contains("inline table"), "{error}");

        let dotted = "[templates]\ndev-api = { targets = [\"flowcpt\"] }\n";
        let error = with_template(&path(), dotted, "dev-api", &session()).unwrap_err();
        assert!(matches!(error, ConfigError::Unwritable { .. }), "{error}");
        // Another template beside it can be added.
        let after = saved(dotted, "tenant", &session());
        assert_eq!(Config::from_toml(&after).unwrap().templates.len(), 2);
    }

    /// A directory of its own for one test.
    fn directory(test: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("gossip-save-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    #[test]
    fn the_file_and_its_directory_are_made_when_they_are_not_there() {
        let directory = directory("new");
        let path = directory.join("nested").join("gossip.toml");
        Config::save_template(&path, "tenant", &session()).unwrap();
        let config = Config::load_from(&path).unwrap();
        assert_eq!(config.templates["tenant"], session());
        // Nothing but the file is left in its directory.
        let left: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["gossip.toml"]);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_file_that_cannot_be_read_stays_as_it_is_on_disk() {
        let directory = directory("broken");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("gossip.toml");
        let broken = "[templates.dev-api]\ntragets = [\"flowcpt\"]\n";
        std::fs::write(&path, broken).unwrap();
        let error = Config::save_template(&path, "tenant", &session()).unwrap_err();
        assert!(error.to_string().contains("tragets"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_file_that_is_saved_twice_keeps_what_was_between() {
        let directory = directory("twice");
        let path = directory.join("gossip.toml");
        Config::save_template(&path, "one", &session()).unwrap();
        // Someone edits the file by hand in between.
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("\n# by hand\n[defaults]\ntail = 5\n");
        std::fs::write(&path, &text).unwrap();
        Config::save_template(&path, "two", &session()).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.starts_with(&text), "{after}");
        let config = Config::load_from(&path).unwrap();
        assert_eq!(config.templates.len(), 2);
        assert_eq!(config.defaults.tail, Some(5));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_config_file_that_is_a_link_stays_a_link() {
        let directory = directory("link");
        std::fs::create_dir_all(directory.join("dotfiles")).unwrap();
        let real = directory.join("dotfiles").join("gossip.toml");
        std::fs::write(&real, "# mine\n").unwrap();
        let link = directory.join("gossip.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        Config::save_template(&link, "tenant", &session()).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        let text = std::fs::read_to_string(&real).unwrap();
        assert!(
            text.contains("# mine") && text.contains("[templates.tenant]"),
            "{text}"
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_config_that_cannot_be_written_is_an_error_and_stays_as_it_is() {
        use std::os::unix::fs::PermissionsExt;

        let directory = directory("read-only");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("gossip.toml");
        std::fs::write(&path, "# mine\n").unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = Config::save_template(&path, "tenant", &session());
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Whoever runs the tests as root can write anywhere.
        if let Err(error) = result {
            assert!(matches!(error, ConfigError::Write { .. }), "{error}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n");
        }
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
