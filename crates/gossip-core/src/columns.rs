//! The columns of the prefix of a line: the time, the pod, the container and
//! the level, each with a fixed width and a side to cut a value on that is
//! too long for it.
//!
//! This is what a config file, a template and the `--columns` flag say about
//! the columns, and the widths that follow once they are layered. How a
//! value is cut and padded is a matter of whoever renders the line.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Which side of a value that is too long for its column is cut away. An
/// ellipsis marks the cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cut {
    /// The value is never cut: one that is too long pushes the rest of the
    /// line to the right.
    None,
    /// The start is cut away and the end is kept: `…9c7-vxfgs`.
    Start,
    /// The end is cut away and the start is kept: `fc-api-7f698…`.
    End,
    /// The middle is cut away and both ends are kept: `fc-api…vxfgs`.
    Middle,
}

impl Cut {
    pub fn as_str(self) -> &'static str {
        match self {
            Cut::None => "none",
            Cut::Start => "start",
            Cut::End => "end",
            Cut::Middle => "middle",
        }
    }
}

impl fmt::Display for Cut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Cut {
    type Err = ColumnsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Cut::None),
            "start" => Ok(Cut::Start),
            "end" => Ok(Cut::End),
            "middle" => Ok(Cut::Middle),
            _ => Err(ColumnsError::Cut(s.to_string())),
        }
    }
}

/// One column: how many terminal cells wide it is, and where a value that is
/// too long for it is cut. A width of zero hides the column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    pub width: u16,
    pub cut: Cut,
}

/// The columns of the prefix, once every layer is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Columns {
    /// `HH:MM:SS.mmm`.
    pub time: Column,
    /// The name of the pod, or of the workload an event is about.
    pub pod: Column,
    /// The name of the container; the mark of an event.
    pub container: Column,
    /// `TRACE` to `ERROR`.
    pub level: Column,
}

impl Default for Columns {
    fn default() -> Self {
        Columns {
            time: Column {
                width: 12,
                cut: Cut::None,
            },
            pod: Column {
                width: 20,
                cut: Cut::Middle,
            },
            container: Column {
                width: 10,
                cut: Cut::End,
            },
            level: Column {
                width: 5,
                cut: Cut::End,
            },
        }
    }
}

/// What a config file or a flag says about one column. Either part may be
/// left out, and then comes from the layer below.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnSetting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut: Option<Cut>,
}

impl ColumnSetting {
    pub fn is_empty(&self) -> bool {
        self.width.is_none() && self.cut.is_none()
    }

    fn or(self, lower: ColumnSetting) -> ColumnSetting {
        ColumnSetting {
            width: self.width.or(lower.width),
            cut: self.cut.or(lower.cut),
        }
    }

    fn over(self, column: Column) -> Column {
        Column {
            width: self.width.unwrap_or(column.width),
            cut: self.cut.unwrap_or(column.cut),
        }
    }
}

/// A `columns` table: what a config file, a template or the `--columns` flag
/// says about the columns. Whatever it leaves out comes from the layer
/// below, down to the built-in widths.
///
/// ```toml
/// [columns]
/// pod = { width = 20, cut = "middle" }
/// container = { width = 10, cut = "end" }
/// level = { width = 5 }
/// ```
///
/// On the command line the same reads `pod=20:middle,container=10:end,level=5`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnsSetting {
    #[serde(default, skip_serializing_if = "ColumnSetting::is_empty")]
    pub time: ColumnSetting,
    #[serde(default, skip_serializing_if = "ColumnSetting::is_empty")]
    pub pod: ColumnSetting,
    #[serde(default, skip_serializing_if = "ColumnSetting::is_empty")]
    pub container: ColumnSetting,
    #[serde(default, skip_serializing_if = "ColumnSetting::is_empty")]
    pub level: ColumnSetting,
}

/// The names of the columns, in the order they stand in on a line.
pub const COLUMN_NAMES: [&str; 4] = ["time", "pod", "container", "level"];

impl ColumnsSetting {
    pub fn is_empty(&self) -> bool {
        self.entries().iter().all(|(_, setting)| setting.is_empty())
    }

    /// Fills in whatever this table leaves out from a lower layer, column by
    /// column and part by part: a flag that gives only a width keeps the cut
    /// of the config file.
    pub fn or(self, lower: ColumnsSetting) -> ColumnsSetting {
        ColumnsSetting {
            time: self.time.or(lower.time),
            pod: self.pod.or(lower.pod),
            container: self.container.or(lower.container),
            level: self.level.or(lower.level),
        }
    }

    /// The columns this table stands for when it is laid over others.
    pub fn over(self, columns: Columns) -> Columns {
        Columns {
            time: self.time.over(columns.time),
            pod: self.pod.over(columns.pod),
            container: self.container.over(columns.container),
            level: self.level.over(columns.level),
        }
    }

    /// The columns this table stands for, with the built-in widths for
    /// whatever it leaves out.
    pub fn resolve(self) -> Columns {
        self.over(Columns::default())
    }

    /// The columns by name, in the order they stand in on a line.
    pub fn entries(&self) -> [(&'static str, ColumnSetting); 4] {
        [
            (COLUMN_NAMES[0], self.time),
            (COLUMN_NAMES[1], self.pod),
            (COLUMN_NAMES[2], self.container),
            (COLUMN_NAMES[3], self.level),
        ]
    }

    fn entry_mut(&mut self, name: &str) -> Option<&mut ColumnSetting> {
        match name {
            "time" => Some(&mut self.time),
            "pod" => Some(&mut self.pod),
            "container" => Some(&mut self.container),
            "level" => Some(&mut self.level),
            _ => None,
        }
    }
}

impl From<Columns> for ColumnsSetting {
    /// Every column spelt out.
    fn from(columns: Columns) -> Self {
        let spell = |column: Column| ColumnSetting {
            width: Some(column.width),
            cut: Some(column.cut),
        };
        ColumnsSetting {
            time: spell(columns.time),
            pod: spell(columns.pod),
            container: spell(columns.container),
            level: spell(columns.level),
        }
    }
}

/// A text that does not say what the columns are.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ColumnsError {
    #[error(
        "`{0}` does not set a column; write a column, `=` and a width, as in `pod=24` or `pod=24:middle`"
    )]
    Entry(String),
    #[error("`{0}` is not a column; the columns are time, pod, container and level")]
    Name(String),
    #[error("`{0}` is not a width; a width is a number of terminal cells, and 0 hides the column")]
    Width(String),
    #[error("`{0}` is not a side to cut on; the sides are start, end, middle and none")]
    Cut(String),
}

impl FromStr for ColumnsSetting {
    type Err = ColumnsError;

    /// Reads `pod=24:middle,container=8`: columns separated by commas, each
    /// with a width, a side to cut on after a colon, or both. `pod=:start`
    /// changes the side and leaves the width.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut columns = ColumnsSetting::default();
        for entry in text.split(',') {
            let entry = entry.trim();
            let (name, value) = entry
                .split_once('=')
                .ok_or_else(|| ColumnsError::Entry(entry.to_string()))?;
            let (name, value) = (name.trim(), value.trim());
            let setting = columns
                .entry_mut(name)
                .ok_or_else(|| ColumnsError::Name(name.to_string()))?;
            let (width, cut) = match value.split_once(':') {
                Some((width, cut)) => (width.trim(), Some(cut.trim())),
                None => (value, None),
            };
            if width.is_empty() && cut.is_none() {
                return Err(ColumnsError::Entry(entry.to_string()));
            }
            if !width.is_empty() {
                let cells = width
                    .parse()
                    .map_err(|_| ColumnsError::Width(width.to_string()))?;
                setting.width = Some(cells);
            }
            if let Some(cut) = cut {
                setting.cut = Some(cut.parse()?);
            }
        }
        Ok(columns)
    }
}

impl fmt::Display for ColumnsSetting {
    /// The way [`FromStr`] reads it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for (name, setting) in self.entries() {
            if setting.is_empty() {
                continue;
            }
            if !first {
                f.write_str(",")?;
            }
            first = false;
            write!(f, "{name}=")?;
            if let Some(width) = setting.width {
                write!(f, "{width}")?;
            }
            if let Some(cut) = setting.cut {
                write!(f, ":{cut}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(width: Option<u16>, cut: Option<Cut>) -> ColumnSetting {
        ColumnSetting { width, cut }
    }

    #[test]
    fn the_built_in_columns() {
        let columns = Columns::default();
        assert_eq!((columns.time.width, columns.time.cut), (12, Cut::None));
        assert_eq!((columns.pod.width, columns.pod.cut), (20, Cut::Middle));
        assert_eq!(
            (columns.container.width, columns.container.cut),
            (10, Cut::End)
        );
        assert_eq!((columns.level.width, columns.level.cut), (5, Cut::End));
        assert_eq!(ColumnsSetting::default().resolve(), columns);
    }

    #[test]
    fn the_flag_reads_a_width_a_side_or_both() {
        let columns: ColumnsSetting = "pod=24:middle,container=8".parse().unwrap();
        assert_eq!(columns.pod, setting(Some(24), Some(Cut::Middle)));
        assert_eq!(columns.container, setting(Some(8), None));
        assert!(columns.time.is_empty() && columns.level.is_empty());

        let columns: ColumnsSetting = " time = 0 , pod = :start , level=3:none ".parse().unwrap();
        assert_eq!(columns.time, setting(Some(0), None));
        assert_eq!(columns.pod, setting(None, Some(Cut::Start)));
        assert_eq!(columns.level, setting(Some(3), Some(Cut::None)));

        // The same column twice: the later one adds to the earlier.
        let columns: ColumnsSetting = "pod=30,pod=:end".parse().unwrap();
        assert_eq!(columns.pod, setting(Some(30), Some(Cut::End)));
    }

    #[test]
    fn what_the_flag_cannot_read_is_an_error_that_says_why() {
        let error = |text: &str| text.parse::<ColumnsSetting>().unwrap_err();
        assert_eq!(error("pod"), ColumnsError::Entry("pod".into()));
        assert_eq!(error(""), ColumnsError::Entry("".into()));
        assert_eq!(error("pod=24,"), ColumnsError::Entry("".into()));
        assert_eq!(error("pod="), ColumnsError::Entry("pod=".into()));
        assert_eq!(error("node=24"), ColumnsError::Name("node".into()));
        assert_eq!(error("pod=wide"), ColumnsError::Width("wide".into()));
        assert_eq!(error("pod=-1"), ColumnsError::Width("-1".into()));
        assert_eq!(error("pod=24:left"), ColumnsError::Cut("left".into()));
        assert_eq!(error("pod=24:"), ColumnsError::Cut("".into()));
        assert!(
            error("node=24")
                .to_string()
                .contains("time, pod, container and level")
        );
    }

    #[test]
    fn a_table_is_written_the_way_the_flag_reads_it() {
        for text in [
            "pod=24:middle,container=8",
            "time=0",
            "pod=:start",
            "time=12:none,pod=20:middle,container=10:end,level=5:end",
        ] {
            let columns: ColumnsSetting = text.parse().unwrap();
            assert_eq!(columns.to_string(), text);
        }
        assert_eq!(ColumnsSetting::default().to_string(), "");
        assert_eq!(
            ColumnsSetting::from(Columns::default()).to_string(),
            "time=12:none,pod=20:middle,container=10:end,level=5:end"
        );
    }

    #[test]
    fn a_layer_fills_in_from_the_one_below_part_by_part() {
        let flag: ColumnsSetting = "pod=24,level=0".parse().unwrap();
        let file: ColumnsSetting = "pod=30:start,container=8:start".parse().unwrap();
        let layered = flag.or(file);
        // The width of the flag, the side of the file.
        assert_eq!(layered.pod, setting(Some(24), Some(Cut::Start)));
        assert_eq!(layered.container, setting(Some(8), Some(Cut::Start)));
        assert_eq!(layered.level, setting(Some(0), None));

        let columns = layered.resolve();
        assert_eq!((columns.pod.width, columns.pod.cut), (24, Cut::Start));
        assert_eq!(
            (columns.container.width, columns.container.cut),
            (8, Cut::Start)
        );
        // What no layer names is the built-in column.
        assert_eq!((columns.level.width, columns.level.cut), (0, Cut::End));
        assert_eq!(columns.time, Columns::default().time);
    }

    #[test]
    fn a_table_laid_over_columns_changes_only_what_it_names() {
        let wide = "pod=40:end".parse::<ColumnsSetting>().unwrap().resolve();
        let narrower: ColumnsSetting = "pod=30,container=0".parse().unwrap();
        let columns = narrower.over(wide);
        assert_eq!((columns.pod.width, columns.pod.cut), (30, Cut::End));
        assert_eq!(columns.container.width, 0);
        assert_eq!(columns.level, Columns::default().level);
    }
}
