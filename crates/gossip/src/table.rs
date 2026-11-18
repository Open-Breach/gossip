//! A plain table in the style of `kubectl`: left-aligned columns, three
//! spaces apart, headers in capitals.

/// A table of text cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    headers: Vec<&'static str>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(headers: &[&'static str]) -> Self {
        Table {
            headers: headers.to_vec(),
            rows: Vec::new(),
        }
    }

    /// Adds a row; it must have one cell per header.
    pub fn row(&mut self, cells: Vec<String>) {
        debug_assert_eq!(cells.len(), self.headers.len());
        self.rows.push(cells);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The table as text, one line per row, each ending in a newline.
    pub fn render(&self) -> String {
        let mut widths: Vec<usize> = self
            .headers
            .iter()
            .map(|header| header.chars().count())
            .collect();
        for row in &self.rows {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.chars().count());
            }
        }

        let mut text = String::new();
        let mut line = |cells: &mut dyn Iterator<Item = &str>| {
            let mut rendered = String::new();
            for (cell, width) in cells.zip(&widths) {
                rendered.push_str(cell);
                let padding = width - cell.chars().count() + 3;
                rendered.extend(std::iter::repeat_n(' ', padding));
            }
            text.push_str(rendered.trim_end());
            text.push('\n');
        };
        line(&mut self.headers.iter().copied());
        for row in &self.rows {
            line(&mut row.iter().map(String::as_str));
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_as_wide_as_their_widest_cell() {
        let mut table = Table::new(&["NAME", "READY", "AGE"]);
        table.row(vec!["flowcpt-api".into(), "2/2".into(), "5d".into()]);
        table.row(vec!["redis".into(), "1/1".into(), "12d".into()]);
        assert_eq!(
            table.render(),
            "NAME          READY   AGE\n\
             flowcpt-api   2/2     5d\n\
             redis         1/1     12d\n"
        );
    }

    #[test]
    fn no_line_ends_in_spaces() {
        let mut table = Table::new(&["CURRENT", "NAME", "NAMESPACE"]);
        table.row(vec!["*".into(), "aks-vulcan-01".into(), String::new()]);
        table.row(vec![String::new(), "kind".into(), "default".into()]);
        assert_eq!(
            table.render(),
            "CURRENT   NAME            NAMESPACE\n\
             *         aks-vulcan-01\n\
             \u{20}         kind            default\n"
        );
    }

    #[test]
    fn a_table_without_rows_is_only_its_headers() {
        let table = Table::new(&["NAME", "AGE"]);
        assert!(table.is_empty());
        assert_eq!(table.render(), "NAME   AGE\n");
    }
}
