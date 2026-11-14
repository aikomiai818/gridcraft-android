//! Probe: how many stored cells are empty-but-styled, and what do those styles carry?
//! Structure only (style ids + counts); no cell contents are printed.
//! Usage: cargo run --release -p gridcraft-engine --example style_probe -- <path.xlsm>

use std::collections::HashMap;

use gridcraft_engine::core::Value;
use gridcraft_engine::model::style::{Style, StyleId};

fn main() {
    let path = std::env::args().nth(1).expect("path to .xlsm");
    let bytes = std::fs::read(&path).unwrap();
    let (wb, _) = gridcraft_xlsx::read_xlsx(&bytes).unwrap();
    println!("styles table entries: {}", wb.styles.len());
    let def = Style::default();
    for (si, sh) in wb.sheets.iter().enumerate() {
        let mut total = 0usize;
        let mut with_value = 0usize;
        let mut formulas = 0usize;
        let mut empty_styled: HashMap<u32, usize> = HashMap::new();
        let mut empty_default = 0usize;
        for (_c, cell) in sh.cells.iter() {
            total += 1;
            if cell.formula.is_some() {
                formulas += 1;
            }
            if cell.value != Value::Empty {
                with_value += 1;
            } else if cell.formula.is_none() {
                if cell.style == StyleId::DEFAULT {
                    empty_default += 1;
                } else {
                    *empty_styled.entry(cell.style.0).or_insert(0) += 1;
                }
            }
        }
        let empty_styled_total: usize = empty_styled.values().sum();
        println!(
            "sheet {} {:?}: total={} with_value={} formulas={} empty_styled={} (distinct_styles={}) empty_default={}",
            si,
            sh.name,
            total,
            with_value,
            formulas,
            empty_styled_total,
            empty_styled.len(),
            empty_default
        );
        // What do the empty cells' styles carry? Compare each distinct style to default.
        let mut style_ids: Vec<u32> = empty_styled.keys().copied().collect();
        style_ids.sort();
        for id in style_ids {
            let st = wb.styles.get(StyleId(id));
            let diffs: Vec<&str> = [
                ("font", st.font != def.font),
                ("fill", st.fill != def.fill),
                ("borders", st.borders != def.borders),
                ("align", st.align != def.align),
                ("num_fmt", st.num_fmt != def.num_fmt),
                ("protection", st.protection != def.protection),
            ]
            .iter()
            .filter(|(_, d)| *d)
            .map(|(n, _)| *n)
            .collect();
            println!(
                "  style {}: empty_cells={} differs_from_default_in={:?}",
                id,
                empty_styled[&id],
                diffs
            );
        }
        // Do the empty cells' styles duplicate their row/column style? If so, the per-cell
        // entry is redundant: style_id() falls back to the row/column style, and the writer
        // re-emits the row's s="..." attribute.
        let mut row_match = 0usize;
        let mut col_match = 0usize;
        let mut neither = 0usize;
        for (c, cell) in sh.cells.iter() {
            if cell.value == Value::Empty && cell.formula.is_none() && cell.style != StyleId::DEFAULT {
                let rs = sh.rows.get(&c.row).and_then(|i| i.style);
                let cs = sh.cols.get(&c.col).and_then(|i| i.style);
                if rs == Some(cell.style) {
                    row_match += 1;
                } else if cs == Some(cell.style) {
                    col_match += 1;
                } else {
                    neither += 1;
                }
            }
        }
        println!(
            "  empty_styled style source: row_style_match={} col_style_match={} neither={}",
            row_match, col_match, neither
        );
    }
    println!("done");
}
