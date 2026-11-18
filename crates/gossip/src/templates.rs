//! `gossip templates`: what the config file holds.

use std::io::Write;

use anyhow::bail;
use gossip_core::{ColumnsSetting, Config, Filter, FormatSetting, SessionSpec, format_duration};

use crate::cli::{TemplatesArgs, TemplatesCommand};
use crate::table::Table;

pub fn run(args: &TemplatesArgs, out: &mut impl Write) -> anyhow::Result<()> {
    let config = Config::load()?;
    match &args.command {
        None => {
            if config.templates.is_empty() {
                let path = Config::path()?;
                eprintln!("The config file {} holds no templates.", path.display());
            } else {
                write!(out, "{}", list(&config).render())?;
            }
        }
        Some(TemplatesCommand::Show { name }) => {
            write!(out, "{}", show(name, find(&config, name)?))?;
        }
    }
    Ok(())
}

/// The template of this name, or an error that says which ones there are.
pub fn find<'a>(config: &'a Config, name: &str) -> anyhow::Result<&'a SessionSpec> {
    if let Some(template) = config.templates.get(name) {
        return Ok(template);
    }
    if config.templates.is_empty() {
        bail!("there is no template `{name}`; the config file holds no templates");
    }
    let known: Vec<&str> = config.templates.keys().map(String::as_str).collect();
    bail!(
        "there is no template `{name}`; the templates are: {}",
        known.join(", ")
    );
}

/// The templates with their targets.
fn list(config: &Config) -> Table {
    let mut table = Table::new(&["NAME", "TARGETS"]);
    for (name, template) in &config.templates {
        let targets: Vec<String> = template.targets.iter().map(ToString::to_string).collect();
        table.row(vec![name.clone(), targets.join(", ")]);
    }
    table
}

/// One template as it stands in the config file: text that can be pasted
/// back into it.
fn show(name: &str, template: &SessionSpec) -> String {
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
        line("filter", filter_text(filter, 0));
    }
    if let Some(fields) = &template.fields {
        let fields: Vec<String> = fields.iter().map(|field| quoted(field)).collect();
        line("fields", format!("[{}]", fields.join(", ")));
    }
    if let Some(output) = template.output {
        line("output", quoted(output.as_str()));
    }
    if !template.columns.is_empty() {
        line("columns", columns_text(&template.columns));
    }
    text
}

/// A `columns` table as the config file writes it on one line.
fn columns_text(columns: &ColumnsSetting) -> String {
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

/// A filter as the config file writes it: a rule on one line, `all` and
/// `any` with one filter per line.
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

/// A string as TOML writes it.
fn quoted(text: &str) -> String {
    toml::Value::String(text.to_string()).to_string()
}

/// A key as TOML writes it: bare when it can be, quoted otherwise.
fn key(name: &str) -> String {
    let bare = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if bare { name.to_string() } else { quoted(name) }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn the_list_shows_the_templates_with_their_targets() {
        assert_eq!(
            list(&config()).render(),
            "NAME           TARGETS\n\
             dev-api        flowcpt/flowcpt-api, flowcpt/flowcpt-web-api\n\
             everything     flowcpt/cronjob:nightly-sync, monitoring/prometheus/*/exporter\n\
             staging-all    flowcpt-stg\n\
             with a space   flowcpt\n"
        );
    }

    #[test]
    fn show_prints_a_template_as_the_config_file_writes_it() {
        let config = config();
        assert_eq!(
            show("dev-api", &config.templates["dev-api"]),
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
            show("everything", &config.templates["everything"]),
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
    fn what_show_prints_reads_back_as_the_same_template() {
        let config = config();
        for (name, template) in &config.templates {
            let text = show(name, template);
            let read_back = Config::from_toml(&text)
                .unwrap_or_else(|error| panic!("`{name}` does not read back:\n{text}\n{error}"));
            assert_eq!(&read_back.templates[name], template, "{text}");
        }
    }

    #[test]
    fn a_template_that_is_not_there_names_the_ones_that_are() {
        let config = config();
        assert!(find(&config, "dev-api").is_ok());
        assert_eq!(
            find(&config, "dev").unwrap_err().to_string(),
            "there is no template `dev`; the templates are: dev-api, everything, staging-all, with a space"
        );
        assert_eq!(
            find(&Config::default(), "dev").unwrap_err().to_string(),
            "there is no template `dev`; the config file holds no templates"
        );
    }

    #[test]
    fn strings_and_keys_as_toml_writes_them() {
        assert_eq!(quoted("api"), r#""api""#);
        assert_eq!(quoted(r#"say "hi""#), r#"'say "hi"'"#);
        assert_eq!(key("dev-api"), "dev-api");
        assert_eq!(key("with a space"), r#""with a space""#);
    }
}
