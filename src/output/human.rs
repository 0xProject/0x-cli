use crate::output::HumanDisplay;
use colored::Colorize;
use comfy_table::{presets::UTF8_FULL_CONDENSED, Cell, ContentArrangement, Table};
use std::io::{self, Write};

/// A key-value display for structured data (like a price quote).
pub struct KeyValueTable {
    pub title: String,
    pub rows: Vec<(String, String)>,
    pub footer: Option<String>,
}

impl HumanDisplay for KeyValueTable {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL_CONDENSED)
            .set_content_arrangement(ContentArrangement::Dynamic);

        if color {
            writeln!(writer, "\n  {}", self.title.bold())?;
        } else {
            writeln!(writer, "\n  {}", self.title)?;
        }

        for (key, value) in &self.rows {
            if color {
                table.add_row(vec![
                    Cell::new(key).fg(comfy_table::Color::Cyan),
                    Cell::new(value),
                ]);
            } else {
                table.add_row(vec![Cell::new(key), Cell::new(value)]);
            }
        }

        writeln!(writer, "{table}")?;

        if let Some(ref footer) = self.footer {
            if color {
                writeln!(writer, "  {}", footer.yellow())?;
            } else {
                writeln!(writer, "  {}", footer)?;
            }
        }

        Ok(())
    }
}

/// A multi-row table for displaying lists (like quotes, chains).
pub struct DataTable {
    pub title: Option<String>,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl HumanDisplay for DataTable {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        if let Some(ref title) = self.title {
            if color {
                writeln!(writer, "\n  {}", title.bold())?;
            } else {
                writeln!(writer, "\n  {}", title)?;
            }
        }

        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL_CONDENSED)
            .set_content_arrangement(ContentArrangement::Dynamic);

        let header_cells: Vec<Cell> = self
            .headers
            .iter()
            .map(|h| {
                if color {
                    Cell::new(h).fg(comfy_table::Color::Cyan)
                } else {
                    Cell::new(h)
                }
            })
            .collect();
        table.set_header(header_cells);

        for row in &self.rows {
            table.add_row(row.iter().map(Cell::new).collect::<Vec<_>>());
        }

        writeln!(writer, "{table}")?;
        Ok(())
    }
}
