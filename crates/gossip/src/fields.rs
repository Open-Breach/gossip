//! Which fields of a line the text output shows, and in which order.
//!
//! Without a list of its own a line shows the fields a request is read by
//! first, always in the same order, then every other field in alphabetical
//! order, and leaves out the ones that repeat something or are noise on
//! every line. A list of names replaces all of that.

use std::collections::BTreeMap;

use gossip_core::parser::{TARGET_FIELD, TIMESTAMP_FIELD};

/// What the text output does with a field it knows by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// Shown before every other field, in the order of the table.
    First,
    /// Left out unless it is asked for.
    Hidden,
}

/// The fields the text output knows by name.
///
/// THIS TABLE IS THE ONLY PLACE TO CHANGE which fields come first, in which
/// order they come, and which fields are hidden. Nothing else in the code
/// names a field for the sake of the output. Spec 5.3 and the README repeat
/// the table for the reader, and want the same change.
///
/// The fields marked `First` are shown in the order they stand in here.
pub const KNOWN_FIELDS: &[(&str, Place)] = &[
    // What a request is read by.
    ("http.method", Place::First),
    ("http.route", Place::First),
    ("http.status_code", Place::First),
    ("tenant_id", Place::First),
    ("user_id", Place::First),
    ("request_id", Place::First),
    ("time.busy", Place::First),
    // The time is on the record already, and where in the code a line was
    // logged is rarely what one reads for.
    (TIMESTAMP_FIELD, Place::Hidden),
    (TARGET_FIELD, Place::Hidden),
    // These repeat the route or the status, or are noise on every line.
    ("otel.name", Place::Hidden),
    ("otel.kind", Place::Hidden),
    ("otel.status_code", Place::Hidden),
    ("http.target", Place::Hidden),
    ("http.scheme", Place::Hidden),
    ("http.flavor", Place::Hidden),
    ("http.host", Place::Hidden),
    ("http.user_agent", Place::Hidden),
    ("http.client_ip", Place::Hidden),
    ("time.idle", Place::Hidden),
    ("name", Place::Hidden),
    ("spans", Place::Hidden),
];

/// The word that asks for every field in place of a list of names.
pub const ALL: &str = "all";

/// Which fields of a line are shown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FieldChoice {
    /// The known fields first, the rest in alphabetical order, the hidden
    /// ones left out.
    #[default]
    Default,
    /// Every field, the hidden ones too, in the same order.
    All,
    /// These and no others, in this order; none of them for an empty list.
    Named(Vec<String>),
}

impl FieldChoice {
    /// The choice a `--fields` flag or the `fields` of a template stands
    /// for: no list is the default, the one word `all` is every field, and
    /// any other list is the fields it names.
    pub fn from_setting(setting: Option<&[String]>) -> FieldChoice {
        match setting {
            None => FieldChoice::Default,
            Some([only]) if only == ALL => FieldChoice::All,
            Some(names) => FieldChoice::Named(names.to_vec()),
        }
    }

    /// The setting that stands for this choice: what `--fields` or the
    /// `fields` of a template would say. None for the default.
    pub fn to_setting(&self) -> Option<Vec<String>> {
        match self {
            FieldChoice::Default => None,
            FieldChoice::All => Some(vec![ALL.to_string()]),
            FieldChoice::Named(names) => Some(names.clone()),
        }
    }

    /// The fields of a line to show, in the order to show them in.
    pub fn of<'a, V>(&self, fields: &'a BTreeMap<String, V>) -> Vec<(&'a String, &'a V)> {
        match self {
            FieldChoice::Default => in_default_order(fields, false),
            FieldChoice::All => in_default_order(fields, true),
            FieldChoice::Named(names) => names
                .iter()
                .filter_map(|name| fields.get_key_value(name))
                .collect(),
        }
    }
}

/// The place of a field the table knows.
fn place(key: &str) -> Option<Place> {
    KNOWN_FIELDS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, place)| *place)
}

/// The fields that come first in the order of the table, each only when the
/// line has it, then every other field in alphabetical order.
fn in_default_order<V>(fields: &BTreeMap<String, V>, hidden_too: bool) -> Vec<(&String, &V)> {
    let first = KNOWN_FIELDS
        .iter()
        .filter(|(_, place)| *place == Place::First)
        .filter_map(|(name, _)| fields.get_key_value(*name));
    let mut rest: Vec<(&String, &V)> = fields
        .iter()
        .filter(|(key, _)| match place(key) {
            Some(Place::First) => false,
            Some(Place::Hidden) => hidden_too,
            None => true,
        })
        .collect();
    // As a reader sorts, without regard to case. The map hands the fields
    // out in the order of their bytes, which is what breaks a tie.
    rest.sort_by_cached_key(|(key, _)| key.to_lowercase());
    first.chain(rest).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// A line with these fields; the value of each is its own name.
    fn line(keys: &[&str]) -> BTreeMap<String, String> {
        keys.iter()
            .map(|key| (key.to_string(), key.to_string()))
            .collect()
    }

    fn shown(choice: &FieldChoice, keys: &[&str]) -> Vec<String> {
        choice
            .of(&line(keys))
            .into_iter()
            .map(|(key, _)| key.clone())
            .collect()
    }

    fn names(names: &[&str]) -> FieldChoice {
        FieldChoice::Named(names.iter().map(|name| name.to_string()).collect())
    }

    #[test]
    fn the_known_fields_come_first_in_a_fixed_order_each_only_when_present() {
        // However the line has them, and with one of them missing.
        assert_eq!(
            shown(
                &FieldChoice::Default,
                &[
                    "time.busy",
                    "request_id",
                    "user_id",
                    "tenant_id",
                    "http.status_code",
                    "http.route",
                    "http.method",
                ]
            ),
            [
                "http.method",
                "http.route",
                "http.status_code",
                "tenant_id",
                "user_id",
                "request_id",
                "time.busy",
            ]
        );
        assert_eq!(
            shown(&FieldChoice::Default, &["request_id", "http.method"]),
            ["http.method", "request_id"]
        );
        assert!(shown(&FieldChoice::Default, &[]).is_empty());
    }

    #[test]
    fn every_other_field_follows_in_alphabetical_order() {
        assert_eq!(
            shown(
                &FieldChoice::Default,
                &[
                    "version",
                    "rows",
                    "tenant_id",
                    "attempt",
                    "http.method",
                    "db.table"
                ]
            ),
            [
                "http.method",
                "tenant_id",
                "attempt",
                "db.table",
                "rows",
                "version"
            ]
        );
        // As a reader sorts, not as the bytes do.
        assert_eq!(
            shown(&FieldChoice::Default, &["Zone", "apple", "Apple", "mango"]),
            ["Apple", "apple", "mango", "Zone"]
        );
    }

    #[test]
    fn the_hidden_fields_are_left_out() {
        let hidden = [
            "timestamp",
            "target",
            "otel.name",
            "otel.kind",
            "otel.status_code",
            "http.target",
            "http.scheme",
            "http.flavor",
            "http.host",
            "http.user_agent",
            "http.client_ip",
            "time.idle",
            "name",
            "spans",
        ];
        assert!(shown(&FieldChoice::Default, &hidden).is_empty());

        let mut keys = hidden.to_vec();
        keys.extend(["rows", "http.route"]);
        assert_eq!(shown(&FieldChoice::Default, &keys), ["http.route", "rows"]);
    }

    #[test]
    fn a_list_of_names_replaces_all_of_it() {
        let keys = ["http.method", "tenant_id", "rows", "name", "timestamp"];
        // In the order asked, a hidden field like any other, and a field
        // the line does not have left out.
        assert_eq!(
            shown(
                &names(&["timestamp", "rows", "missing", "name", "http.method"]),
                &keys
            ),
            ["timestamp", "rows", "name", "http.method"]
        );
        // An empty list is no field at all.
        assert!(shown(&names(&[]), &keys).is_empty());
    }

    #[test]
    fn all_shows_the_hidden_fields_too_in_the_default_order() {
        assert_eq!(
            shown(
                &FieldChoice::All,
                &[
                    "timestamp",
                    "target",
                    "time.idle",
                    "time.busy",
                    "otel.name",
                    "name",
                    "rows",
                    "tenant_id",
                    "http.method",
                ]
            ),
            [
                "http.method",
                "tenant_id",
                "time.busy",
                "name",
                "otel.name",
                "rows",
                "target",
                "time.idle",
                "timestamp",
            ]
        );
    }

    #[test]
    fn the_one_word_all_is_every_field_and_any_other_list_is_names() {
        let list =
            |names: &[&str]| -> Vec<String> { names.iter().map(|name| name.to_string()).collect() };
        assert_eq!(FieldChoice::from_setting(None), FieldChoice::Default);
        assert_eq!(
            FieldChoice::from_setting(Some(&list(&["all"]))),
            FieldChoice::All
        );
        assert_eq!(
            FieldChoice::from_setting(Some(&list(&[]))),
            FieldChoice::Named(vec![])
        );
        assert_eq!(
            FieldChoice::from_setting(Some(&list(&["rows", "tenant_id"]))),
            names(&["rows", "tenant_id"])
        );
        // Beside another name, `all` is the name of a field.
        assert_eq!(
            FieldChoice::from_setting(Some(&list(&["all", "rows"]))),
            names(&["all", "rows"])
        );
        assert_eq!(
            FieldChoice::from_setting(Some(&list(&["ALL"]))),
            names(&["ALL"])
        );
    }

    #[test]
    fn a_choice_is_written_as_the_setting_it_was_read_from() {
        let list =
            |names: &[&str]| -> Vec<String> { names.iter().map(|name| name.to_string()).collect() };
        for setting in [
            None,
            Some(list(&["all"])),
            Some(list(&[])),
            Some(list(&["rows", "tenant_id"])),
        ] {
            let choice = FieldChoice::from_setting(setting.as_deref());
            assert_eq!(choice.to_setting(), setting);
        }
    }

    #[test]
    fn the_table_names_no_field_twice() {
        let names: BTreeSet<&str> = KNOWN_FIELDS.iter().map(|(name, _)| *name).collect();
        assert_eq!(names.len(), KNOWN_FIELDS.len());
    }
}
