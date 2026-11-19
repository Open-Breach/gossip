//! The filter of a run, as the flags of the command line and the commands
//! typed later build it.
//!
//! Both build the same `Filter` the same way: a flag is the first rule, a
//! command replaces or adds to it. The rules are about log lines; events
//! always pass them. Beside a filter of the config file they narrow: a
//! record must pass both.

use anyhow::{Context, bail};
use gossip_core::{Filter, Kind, Level};

use crate::cli::FilterFlags;

/// The rules about lines that the filter flags stand for, and that the
/// filter commands change: every one of them must hold for a line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LineRules {
    /// `level >= level`
    level: Option<Level>,
    /// `message contains text`
    grep: Option<String>,
    /// `message matches regex`
    matches: Option<String>,
    /// `f.key == value`, in the order they were given.
    fields: Vec<(String, String)>,
}

impl LineRules {
    /// The rules the filter flags stand for.
    pub fn from_args(args: &FilterFlags) -> anyhow::Result<LineRules> {
        let mut rules = LineRules {
            level: args.level,
            grep: args.grep.clone(),
            ..LineRules::default()
        };
        if let Some(regex) = &args.matches {
            rules.set_match(Some(regex)).context("--match")?;
        }
        for pair in &args.fields_equal {
            let Some((key, value)) = pair.split_once('=') else {
                bail!("--field takes KEY=VALUE, and `{pair}` has no `=`");
            };
            // The flag can be given several times, also for one key.
            Filter::field_equals(key, value).context("--field")?;
            rules.fields.push((key.to_string(), value.to_string()));
        }
        Ok(rules)
    }

    /// Sets the level rule, or removes it.
    pub fn set_level(&mut self, level: Option<Level>) {
        self.level = level;
    }

    /// Sets the rule on the text of the message, or removes it.
    pub fn set_grep(&mut self, text: Option<&str>) {
        self.grep = text.map(str::to_string);
    }

    /// Sets the rule on the pattern of the message, or removes it. A text
    /// that is not a regular expression changes nothing.
    pub fn set_match(&mut self, regex: Option<&str>) -> anyhow::Result<()> {
        if let Some(regex) = regex {
            Filter::message_matches(regex)?;
        }
        self.matches = regex.map(str::to_string);
        Ok(())
    }

    /// Sets the rule on a field: the one rule on that key from here on. In
    /// the place of the first rule on the key when there was one, so that
    /// the order of the rules stays.
    pub fn set_field(&mut self, key: &str, value: &str) -> anyhow::Result<()> {
        Filter::field_equals(key, value)?;
        let first = self.fields.iter().position(|(known, _)| known == key);
        self.fields.retain(|(known, _)| known != key);
        // Nothing before the first rule on the key was taken away.
        let place = first.unwrap_or(self.fields.len());
        self.fields
            .insert(place, (key.to_string(), value.to_string()));
        Ok(())
    }

    /// Removes the rules on a field. Whether there was one.
    pub fn remove_field(&mut self, key: &str) -> bool {
        let before = self.fields.len();
        self.fields.retain(|(known, _)| known != key);
        self.fields.len() != before
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        *self == LineRules::default()
    }

    /// The filter the rules stand for: every one of them must hold for a
    /// line. The rules are about lines; events pass them, and `--no-events`
    /// is what leaves events out. None when there is no rule.
    pub fn filter(&self) -> Option<Filter> {
        let mut rules = Vec::new();
        if let Some(level) = self.level {
            rules.push(Filter::level_at_least(level));
        }
        if let Some(text) = &self.grep {
            rules.push(Filter::message_contains(text));
        }
        if let Some(regex) = &self.matches {
            rules.push(Filter::message_matches(regex).expect("the pattern was checked when set"));
        }
        for (key, value) in &self.fields {
            let rule = Filter::field_equals(key, value).expect("the rule was checked when set");
            rules.push(rule);
        }
        let lines = match rules.len() {
            0 => return None,
            1 => rules.remove(0),
            _ => Filter::All(rules),
        };
        Some(Filter::Any(vec![Filter::kind_is(Kind::Event), lines]))
    }
}

/// The whole filter of a run: the filter of the config file, when there is
/// one and it was not dropped, narrowed by the rules about lines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FilterState {
    /// The filter of the template or of the defaults. It is exactly what is
    /// written there, and applies to events too.
    pub base: Option<Filter>,
    pub rules: LineRules,
}

impl FilterState {
    /// The filter a record must pass: both parts, or the one there is.
    pub fn filter(&self) -> Option<Filter> {
        match (self.base.clone(), self.rules.filter()) {
            (Some(base), Some(rules)) => Some(Filter::All(vec![base, rules])),
            (base, rules) => base.or(rules),
        }
    }

    /// Removes every rule, the filter of the config file included.
    pub fn clear(&mut self) {
        *self = FilterState::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    use crate::cli::{Cli, Command};

    fn rules(flags: &[&str]) -> LineRules {
        let cli = Cli::try_parse_from(["gossip", "follow", "flowcpt"].iter().chain(flags)).unwrap();
        match cli.command {
            Command::Follow(args) => LineRules::from_args(&args.filter).unwrap(),
            other => panic!("expected the follow command, got {other:?}"),
        }
    }

    /// The part of a filter that is about lines.
    fn lines(rules: &LineRules) -> Filter {
        match rules.filter() {
            Some(Filter::Any(mut parts)) => {
                assert_eq!(parts[0], Filter::kind_is(Kind::Event));
                parts.remove(1)
            }
            other => panic!("expected events or lines, got {other:?}"),
        }
    }

    #[test]
    fn no_rule_is_no_filter() {
        assert!(LineRules::default().is_empty());
        assert_eq!(LineRules::default().filter(), None);
        assert_eq!(FilterState::default().filter(), None);
    }

    #[test]
    fn a_command_builds_the_filter_the_flag_would_have_built() {
        // Each rule set later is the rule the flag stands for.
        let mut typed = LineRules::default();
        typed.set_level(Some(Level::Warn));
        assert_eq!(typed, rules(&["--level", "warn"]));
        typed.set_grep(Some("expired"));
        assert_eq!(typed, rules(&["--level", "warn", "--grep", "expired"]));
        typed.set_match(Some("^pay")).unwrap();
        typed.set_field("tenant_id", "t_123").unwrap();
        let flags = rules(&[
            "--level",
            "warn",
            "--grep",
            "expired",
            "--match",
            "^pay",
            "--field",
            "tenant_id=t_123",
        ]);
        assert_eq!(typed, flags);
        assert_eq!(typed.filter(), flags.filter());
        // In whatever order they were typed, the filter is the same.
        let mut other_order = LineRules::default();
        other_order.set_field("tenant_id", "t_123").unwrap();
        other_order.set_match(Some("^pay")).unwrap();
        other_order.set_grep(Some("expired"));
        other_order.set_level(Some(Level::Warn));
        assert_eq!(other_order.filter(), flags.filter());
    }

    #[test]
    fn a_flag_is_the_first_rule_and_a_command_replaces_or_adds_to_it() {
        let mut state = rules(&["--level", "warn", "--grep", "expired"]);

        // The same command again replaces its own rule and leaves the rest.
        state.set_level(Some(Level::Error));
        assert_eq!(state, rules(&["--level", "error", "--grep", "expired"]));
        state.set_grep(Some("declined"));
        assert_eq!(state, rules(&["--level", "error", "--grep", "declined"]));

        // Another command adds a rule, joined by `and`.
        state.set_field("tenant_id", "t_123").unwrap();
        assert_eq!(
            lines(&state),
            Filter::All(vec![
                Filter::level_at_least(Level::Error),
                Filter::message_contains("declined"),
                Filter::field_equals("tenant_id", "t_123").unwrap(),
            ])
        );

        // Each can be taken away again.
        state.set_level(None);
        state.set_grep(None);
        assert_eq!(state, rules(&["--field", "tenant_id=t_123"]));
        assert!(state.remove_field("tenant_id"));
        assert!(state.is_empty());
        assert!(!state.remove_field("tenant_id"));
    }

    #[test]
    fn a_field_command_is_the_one_rule_on_its_key() {
        let mut state = rules(&["--field", "tenant_id=t_1", "--field", "status=200"]);
        state.set_field("tenant_id", "t_2").unwrap();
        // In the place of the old rule, so the order stays.
        assert_eq!(
            state,
            rules(&["--field", "tenant_id=t_2", "--field", "status=200"])
        );
        state.set_field("route", "/api").unwrap();
        assert_eq!(
            state,
            rules(&[
                "--field",
                "tenant_id=t_2",
                "--field",
                "status=200",
                "--field",
                "route=/api"
            ])
        );

        // The flag may name a key twice; the command leaves one rule on it.
        let mut twice = rules(&[
            "--field",
            "tenant_id=t_1",
            "--field",
            "status=200",
            "--field",
            "tenant_id=t_9",
        ]);
        twice.set_field("tenant_id", "t_2").unwrap();
        assert_eq!(
            twice,
            rules(&["--field", "tenant_id=t_2", "--field", "status=200"])
        );
    }

    #[test]
    fn a_rule_that_cannot_be_read_changes_nothing() {
        let mut state = rules(&["--match", "^pay", "--field", "tenant_id=t_1"]);
        let before = state.clone();
        assert!(state.set_match(Some("(unclosed")).is_err());
        assert!(state.set_field("", "t_2").is_err());
        assert_eq!(state, before);
    }

    #[test]
    fn the_rules_narrow_the_filter_of_the_config_file() {
        let base = Filter::field_equals("tenant_id", "t_123").unwrap();
        let mut state = FilterState {
            base: Some(base.clone()),
            rules: LineRules::default(),
        };
        assert_eq!(state.filter(), Some(base.clone()));

        state.rules.set_level(Some(Level::Warn));
        let warn = Filter::Any(vec![
            Filter::kind_is(Kind::Event),
            Filter::level_at_least(Level::Warn),
        ]);
        assert_eq!(
            state.filter(),
            Some(Filter::All(vec![base.clone(), warn.clone()]))
        );

        // Taking the rule away leaves the filter of the config file.
        state.rules.set_level(None);
        assert_eq!(state.filter(), Some(base));

        // Clearing removes every rule, that filter included; a rule set
        // after that is the whole filter.
        state.clear();
        assert_eq!(state.filter(), None);
        state.rules.set_level(Some(Level::Warn));
        assert_eq!(state.filter(), Some(warn));
    }
}
