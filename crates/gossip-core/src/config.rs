//! The config file: `[defaults]` and the templates, the named and saved
//! combinations of targets, filters and options.
//!
//! One TOML file, `~/.config/gossip/gossip.toml` on Linux (the platform's
//! config directory elsewhere); `$GOSSIP_CONFIG` overrides the path. Nothing
//! secret goes in it: credentials stay in the kubeconfig.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use directories::ProjectDirs;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::columns::{Columns, ColumnsSetting};
use crate::filter::Filter;
use crate::parser::FormatChoice;
use crate::target::{Pattern, Target};

/// The environment variable that overrides the path of the config file.
pub const CONFIG_ENV: &str = "GOSSIP_CONFIG";

/// How many lines of history a container gets at start when neither `since`
/// nor `tail` is given anywhere.
pub const DEFAULT_TAIL: u64 = 20;

/// Why the config file could not be read or written.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read the config file {}: {source}", path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("the config file {} is not valid\n{source}", path.display())]
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    #[error("cannot write the config file {}: {source}", path.display())]
    Write { path: PathBuf, source: io::Error },
    #[error("cannot write the config as TOML: {0}")]
    Serialise(#[from] toml::ser::Error),
    #[error("there is no home directory to look for the config file in; set {CONFIG_ENV}")]
    NoHome,
}

/// The whole config file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Fills in anything a template or the command line leaves out.
    #[serde(default, skip_serializing_if = "Defaults::is_empty")]
    pub defaults: Defaults,
    /// The columns of the prefix of a line, for every run; a template and
    /// the `--columns` flag go over it.
    #[serde(default, skip_serializing_if = "ColumnsSetting::is_empty")]
    pub columns: ColumnsSetting,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub templates: BTreeMap<String, Template>,
}

impl Config {
    /// The path of the config file: `$GOSSIP_CONFIG` when it is set, the
    /// platform's config directory otherwise.
    pub fn path() -> Result<PathBuf, ConfigError> {
        config_path(std::env::var_os(CONFIG_ENV))
    }

    /// Reads the config file from its usual place. A file that does not
    /// exist is an empty config, unless `$GOSSIP_CONFIG` names it.
    pub fn load() -> Result<Config, ConfigError> {
        let named = std::env::var_os(CONFIG_ENV).is_some_and(|value| !value.is_empty());
        let path = Config::path()?;
        if !named && !path.exists() {
            return Ok(Config::default());
        }
        Config::load_from(&path)
    }

    /// The lowest layer the config file gives: its `[defaults]` and its
    /// `[columns]`, as a template without targets.
    pub fn base(&self) -> Template {
        Template {
            columns: self.columns,
            ..self.defaults.clone().into()
        }
    }

    /// Reads a config file.
    pub fn load_from(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Config::from_toml(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
    }

    /// Reads a config from the text of a config file. Unknown keys are an
    /// error that names the line.
    pub fn from_toml(text: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(text)
    }

    /// Writes the config as the text of a config file.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }

    /// Writes a config file, creating its directory when needed.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let text = self.to_toml()?;
        let write = || {
            if let Some(directory) = path.parent() {
                std::fs::create_dir_all(directory)?;
            }
            std::fs::write(path, text)
        };
        write().map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })
    }
}

fn config_path(from_env: Option<OsString>) -> Result<PathBuf, ConfigError> {
    if let Some(path) = from_env.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let directories = ProjectDirs::from("", "", "gossip").ok_or(ConfigError::NoHome)?;
    Ok(directories.config_dir().join("gossip.toml"))
}

/// A named, saved combination of targets, filters and options. Every key is
/// optional; what a template leaves out comes from `[defaults]`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    /// The kubeconfig context of the cluster.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<Target>,
    /// Only containers whose name matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<Pattern>,
    /// History per container at start, as a duration such as `5m`.
    #[serde(
        default,
        with = "crate::duration::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub since: Option<Duration>,
    /// History per container at start, as a number of lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<u64>,
    /// Whether cluster events are merged into the stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<FormatSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter>,
    /// The fields of a line the text output shows, in this order; none of
    /// them for an empty list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Output>,
    /// The columns of the prefix of a line; what it leaves out comes from
    /// the `[columns]` of the config file.
    #[serde(default, skip_serializing_if = "ColumnsSetting::is_empty")]
    pub columns: ColumnsSetting,
}

impl Template {
    /// Fills in whatever this template leaves out from a lower layer: the
    /// command line over a template, a template over the defaults.
    ///
    /// `since` and `tail` are one setting, the history at start: a layer
    /// that gives either one decides both.
    pub fn or(mut self, lower: Template) -> Template {
        self.context = self.context.or(lower.context);
        if self.targets.is_empty() {
            self.targets = lower.targets;
        }
        self.container = self.container.or(lower.container);
        if self.since.is_none() && self.tail.is_none() {
            self.since = lower.since;
            self.tail = lower.tail;
        }
        self.events = self.events.or(lower.events);
        self.format = self.format.or(lower.format);
        self.filter = self.filter.or(lower.filter);
        self.fields = self.fields.or(lower.fields);
        self.output = self.output.or(lower.output);
        self.columns = self.columns.or(lower.columns);
        self
    }

    /// Fills in what is still left out with the built-in defaults: the last
    /// 20 lines, events on, formats detected, text output, the built-in
    /// columns.
    pub fn resolve(self) -> Settings {
        let tail = match (self.since, self.tail) {
            (None, None) => Some(DEFAULT_TAIL),
            (_, tail) => tail,
        };
        Settings {
            context: self.context,
            targets: self.targets,
            container: self.container,
            since: self.since,
            tail,
            events: self.events.unwrap_or(true),
            format: self.format.unwrap_or_default(),
            filter: self.filter,
            fields: self.fields,
            output: self.output.unwrap_or_default(),
            columns: self.columns.resolve(),
        }
    }
}

/// The `[defaults]` table: everything a template holds except its targets.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<Pattern>,
    #[serde(
        default,
        with = "crate::duration::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub since: Option<Duration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<FormatSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Output>,
}

impl Defaults {
    pub fn is_empty(&self) -> bool {
        *self == Defaults::default()
    }
}

impl From<Defaults> for Template {
    fn from(defaults: Defaults) -> Self {
        Template {
            context: defaults.context,
            targets: Vec::new(),
            container: defaults.container,
            since: defaults.since,
            tail: defaults.tail,
            events: defaults.events,
            format: defaults.format,
            filter: defaults.filter,
            fields: defaults.fields,
            output: defaults.output,
            columns: ColumnsSetting::default(),
        }
    }
}

/// What a session and its rendering run with, once the command line, a
/// template, the defaults and the built-in defaults are layered.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// The kubeconfig context; none means the current one.
    pub context: Option<String>,
    pub targets: Vec<Target>,
    pub container: Option<Pattern>,
    pub since: Option<Duration>,
    pub tail: Option<u64>,
    pub events: bool,
    pub format: FormatSetting,
    pub filter: Option<Filter>,
    pub fields: Option<Vec<String>>,
    pub output: Output,
    pub columns: Columns,
}

/// The `format` key: one format for every container, or a table of formats
/// by container name, as in `format = { api = "json", envoy = "text" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum FormatSetting {
    All(FormatChoice),
    PerContainer(BTreeMap<String, FormatChoice>),
}

impl FormatSetting {
    /// The format of a container; a container the table does not name has
    /// its format detected.
    pub fn for_container(&self, container: &str) -> FormatChoice {
        match self {
            FormatSetting::All(choice) => *choice,
            FormatSetting::PerContainer(table) => table.get(container).copied().unwrap_or_default(),
        }
    }
}

impl Default for FormatSetting {
    fn default() -> Self {
        FormatSetting::All(FormatChoice::Auto)
    }
}

impl<'de> Deserialize<'de> for FormatSetting {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FormatVisitor;

        impl<'de> Visitor<'de> for FormatVisitor {
            type Value = FormatSetting;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "a format (auto, json, logfmt or text) or a table of formats by container",
                )
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
                text.parse().map(FormatSetting::All).map_err(E::custom)
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut table = BTreeMap::new();
                while let Some((container, format)) = map.next_entry::<String, String>()? {
                    let choice = format.parse().map_err(de::Error::custom)?;
                    table.insert(container, choice);
                }
                Ok(FormatSetting::PerContainer(table))
            }
        }

        deserializer.deserialize_any(FormatVisitor)
    }
}

/// How records are rendered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Output {
    /// One line of text per record.
    #[default]
    Text,
    /// One JSON object per record.
    Json,
}

impl Output {
    pub fn as_str(self) -> &'static str {
        match self {
            Output::Text => "text",
            Output::Json => "json",
        }
    }
}

impl fmt::Display for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A word that names no output.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("`{0}` is not an output; the outputs are text and json")]
pub struct UnknownOutput(pub String);

impl FromStr for Output {
    type Err = UnknownOutput;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text" => Ok(Output::Text),
            "json" => Ok(Output::Json),
            _ => Err(UnknownOutput(s.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::columns::Cut;
    use crate::record::Level;

    /// The example of the spec, section 6, as it stands there.
    const SPEC_EXAMPLE: &str = r#"
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
"#;

    fn target(text: &str) -> Target {
        text.parse().unwrap()
    }

    #[test]
    fn the_example_of_the_spec() {
        let config = Config::from_toml(SPEC_EXAMPLE).unwrap();

        assert_eq!(config.defaults.context.as_deref(), Some("aks-vulcan-01"));
        assert_eq!(config.defaults.tail, Some(20));
        assert_eq!(config.defaults.events, Some(true));

        let names: Vec<&str> = config.templates.keys().map(String::as_str).collect();
        assert_eq!(names, ["dev-api", "staging-all"]);

        let dev = &config.templates["dev-api"];
        assert_eq!(dev.context.as_deref(), Some("aks-vulcan-01"));
        assert_eq!(
            dev.targets,
            [
                target("flowcpt/flowcpt-api"),
                target("flowcpt/flowcpt-web-api")
            ]
        );
        assert_eq!(dev.container, Some(Pattern::new("api").unwrap()));
        assert_eq!(
            dev.filter,
            Some(Filter::All(vec![
                Filter::level_at_least(Level::Info),
                Filter::Any(vec![
                    Filter::field_equals("tenant_id", "t_123").unwrap(),
                    Filter::kind_is(crate::record::Kind::Event),
                ]),
            ]))
        );
        assert_eq!(
            dev.fields,
            Some(vec!["tenant_id".to_string(), "request_id".to_string()])
        );
        assert_eq!(dev.since, None);
        assert_eq!(dev.tail, None);

        let staging = &config.templates["staging-all"];
        assert_eq!(staging.targets, [target("flowcpt-stg")]);
        assert_eq!(staging.since, Some(Duration::from_secs(300)));
        assert_eq!(staging.filter, None);
    }

    #[test]
    fn every_key_a_template_holds() {
        let config = Config::from_toml(
            r#"
            [templates.everything]
            context = "aks-vulcan-01"
            targets = ["flowcpt/cronjob:nightly-sync", "monitoring/prometheus/*/exporter"]
            container = "{api,envoy}"
            since = "1h30m"
            tail = 500
            events = false
            format = { api = "json", envoy = "text" }
            filter = { field = "level", op = ">=", value = "warn" }
            fields = ["tenant_id"]
            output = "json"
            columns = { pod = { width = 24, cut = "start" }, level = { width = 0 } }
            "#,
        )
        .unwrap();
        let template = &config.templates["everything"];
        assert_eq!(template.columns, "pod=24:start,level=0".parse().unwrap());
        assert_eq!(template.targets.len(), 2);
        assert!(template.container.as_ref().unwrap().matches("envoy"));
        assert_eq!(template.since, Some(Duration::from_secs(5400)));
        assert_eq!(template.tail, Some(500));
        assert_eq!(template.events, Some(false));
        let format = template.format.as_ref().unwrap();
        assert_eq!(format.for_container("api"), FormatChoice::Json);
        assert_eq!(format.for_container("envoy"), FormatChoice::Text);
        assert_eq!(format.for_container("sidecar"), FormatChoice::Auto);
        assert_eq!(template.output, Some(Output::Json));
    }

    /// The example of the round-two spec, section 5, as it stands there.
    const COLUMNS_EXAMPLE: &str = r#"
[columns]
pod = { width = 20, cut = "middle" }
container = { width = 10, cut = "end" }
level = { width = 5 }
"#;

    #[test]
    fn the_columns_table_of_the_spec() {
        let config = Config::from_toml(COLUMNS_EXAMPLE).unwrap();
        assert_eq!(
            config.columns,
            "pod=20:middle,container=10:end,level=5".parse().unwrap()
        );
        assert_eq!(config.columns.resolve(), Columns::default());
        assert!(config.defaults.is_empty() && config.templates.is_empty());
        // Written as sub-tables it reads the same.
        let tables = Config::from_toml(
            "[columns.pod]\nwidth = 20\ncut = \"middle\"\n[columns.container]\nwidth = 10\ncut = \"end\"\n[columns.level]\nwidth = 5\n",
        )
        .unwrap();
        assert_eq!(tables, config);
    }

    #[test]
    fn the_columns_layer_part_by_part_from_the_flag_down_to_the_config_file() {
        let config = Config::from_toml(
            r#"
            [defaults]
            tail = 5

            [columns]
            pod = { width = 30, cut = "start" }
            container = { width = 8 }

            [templates.t]
            targets = ["flowcpt"]
            columns = { pod = { cut = "end" }, level = { width = 0 } }
            "#,
        )
        .unwrap();
        // The lowest layer of the file holds its defaults and its columns.
        let base = config.base();
        assert_eq!(base.tail, Some(5));
        assert_eq!(base.columns, config.columns);

        let flag = Template {
            columns: "container=12".parse().unwrap(),
            ..Template::default()
        };
        let settings = flag
            .or(config.templates["t"].clone())
            .or(config.base())
            .resolve();
        let columns = settings.columns;
        // The width of the file with the side of the template.
        assert_eq!((columns.pod.width, columns.pod.cut), (30, Cut::End));
        // The width of the flag with the built-in side.
        assert_eq!(
            (columns.container.width, columns.container.cut),
            (12, Cut::End)
        );
        assert_eq!(columns.level.width, 0);
        assert_eq!(columns.time, Columns::default().time);
    }

    #[test]
    fn a_columns_table_that_cannot_be_read_is_an_error_with_the_line_number() {
        for (text, line, expected) in [
            ("[columns]\nnode = { width = 20 }\n", "line 2", "node"),
            ("[columns]\npod = { wide = 20 }\n", "line 2", "wide"),
            ("[columns]\npod = { width = -1 }\n", "line 2", "width"),
            ("[columns]\npod = { width = \"20\" }\n", "line 2", "width"),
            ("[columns]\n\npod = { cut = \"left\" }\n", "line 3", "left"),
            ("[columns]\npod = 20\n", "line 2", "pod"),
            (
                "[templates.t]\ncolumns = { node = { width = 20 } }\n",
                "line 2",
                "node",
            ),
            // The columns are a table of their own, not a default.
            (
                "[defaults]\ncolumns = { pod = { width = 20 } }\n",
                "line 2",
                "columns",
            ),
        ] {
            let error = error_of(text);
            assert!(error.contains(line), "`{text}` gave: {error}");
            assert!(error.contains(expected), "`{text}` gave: {error}");
        }
    }

    #[test]
    fn format_is_one_word_or_a_table_by_container() {
        let config = Config::from_toml(
            r#"
            [defaults]
            format = "logfmt"
            [templates.a]
            format = "text"
            [templates.b]
            format = { api = "json" }
            "#,
        )
        .unwrap();
        assert_eq!(
            config.defaults.format,
            Some(FormatSetting::All(FormatChoice::Logfmt))
        );
        assert_eq!(
            config.templates["a"].format,
            Some(FormatSetting::All(FormatChoice::Text))
        );
        assert_eq!(
            config.templates["b"].format,
            Some(FormatSetting::PerContainer(BTreeMap::from([(
                "api".to_string(),
                FormatChoice::Json
            )])))
        );
    }

    #[test]
    fn an_empty_file_is_an_empty_config() {
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
        assert_eq!(
            Config::from_toml("# nothing yet\n").unwrap(),
            Config::default()
        );
    }

    /// The error of a config that must not parse.
    fn error_of(text: &str) -> String {
        Config::from_toml(text)
            .expect_err("the config should be refused")
            .to_string()
    }

    #[test]
    fn an_unknown_key_is_an_error_with_the_line_number() {
        let error = error_of(
            "[templates.dev-api]\ncontext = \"aks-vulcan-01\"\ntragets = [\"flowcpt/flowcpt-api\"]\n",
        );
        assert!(error.contains("line 3"), "{error}");
        assert!(error.contains("tragets"), "{error}");

        let error = error_of("[defaults]\ntail = 20\n\nevnts = true\n");
        assert!(error.contains("line 4"), "{error}");
        assert!(error.contains("evnts"), "{error}");

        let error = error_of("[default]\ntail = 20\n");
        assert!(error.contains("line 1"), "{error}");
        assert!(error.contains("default"), "{error}");

        // Targets belong to a template, not to the defaults.
        let error = error_of("[defaults]\ntargets = [\"flowcpt\"]\n");
        assert!(error.contains("line 2"), "{error}");
        assert!(error.contains("targets"), "{error}");
    }

    #[test]
    fn an_unknown_key_inside_a_filter_is_an_error_with_the_line_number() {
        let error = error_of(
            "[templates.t]\ntargets = [\"flowcpt\"]\nfilter = { all = [\n  { feild = \"level\", op = \">=\", value = \"info\" },\n] }\n",
        );
        assert!(error.contains("line 4"), "{error}");
        assert!(error.contains("feild"), "{error}");
    }

    #[test]
    fn a_bad_value_is_an_error_with_the_line_number() {
        for (text, line, expected) in [
            (
                "[templates.t]\ntargets = [\"a/b/c/d/e\"]\n",
                "line 2",
                "levels",
            ),
            ("[templates.t]\n\nsince = \"soon\"\n", "line 3", "duration"),
            (
                "[templates.t]\ncontainer = \"a b\"\n",
                "line 2",
                "whitespace",
            ),
            (
                "[templates.t]\nformat = \"yaml\"\n",
                "line 2",
                "not a format",
            ),
            (
                "[templates.t]\nformat = { api = \"yaml\" }\n",
                "line 2",
                "not a format",
            ),
            ("[templates.t]\noutput = \"xml\"\n", "line 2", "xml"),
            ("[templates.t]\ntail = -1\n", "line 2", "tail"),
            ("[templates.t]\ntail = \"20\"\n", "line 2", "tail"),
            ("[templates.t]\nevents = \"yes\"\n", "line 2", "events"),
            (
                "[templates.t]\nfilter = { field = \"level\", op = \">=\", value = \"loud\" }\n",
                "line 2",
                "not a level",
            ),
            (
                "[templates.t]\nfilter = { field = \"message\", op = \"matches\", value = \"(\" }\n",
                "line 2",
                "regular expression",
            ),
        ] {
            let error = error_of(text);
            assert!(error.contains(line), "`{text}` gave: {error}");
            assert!(error.contains(expected), "`{text}` gave: {error}");
        }
    }

    #[test]
    fn a_template_fills_in_from_the_layer_below() {
        let config = Config::from_toml(SPEC_EXAMPLE).unwrap();
        let dev = config.templates["dev-api"].clone();
        let layered = dev.clone().or(config.defaults.clone().into());

        // What the template gives stays.
        assert_eq!(layered.targets, dev.targets);
        assert_eq!(layered.container, dev.container);
        assert_eq!(layered.filter, dev.filter);
        assert_eq!(layered.fields, dev.fields);
        // What it leaves out comes from the defaults.
        assert_eq!(layered.tail, Some(20));
        assert_eq!(layered.events, Some(true));
    }

    #[test]
    fn the_upper_layer_wins() {
        let upper = Template {
            context: Some("other".into()),
            events: Some(false),
            output: Some(Output::Json),
            ..Template::default()
        };
        let lower = Template {
            context: Some("aks-vulcan-01".into()),
            targets: vec![target("flowcpt")],
            events: Some(true),
            output: Some(Output::Text),
            fields: Some(vec!["a".into()]),
            ..Template::default()
        };
        let layered = upper.or(lower);
        assert_eq!(layered.context.as_deref(), Some("other"));
        assert_eq!(layered.events, Some(false));
        assert_eq!(layered.output, Some(Output::Json));
        assert_eq!(layered.targets, [target("flowcpt")]);
        assert_eq!(layered.fields, Some(vec!["a".to_string()]));
    }

    #[test]
    fn since_and_tail_are_one_setting() {
        let config = Config::from_toml(SPEC_EXAMPLE).unwrap();
        // The template gives `since`, the defaults give `tail`: the template
        // decides the history, so the last five minutes are not cut to 20
        // lines.
        let staging = config.templates["staging-all"]
            .clone()
            .or(config.defaults.clone().into())
            .resolve();
        assert_eq!(staging.since, Some(Duration::from_secs(300)));
        assert_eq!(staging.tail, None);

        // A layer that gives both keeps both.
        let both = Template {
            since: Some(Duration::from_secs(60)),
            tail: Some(5),
            ..Template::default()
        };
        let settings = both
            .or(Template {
                tail: Some(100),
                ..Template::default()
            })
            .resolve();
        assert_eq!(settings.since, Some(Duration::from_secs(60)));
        assert_eq!(settings.tail, Some(5));

        // A layer that gives neither takes both from below.
        let settings = Template::default()
            .or(Template {
                since: Some(Duration::from_secs(60)),
                ..Template::default()
            })
            .resolve();
        assert_eq!(settings.since, Some(Duration::from_secs(60)));
        assert_eq!(settings.tail, None);
    }

    #[test]
    fn the_built_in_defaults() {
        let settings = Template::default().resolve();
        assert_eq!(settings.context, None);
        assert!(settings.targets.is_empty());
        assert_eq!(settings.container, None);
        assert_eq!(settings.since, None);
        assert_eq!(settings.tail, Some(DEFAULT_TAIL));
        assert!(settings.events);
        assert_eq!(settings.format, FormatSetting::All(FormatChoice::Auto));
        assert_eq!(settings.filter, None);
        assert_eq!(settings.fields, None);
        assert_eq!(settings.output, Output::Text);
        assert_eq!(settings.columns, Columns::default());
    }

    #[test]
    fn the_config_survives_a_round_trip_through_toml() {
        let config = Config::from_toml(SPEC_EXAMPLE).unwrap();
        let text = config.to_toml().unwrap();
        assert_eq!(Config::from_toml(&text).unwrap(), config, "{text}");
        let config = Config::from_toml(COLUMNS_EXAMPLE).unwrap();
        let text = config.to_toml().unwrap();
        assert_eq!(Config::from_toml(&text).unwrap(), config, "{text}");

        let everything = Config {
            defaults: Defaults {
                format: Some(FormatSetting::All(FormatChoice::Logfmt)),
                since: Some(Duration::from_secs(90)),
                ..Defaults::default()
            },
            columns: "time=0,pod=:start".parse().unwrap(),
            templates: BTreeMap::from([(
                "t".to_string(),
                Template {
                    columns: "container=8:none".parse().unwrap(),
                    targets: vec![target("flowcpt/-/*/api")],
                    format: Some(FormatSetting::PerContainer(BTreeMap::from([(
                        "api".to_string(),
                        FormatChoice::Json,
                    )]))),
                    output: Some(Output::Json),
                    ..Template::default()
                },
            )]),
        };
        let text = everything.to_toml().unwrap();
        assert_eq!(Config::from_toml(&text).unwrap(), everything, "{text}");
    }

    #[test]
    fn the_config_file_is_read_and_written() {
        let directory =
            std::env::temp_dir().join(format!("gossip-config-test-{}", std::process::id()));
        let path = directory.join("nested").join("gossip.toml");
        let config = Config::from_toml(SPEC_EXAMPLE).unwrap();

        config.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap(), config);

        std::fs::write(&path, "[templates.t]\ntragets = []\n").unwrap();
        let error = Config::load_from(&path).unwrap_err().to_string();
        assert!(error.contains("gossip.toml"), "{error}");
        assert!(error.contains("line 2"), "{error}");

        std::fs::remove_dir_all(&directory).unwrap();
        let error = Config::load_from(&path).unwrap_err();
        assert!(matches!(error, ConfigError::Read { .. }), "{error}");
    }

    #[test]
    fn the_path_of_the_config_file() {
        assert_eq!(
            config_path(Some(OsString::from("/etc/gossip/custom.toml"))).unwrap(),
            PathBuf::from("/etc/gossip/custom.toml")
        );
        // An empty variable counts as not set.
        for unset in [None, Some(OsString::new())] {
            if let Ok(path) = config_path(unset) {
                assert!(path.ends_with("gossip.toml"), "{}", path.display());
                assert!(
                    path.parent()
                        .is_some_and(|parent| parent.ends_with("gossip")),
                    "{}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn output_names() {
        assert_eq!("text".parse::<Output>(), Ok(Output::Text));
        assert_eq!("json".parse::<Output>(), Ok(Output::Json));
        assert!("xml".parse::<Output>().is_err());
        assert_eq!(Output::Json.to_string(), "json");
    }
}
