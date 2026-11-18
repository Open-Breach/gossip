//! Filters: rules `field operator value`, joined with `all`, `any` and `not`
//! into a tree, applied to records after parsing and before output.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::record::{Kind, Level, Record, UnknownKind, UnknownLevel, value_text};

/// Why a filter was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FilterError {
    #[error(
        "unknown field `{0}`; the fields are level, message, pod, container, namespace, kind, \
         and `f.<key>` for a field of the line"
    )]
    UnknownField(String),
    #[error("unknown operator `{0}`; the operators are ==, !=, >=, contains, matches and exists")]
    UnknownOperator(String),
    #[error("the operator `{op}` needs a value")]
    MissingValue { op: Op },
    #[error("the operator `exists` takes no value")]
    UnexpectedValue,
    #[error(
        "the operator `>=` compares levels, so it works on the field `level` only, not on `{field}`"
    )]
    OrderOnNonLevel { field: Field },
    #[error(transparent)]
    UnknownLevel(#[from] UnknownLevel),
    #[error(transparent)]
    UnknownKind(#[from] UnknownKind),
    #[error("`{pattern}` is not a valid regular expression: {reason}")]
    BadRegex { pattern: String, reason: String },
    #[error(
        "a filter is either a rule (`field`, `op`, `value`) or one of `all`, `any`, `not`; \
         this one {0}"
    )]
    BadShape(String),
}

/// What a rule looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    Level,
    Message,
    Pod,
    Container,
    Namespace,
    Kind,
    /// A key of the record's `fields`, written `f.<key>`.
    Extra(String),
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Field::Level => f.write_str("level"),
            Field::Message => f.write_str("message"),
            Field::Pod => f.write_str("pod"),
            Field::Container => f.write_str("container"),
            Field::Namespace => f.write_str("namespace"),
            Field::Kind => f.write_str("kind"),
            Field::Extra(key) => write!(f, "f.{key}"),
        }
    }
}

impl FromStr for Field {
    type Err = FilterError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "level" => Ok(Field::Level),
            "message" => Ok(Field::Message),
            "pod" => Ok(Field::Pod),
            "container" => Ok(Field::Container),
            "namespace" => Ok(Field::Namespace),
            "kind" => Ok(Field::Kind),
            _ => match s.strip_prefix("f.") {
                Some(key) if !key.is_empty() => Ok(Field::Extra(key.to_string())),
                _ => Err(FilterError::UnknownField(s.to_string())),
            },
        }
    }
}

/// How a rule compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `==`
    Eq,
    /// `!=`, the opposite of `==`: true also when the record has no such field.
    Ne,
    /// `>=`, on levels only.
    Ge,
    /// `contains`: the value is a part of the field's text.
    Contains,
    /// `matches`: the regular expression finds a match in the field's text.
    Matches,
    /// `exists`: the record has the field.
    Exists,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::Ge => ">=",
            Op::Contains => "contains",
            Op::Matches => "matches",
            Op::Exists => "exists",
        }
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Op {
    type Err = FilterError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "==" => Ok(Op::Eq),
            "!=" => Ok(Op::Ne),
            ">=" => Ok(Op::Ge),
            "contains" => Ok(Op::Contains),
            "matches" => Ok(Op::Matches),
            "exists" => Ok(Op::Exists),
            _ => Err(FilterError::UnknownOperator(s.to_string())),
        }
    }
}

/// One rule: `field operator value`.
///
/// A rule on a field the record does not have is false, except `!=`, which
/// is the opposite of `==` and so is true.
#[derive(Debug, Clone)]
pub struct Rule {
    field: Field,
    op: Op,
    value: Option<String>,
    test: Test,
}

/// The value of a rule, read once when the rule is built.
#[derive(Debug, Clone)]
enum Test {
    Level(Level),
    Kind(Kind),
    Text(String),
    Regex(Regex),
    Exists,
}

impl Rule {
    pub fn new(field: Field, op: Op, value: Option<&str>) -> Result<Self, FilterError> {
        let test = match (op, value) {
            (Op::Exists, None) => Test::Exists,
            (Op::Exists, Some(_)) => return Err(FilterError::UnexpectedValue),
            (op, None) => return Err(FilterError::MissingValue { op }),
            (Op::Ge, Some(value)) => {
                if field != Field::Level {
                    return Err(FilterError::OrderOnNonLevel { field });
                }
                Test::Level(value.parse()?)
            }
            (Op::Eq | Op::Ne, Some(value)) => match field {
                Field::Level => Test::Level(value.parse()?),
                Field::Kind => Test::Kind(value.parse()?),
                _ => Test::Text(value.to_string()),
            },
            (Op::Contains, Some(value)) => Test::Text(value.to_string()),
            (Op::Matches, Some(value)) => {
                Test::Regex(Regex::new(value).map_err(|error| FilterError::BadRegex {
                    pattern: value.to_string(),
                    reason: error.to_string(),
                })?)
            }
        };
        Ok(Rule {
            field,
            op,
            value: value.map(str::to_string),
            test,
        })
    }

    pub fn field(&self) -> &Field {
        &self.field
    }

    pub fn op(&self) -> Op {
        self.op
    }

    pub fn value(&self) -> Option<&str> {
        self.value.as_deref()
    }

    /// Whether the rule holds for a record.
    pub fn matches(&self, record: &Record) -> bool {
        match (&self.test, self.op) {
            (Test::Exists, _) => self.text_of(record).is_some(),
            (Test::Level(wanted), op) => match (record.level, op) {
                (Some(level), Op::Eq) => level == *wanted,
                (Some(level), Op::Ne) => level != *wanted,
                (Some(level), Op::Ge) => level >= *wanted,
                (None, Op::Ne) => true,
                _ => false,
            },
            (Test::Kind(wanted), Op::Eq) => record.kind == *wanted,
            (Test::Kind(wanted), _) => record.kind != *wanted,
            (Test::Text(wanted), Op::Eq) => {
                self.text_of(record).is_some_and(|text| text == *wanted)
            }
            (Test::Text(wanted), Op::Ne) => {
                !self.text_of(record).is_some_and(|text| text == *wanted)
            }
            (Test::Text(wanted), _) => self
                .text_of(record)
                .is_some_and(|text| text.contains(wanted.as_str())),
            (Test::Regex(regex), _) => self
                .text_of(record)
                .is_some_and(|text| regex.is_match(&text)),
        }
    }

    /// The text of the rule's field in a record, or none when the record has
    /// no such field.
    fn text_of<'a>(&self, record: &'a Record) -> Option<Cow<'a, str>> {
        let present = |text: &'a str| (!text.is_empty()).then_some(Cow::Borrowed(text));
        match &self.field {
            Field::Level => record.level.map(|level| Cow::Borrowed(level.as_str())),
            Field::Message => Some(Cow::Borrowed(record.message.as_str())),
            Field::Pod => present(&record.pod),
            Field::Container => record.container.as_deref().and_then(present),
            Field::Namespace => present(&record.namespace),
            Field::Kind => Some(Cow::Borrowed(record.kind.as_str())),
            Field::Extra(key) => record.fields.get(key).map(value_text),
        }
    }
}

impl PartialEq for Rule {
    fn eq(&self, other: &Self) -> bool {
        self.field == other.field && self.op == other.op && self.value == other.value
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.value {
            Some(value) => write!(f, "{} {} {value}", self.field, self.op),
            None => write!(f, "{} {}", self.field, self.op),
        }
    }
}

/// A filter: a rule, or `all` / `any` / `not` over filters.
///
/// In the config file a filter is a table that holds either a rule (`field`,
/// `op`, `value`) or exactly one of `all`, `any`, `not`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawFilter", into = "RawFilter")]
pub enum Filter {
    Rule(Rule),
    /// Every one holds; an empty list holds.
    All(Vec<Filter>),
    /// At least one holds; an empty list does not.
    Any(Vec<Filter>),
    Not(Box<Filter>),
}

impl Filter {
    /// A rule from its three parts as they are written.
    pub fn rule(field: &str, op: &str, value: Option<&str>) -> Result<Filter, FilterError> {
        Ok(Filter::Rule(Rule::new(field.parse()?, op.parse()?, value)?))
    }

    /// `level >= level`
    pub fn level_at_least(level: Level) -> Filter {
        let rule = Rule::new(Field::Level, Op::Ge, Some(level.as_str()));
        Filter::Rule(rule.expect("a level is a valid value for `>=`"))
    }

    /// `message contains text`
    pub fn message_contains(text: &str) -> Filter {
        let rule = Rule::new(Field::Message, Op::Contains, Some(text));
        Filter::Rule(rule.expect("any text is a valid value for `contains`"))
    }

    /// `message matches regex`
    pub fn message_matches(regex: &str) -> Result<Filter, FilterError> {
        Ok(Filter::Rule(Rule::new(
            Field::Message,
            Op::Matches,
            Some(regex),
        )?))
    }

    /// `f.key == value`
    pub fn field_equals(key: &str, value: &str) -> Result<Filter, FilterError> {
        if key.is_empty() {
            return Err(FilterError::UnknownField("f.".to_string()));
        }
        Ok(Filter::Rule(Rule::new(
            Field::Extra(key.to_string()),
            Op::Eq,
            Some(value),
        )?))
    }

    /// `kind == kind`
    pub fn kind_is(kind: Kind) -> Filter {
        let rule = Rule::new(Field::Kind, Op::Eq, Some(kind.as_str()));
        Filter::Rule(rule.expect("a kind is a valid value for `==`"))
    }

    /// Whether a record is shown: notices always are, anything else when the
    /// filter holds for it.
    pub fn accepts(&self, record: &Record) -> bool {
        record.kind == Kind::Notice || self.matches(record)
    }

    /// Whether the filter holds for a record, with no exception for notices.
    pub fn matches(&self, record: &Record) -> bool {
        match self {
            Filter::Rule(rule) => rule.matches(record),
            Filter::All(filters) => filters.iter().all(|filter| filter.matches(record)),
            Filter::Any(filters) => filters.iter().any(|filter| filter.matches(record)),
            Filter::Not(filter) => !filter.matches(record),
        }
    }
}

impl fmt::Display for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |f: &mut fmt::Formatter<'_>, filters: &[Filter], joiner: &str| {
            f.write_str("(")?;
            for (index, filter) in filters.iter().enumerate() {
                if index > 0 {
                    write!(f, " {joiner} ")?;
                }
                write!(f, "{filter}")?;
            }
            f.write_str(")")
        };
        match self {
            Filter::Rule(rule) => write!(f, "{rule}"),
            Filter::All(filters) => list(f, filters, "and"),
            Filter::Any(filters) => list(f, filters, "or"),
            Filter::Not(filter) => write!(f, "not {filter}"),
        }
    }
}

/// A filter as the config file writes it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    op: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<RawValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    all: Option<Vec<RawFilter>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    any: Option<Vec<RawFilter>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    not: Option<Box<RawFilter>>,
}

/// The value of a rule in the config file: a string, or a number or boolean
/// that is read as its text.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum RawValue {
    Text(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
}

impl RawValue {
    fn into_text(self) -> String {
        match self {
            RawValue::Text(text) => text,
            RawValue::Integer(number) => number.to_string(),
            RawValue::Float(number) => number.to_string(),
            RawValue::Bool(flag) => flag.to_string(),
        }
    }
}

impl TryFrom<RawFilter> for Filter {
    type Error = FilterError;

    fn try_from(raw: RawFilter) -> Result<Self, Self::Error> {
        let is_rule = raw.field.is_some() || raw.op.is_some() || raw.value.is_some();
        let shapes = [
            is_rule,
            raw.all.is_some(),
            raw.any.is_some(),
            raw.not.is_some(),
        ];
        match shapes.iter().filter(|present| **present).count() {
            0 => return Err(FilterError::BadShape("is empty".to_string())),
            1 => {}
            _ => {
                return Err(FilterError::BadShape("mixes several of them".to_string()));
            }
        }

        let convert_all = |filters: Vec<RawFilter>| {
            filters
                .into_iter()
                .map(Filter::try_from)
                .collect::<Result<Vec<_>, _>>()
        };
        if let Some(filters) = raw.all {
            return Ok(Filter::All(convert_all(filters)?));
        }
        if let Some(filters) = raw.any {
            return Ok(Filter::Any(convert_all(filters)?));
        }
        if let Some(filter) = raw.not {
            return Ok(Filter::Not(Box::new(Filter::try_from(*filter)?)));
        }

        let field = raw
            .field
            .ok_or_else(|| FilterError::BadShape("is a rule without `field`".to_string()))?;
        let op = raw
            .op
            .ok_or_else(|| FilterError::BadShape("is a rule without `op`".to_string()))?;
        let value = raw.value.map(RawValue::into_text);
        Filter::rule(&field, &op, value.as_deref())
    }
}

impl From<Filter> for RawFilter {
    fn from(filter: Filter) -> Self {
        let convert_all =
            |filters: Vec<Filter>| filters.into_iter().map(RawFilter::from).collect::<Vec<_>>();
        match filter {
            Filter::Rule(rule) => RawFilter {
                field: Some(rule.field.to_string()),
                op: Some(rule.op.to_string()),
                value: rule.value.map(RawValue::Text),
                ..RawFilter::default()
            },
            Filter::All(filters) => RawFilter {
                all: Some(convert_all(filters)),
                ..RawFilter::default()
            },
            Filter::Any(filters) => RawFilter {
                any: Some(convert_all(filters)),
                ..RawFilter::default()
            },
            Filter::Not(filter) => RawFilter {
                not: Some(Box::new(RawFilter::from(*filter))),
                ..RawFilter::default()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use crate::record::Format;

    fn line(level: Option<Level>, message: &str, fields: &[(&str, serde_json::Value)]) -> Record {
        Record {
            cluster: "aks-vulcan-01".into(),
            namespace: "flowcpt".into(),
            pod: "flowcpt-api-7d9f-xk2p1".into(),
            container: Some("api".into()),
            time: Utc.with_ymd_and_hms(2026, 11, 18, 14, 2, 11).unwrap(),
            kind: Kind::Line,
            format: Format::Json,
            level,
            message: message.into(),
            fields: fields
                .iter()
                .map(|(key, value)| (key.to_string(), value.clone()))
                .collect(),
            raw: message.into(),
            previous: false,
            truncated: false,
        }
    }

    fn event(level: Level, reason: &str, message: &str) -> Record {
        Record {
            container: None,
            kind: Kind::Event,
            format: Format::Text,
            fields: BTreeMap::from([("reason".to_string(), json!(reason))]),
            ..line(Some(level), message, &[])
        }
    }

    fn notice(message: &str) -> Record {
        Record::notice(
            "aks-vulcan-01",
            "flowcpt",
            "",
            None,
            Utc.with_ymd_and_hms(2026, 11, 18, 14, 2, 11).unwrap(),
            message,
        )
    }

    fn rule(field: &str, op: &str, value: Option<&str>) -> Filter {
        Filter::rule(field, op, value)
            .unwrap_or_else(|error| panic!("`{field} {op} {value:?}` should be a rule: {error}"))
    }

    // ---- rules ---------------------------------------------------------

    #[test]
    fn level_at_least() {
        let filter = rule("level", ">=", Some("warn"));
        assert!(filter.matches(&line(Some(Level::Warn), "", &[])));
        assert!(filter.matches(&line(Some(Level::Error), "", &[])));
        assert!(!filter.matches(&line(Some(Level::Info), "", &[])));
        assert!(!filter.matches(&line(Some(Level::Trace), "", &[])));
        // A record without a level is not at least anything.
        assert!(!filter.matches(&line(None, "", &[])));
    }

    #[test]
    fn level_values_are_read_in_any_case_and_spelling() {
        for spelling in ["warn", "WARN", "Warning"] {
            let filter = rule("level", ">=", Some(spelling));
            assert!(filter.matches(&line(Some(Level::Warn), "", &[])));
            assert!(!filter.matches(&line(Some(Level::Info), "", &[])));
        }
    }

    #[test]
    fn level_equals_and_differs() {
        let equals = rule("level", "==", Some("info"));
        assert!(equals.matches(&line(Some(Level::Info), "", &[])));
        assert!(!equals.matches(&line(Some(Level::Warn), "", &[])));
        assert!(!equals.matches(&line(None, "", &[])));

        let differs = rule("level", "!=", Some("info"));
        assert!(!differs.matches(&line(Some(Level::Info), "", &[])));
        assert!(differs.matches(&line(Some(Level::Warn), "", &[])));
        assert!(differs.matches(&line(None, "", &[])));
    }

    #[test]
    fn message_rules() {
        let record = line(None, "payment intent expired", &[]);
        assert!(rule("message", "contains", Some("intent")).matches(&record));
        assert!(!rule("message", "contains", Some("Intent")).matches(&record));
        assert!(rule("message", "matches", Some(r"^payment \w+ expired$")).matches(&record));
        assert!(rule("message", "matches", Some("(?i)INTENT")).matches(&record));
        assert!(!rule("message", "matches", Some("^intent")).matches(&record));
        assert!(rule("message", "==", Some("payment intent expired")).matches(&record));
        assert!(rule("message", "!=", Some("something else")).matches(&record));
        assert!(rule("message", "exists", None).matches(&record));
    }

    #[test]
    fn rules_on_where_the_record_came_from() {
        let record = line(None, "", &[]);
        assert!(rule("pod", "==", Some("flowcpt-api-7d9f-xk2p1")).matches(&record));
        assert!(rule("pod", "contains", Some("7d9f")).matches(&record));
        assert!(rule("pod", "matches", Some("^flowcpt-api-")).matches(&record));
        assert!(rule("container", "==", Some("api")).matches(&record));
        assert!(rule("container", "!=", Some("envoy")).matches(&record));
        assert!(rule("namespace", "==", Some("flowcpt")).matches(&record));
        assert!(!rule("namespace", "==", Some("monitoring")).matches(&record));
    }

    #[test]
    fn kind_rules() {
        let is_event = rule("kind", "==", Some("event"));
        assert!(is_event.matches(&event(Level::Info, "Started", "Started container api")));
        assert!(!is_event.matches(&line(None, "", &[])));
        let not_event = rule("kind", "!=", Some("Event"));
        assert!(not_event.matches(&line(None, "", &[])));
        assert!(!not_event.matches(&event(Level::Info, "Started", "")));
        assert!(rule("kind", "exists", None).matches(&line(None, "", &[])));
    }

    #[test]
    fn rules_on_the_fields_of_a_line() {
        let record = line(
            None,
            "",
            &[
                ("tenant_id", json!("t_123")),
                ("status", json!(200)),
                ("cached", json!(true)),
                ("error", json!(null)),
            ],
        );
        assert!(rule("f.tenant_id", "==", Some("t_123")).matches(&record));
        assert!(!rule("f.tenant_id", "==", Some("t_999")).matches(&record));
        assert!(rule("f.tenant_id", "!=", Some("t_999")).matches(&record));
        assert!(rule("f.tenant_id", "contains", Some("123")).matches(&record));
        assert!(rule("f.tenant_id", "matches", Some(r"^t_\d+$")).matches(&record));
        assert!(rule("f.tenant_id", "exists", None).matches(&record));
        // Numbers and booleans compare by their text.
        assert!(rule("f.status", "==", Some("200")).matches(&record));
        assert!(rule("f.status", "matches", Some("^2")).matches(&record));
        assert!(rule("f.cached", "==", Some("true")).matches(&record));
        // A key that is there with a null value exists.
        assert!(rule("f.error", "exists", None).matches(&record));
    }

    #[test]
    fn a_rule_on_a_field_the_record_does_not_have() {
        let record = line(None, "", &[]);
        assert!(!rule("f.tenant_id", "==", Some("t_123")).matches(&record));
        assert!(!rule("f.tenant_id", "contains", Some("t_")).matches(&record));
        assert!(!rule("f.tenant_id", "matches", Some(".*")).matches(&record));
        assert!(!rule("f.tenant_id", "exists", None).matches(&record));
        assert!(!rule("level", "exists", None).matches(&record));
        // `!=` is the opposite of `==`.
        assert!(rule("f.tenant_id", "!=", Some("t_123")).matches(&record));

        let on_deployment = Record {
            container: None,
            ..event(Level::Info, "ScalingReplicaSet", "")
        };
        assert!(!rule("container", "exists", None).matches(&on_deployment));
        assert!(!rule("container", "==", Some("api")).matches(&on_deployment));
        assert!(rule("container", "!=", Some("api")).matches(&on_deployment));
    }

    #[test]
    fn what_is_not_a_rule() {
        assert!(matches!(
            Filter::rule("tenant_id", "==", Some("t_123")),
            Err(FilterError::UnknownField(_))
        ));
        assert!(matches!(
            Filter::rule("f.", "==", Some("x")),
            Err(FilterError::UnknownField(_))
        ));
        assert!(matches!(
            Filter::rule("level", "=", Some("warn")),
            Err(FilterError::UnknownOperator(_))
        ));
        assert!(matches!(
            Filter::rule("level", ">", Some("warn")),
            Err(FilterError::UnknownOperator(_))
        ));
        assert!(matches!(
            Filter::rule("message", ">=", Some("warn")),
            Err(FilterError::OrderOnNonLevel { .. })
        ));
        assert!(matches!(
            Filter::rule("f.status", ">=", Some("500")),
            Err(FilterError::OrderOnNonLevel { .. })
        ));
        assert!(matches!(
            Filter::rule("level", ">=", Some("loud")),
            Err(FilterError::UnknownLevel(_))
        ));
        assert!(matches!(
            Filter::rule("level", "==", Some("loud")),
            Err(FilterError::UnknownLevel(_))
        ));
        assert!(matches!(
            Filter::rule("kind", "==", Some("text")),
            Err(FilterError::UnknownKind(_))
        ));
        assert!(matches!(
            Filter::rule("message", "matches", Some("(unclosed")),
            Err(FilterError::BadRegex { .. })
        ));
        assert!(matches!(
            Filter::rule("message", "contains", None),
            Err(FilterError::MissingValue { .. })
        ));
        assert!(matches!(
            Filter::rule("message", "exists", Some("yes")),
            Err(FilterError::UnexpectedValue)
        ));
    }

    // ---- trees ---------------------------------------------------------

    #[test]
    fn all_any_and_not() {
        let warn = || rule("level", ">=", Some("warn"));
        let tenant = || rule("f.tenant_id", "==", Some("t_123"));
        let loud_tenant = line(Some(Level::Error), "", &[("tenant_id", json!("t_123"))]);
        let quiet_tenant = line(Some(Level::Info), "", &[("tenant_id", json!("t_123"))]);
        let loud_other = line(Some(Level::Error), "", &[("tenant_id", json!("t_999"))]);
        let quiet_other = line(Some(Level::Info), "", &[]);

        let both = Filter::All(vec![warn(), tenant()]);
        assert!(both.matches(&loud_tenant));
        assert!(!both.matches(&quiet_tenant));
        assert!(!both.matches(&loud_other));
        assert!(!both.matches(&quiet_other));

        let either = Filter::Any(vec![warn(), tenant()]);
        assert!(either.matches(&loud_tenant));
        assert!(either.matches(&quiet_tenant));
        assert!(either.matches(&loud_other));
        assert!(!either.matches(&quiet_other));

        let neither = Filter::Not(Box::new(Filter::Any(vec![warn(), tenant()])));
        assert!(!neither.matches(&loud_tenant));
        assert!(neither.matches(&quiet_other));
    }

    #[test]
    fn an_empty_all_holds_and_an_empty_any_does_not() {
        let record = line(None, "", &[]);
        assert!(Filter::All(vec![]).matches(&record));
        assert!(!Filter::Any(vec![]).matches(&record));
    }

    #[test]
    fn notices_always_pass() {
        let nothing = Filter::Any(vec![]);
        assert!(nothing.accepts(&notice("reconnecting")));
        assert!(!nothing.accepts(&line(None, "", &[])));
        // `matches` makes no such exception.
        assert!(!nothing.matches(&notice("reconnecting")));

        let strict = Filter::All(vec![
            rule("level", ">=", Some("error")),
            rule("f.tenant_id", "==", Some("t_123")),
        ]);
        assert!(strict.accepts(&notice("target flowcpt/x matches nothing")));
    }

    #[test]
    fn the_helpers_build_the_rules_the_flags_stand_for() {
        assert_eq!(
            Filter::level_at_least(Level::Warn),
            rule("level", ">=", Some("warn"))
        );
        assert_eq!(
            Filter::message_contains("expired"),
            rule("message", "contains", Some("expired"))
        );
        assert_eq!(
            Filter::message_matches("^pay").unwrap(),
            rule("message", "matches", Some("^pay"))
        );
        assert_eq!(
            Filter::field_equals("tenant_id", "t_123").unwrap(),
            rule("f.tenant_id", "==", Some("t_123"))
        );
        assert_eq!(
            Filter::kind_is(Kind::Event),
            rule("kind", "==", Some("event"))
        );
        assert!(Filter::message_matches("(").is_err());
        assert!(Filter::field_equals("", "x").is_err());
    }

    #[test]
    fn display() {
        let filter = Filter::All(vec![
            rule("level", ">=", Some("info")),
            Filter::Any(vec![
                rule("f.tenant_id", "==", Some("t_123")),
                Filter::Not(Box::new(rule("f.debug", "exists", None))),
            ]),
        ]);
        assert_eq!(
            filter.to_string(),
            "(level >= info and (f.tenant_id == t_123 or not f.debug exists))"
        );
    }

    // ---- the config file's shape ---------------------------------------

    /// A filter written as the config file writes it, under a `filter` key.
    fn from_toml(text: &str) -> Result<Filter, toml::de::Error> {
        #[derive(Deserialize)]
        struct Holder {
            filter: Filter,
        }
        toml::from_str::<Holder>(text).map(|holder| holder.filter)
    }

    #[test]
    fn the_filter_of_the_spec_example() {
        let filter = from_toml(
            r#"
            filter = { all = [
              { field = "level", op = ">=", value = "info" },
              { any = [
                { field = "f.tenant_id", op = "==", value = "t_123" },
                { field = "kind", op = "==", value = "event" },
              ] },
            ] }
            "#,
        )
        .unwrap();
        assert_eq!(
            filter,
            Filter::All(vec![
                rule("level", ">=", Some("info")),
                Filter::Any(vec![
                    rule("f.tenant_id", "==", Some("t_123")),
                    rule("kind", "==", Some("event")),
                ]),
            ])
        );

        // Lines of the tenant at info or above pass, and so do events.
        assert!(filter.accepts(&line(
            Some(Level::Warn),
            "",
            &[("tenant_id", json!("t_123"))]
        )));
        assert!(!filter.accepts(&line(
            Some(Level::Debug),
            "",
            &[("tenant_id", json!("t_123"))]
        )));
        assert!(!filter.accepts(&line(
            Some(Level::Warn),
            "",
            &[("tenant_id", json!("t_999"))]
        )));
        assert!(filter.accepts(&event(Level::Info, "Started", "Started container api")));
        assert!(filter.accepts(&event(Level::Warn, "BackOff", "Back-off restarting")));
    }

    #[test]
    fn a_filter_can_be_one_rule_or_nested_deeply() {
        assert_eq!(
            from_toml(r#"filter = { field = "message", op = "contains", value = "expired" }"#)
                .unwrap(),
            rule("message", "contains", Some("expired"))
        );
        assert_eq!(
            from_toml(r#"filter = { field = "f.debug", op = "exists" }"#).unwrap(),
            rule("f.debug", "exists", None)
        );
        assert_eq!(
            from_toml(
                r#"filter = { not = { any = [ { not = { field = "f.a", op = "exists" } } ] } }"#
            )
            .unwrap(),
            Filter::Not(Box::new(Filter::Any(vec![Filter::Not(Box::new(rule(
                "f.a", "exists", None
            )))])))
        );
    }

    #[test]
    fn a_number_or_a_boolean_as_the_value_is_read_as_its_text() {
        assert_eq!(
            from_toml(r#"filter = { field = "f.status", op = "==", value = 200 }"#).unwrap(),
            rule("f.status", "==", Some("200"))
        );
        assert_eq!(
            from_toml(r#"filter = { field = "f.cached", op = "==", value = true }"#).unwrap(),
            rule("f.cached", "==", Some("true"))
        );
    }

    #[test]
    fn what_is_not_a_filter_in_the_config_file() {
        for (text, expected) in [
            ("filter = {}", "is empty"),
            (
                r#"filter = { field = "level", op = ">=", value = "info", all = [] }"#,
                "mixes several",
            ),
            (r#"filter = { all = [], any = [] }"#, "mixes several"),
            (r#"filter = { op = "exists" }"#, "without `field`"),
            (r#"filter = { field = "level" }"#, "without `op`"),
            (
                r#"filter = { field = "level", op = ">=", value = "loud" }"#,
                "not a level",
            ),
            (
                r#"filter = { any = [ { field = "nope", op = "exists" } ] }"#,
                "unknown field `nope`",
            ),
            (
                r#"filter = { field = "level", operator = ">=", value = "info" }"#,
                "unknown field `operator`",
            ),
        ] {
            let error = from_toml(text).expect_err(text).to_string();
            assert!(error.contains(expected), "`{text}` gave: {error}");
        }
    }

    #[test]
    fn a_filter_survives_a_round_trip_through_toml() {
        #[derive(Serialize)]
        struct Holder {
            filter: Filter,
        }
        let filter = Filter::All(vec![
            rule("level", ">=", Some("info")),
            Filter::Any(vec![
                rule("f.tenant_id", "==", Some("t_123")),
                Filter::Not(Box::new(rule("f.debug", "exists", None))),
            ]),
        ]);
        let text = toml::to_string(&Holder {
            filter: filter.clone(),
        })
        .unwrap();
        assert_eq!(from_toml(&text).unwrap(), filter, "{text}");
    }
}
