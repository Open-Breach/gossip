//! `gossip templates`: what the config file holds.

use std::io::Write;

use anyhow::bail;
use gossip_core::{Config, SessionSpec, template_toml};

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
            write!(out, "{}", template_toml(name, find(&config, name)?))?;
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
pub fn list(config: &Config) -> Table {
    let mut table = Table::new(&["NAME", "TARGETS"]);
    for (name, template) in &config.templates {
        let targets: Vec<String> = template.targets.iter().map(ToString::to_string).collect();
        table.row(vec![name.clone(), targets.join(", ")]);
    }
    table
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
}
