//! Synthetic ~135k-row workbook built from code (never the user's file): verifies the
//! open–edit–save path at the user's scale — a COUNTIF column over a 135k-row criteria
//! range, plus styled empty cells — then re-opens the saved file and checks the edit
//! and the COUNTIF value survived.
//!
//! Usage: cargo run --release -p gridcraft-engine --example synthetic_huge

use std::collections::HashMap;
use std::time::Instant;

use gridcraft_core::{CellRef, Value};
use gridcraft_engine::Session;
use gridcraft_model::{Cell, Formula, Style, Workbook};
use serde_json::json;

const ROWS: u32 = 135_000;
const CRIT: [&str; 5] = ["A", "B", "C", "D", "E"];
const COL_Y: u32 = 24; // criteria range column
const COL_AD: u32 = 29; // per-row criterion
const COL_AG: u32 = 32; // COUNTIF column

fn rss_kib() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let mut rss = 0u64;
    let mut peak = 0u64;
    for line in s.lines() {
        if let Some(v) = line.strip_prefix("VmRSS:") {
            rss = v.trim().trim_end_matches(" kB").parse().unwrap_or(0);
        }
        if let Some(v) = line.strip_prefix("VmHWM:") {
            peak = v.trim().trim_end_matches(" kB").parse().unwrap_or(0);
        }
    }
    (rss, peak)
}

fn col_letters(mut col: u32) -> String {
    let mut s = String::new();
    loop {
        s.push((b'A' + (col % 26) as u8) as char);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    s.chars().rev().collect()
}

fn a1(row: u32, col: u32) -> String {
    format!("{}{}", col_letters(col), row + 1)
}

fn main() {
    let scratch = std::env::var("HOME").unwrap() + "/workspace/.scratch";
    let path = format!("{scratch}/synthetic_huge.xlsx");
    let saved_path = format!("{scratch}/synthetic_huge_saved.xlsx");

    // 1. Build the workbook from the model API.
    let t0 = Instant::now();
    let mut wb = Workbook::new();
    let mut st = Style::default();
    st.font.bold = true;
    let sid = wb.styles.intern(st);
    let mut counts: HashMap<&str, usize> = HashMap::new();
    {
        let sh = wb.sheet_mut(0).unwrap();
        sh.set_cell(CellRef::new(0, 0), Cell::value(Value::text("No")));
        sh.set_cell(CellRef::new(0, COL_Y), Cell::value(Value::text("Kriteria")));
        sh.set_cell(CellRef::new(0, COL_AD), Cell::value(Value::text("Cari")));
        sh.set_cell(CellRef::new(0, COL_AG), Cell::value(Value::text("Hitung")));
        for r in 1..ROWS {
            let key = CRIT[(r as usize - 1) % CRIT.len()];
            *counts.entry(key).or_insert(0) += 1;
            sh.set_cell(CellRef::new(r, 0), Cell::value(Value::Number(r as f64)));
            sh.set_cell(CellRef::new(r, COL_Y), Cell::value(Value::text(key)));
            sh.set_cell(CellRef::new(r, COL_AD), Cell::value(Value::text(key)));
            let f = Formula::new(&format!("=COUNTIF($Y$2:$Y${ROWS},{}{})", col_letters(COL_AD), r + 1));
            sh.set_cell(CellRef::new(r, COL_AG), Cell::formula(f));
            // Styled empty cells (a visible format on an empty cell, as in the user's file).
            if r < 40_000 {
                for c in 1..6u32 {
                    sh.set_cell(CellRef::new(r, c), Cell { value: Value::Empty, formula: None, style: sid });
                }
            }
        }
    }
    let cells: usize = wb.sheets.iter().map(|s| s.cells.iter().count()).sum();
    let formulas: usize = wb
        .sheets
        .iter()
        .map(|s| s.cells.iter().filter(|(_, c)| c.formula.is_some()).count())
        .sum();
    println!("build: {:?} cells={} formulas={} rss={:?} KiB", t0.elapsed(), cells, formulas, rss_kib());
    let expected_a = counts["A"] as f64;
    println!("expected COUNTIF for 'A' = {expected_a} (counts: {counts:?})");

    // 2. Populate cached values (as a file saved by Excel would have).
    let t0 = Instant::now();
    let mut calc = gridcraft_engine::calc::recalc::Calc::new();
    calc.recalc_all(&mut wb);
    println!("recalc_all (populate cache): {:?} last_recalc_cells={}", t0.elapsed(), calc.last_recalc_cells);
    let ag2 = wb.sheet(0).unwrap().value(CellRef::new(1, COL_AG));
    println!("AG2 after recalc = {ag2:?} (expected Number({expected_a}))");
    assert_eq!(ag2, Value::Number(expected_a));

    // 3. Write the file.
    let t0 = Instant::now();
    let bytes = gridcraft_xlsx::write_xlsx(&wb).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    println!("write_xlsx: {:?} bytes={} rss={:?} KiB", t0.elapsed(), bytes.len(), rss_kib());
    drop(wb);

    // 4. Open through the real Session path (file.open).
    let t0 = Instant::now();
    let mut s = Session::new();
    s.execute("file.open", json!({"path": path})).unwrap();
    println!("stage file.open: {:?} rss={:?} KiB", t0.elapsed(), rss_kib());
    let d = s.doc().unwrap();
    let sh0 = d.wb.sheet(0).unwrap();
    let open_cells: usize = sh0.cells.iter().count();
    let open_formulas: usize = sh0.cells.iter().filter(|(_, c)| c.formula.is_some()).count();
    let empty_formulas: usize = sh0.cells.iter().filter(|(_, c)| c.formula.is_some() && c.value == Value::Empty).count();
    let ag2_open = sh0.value(CellRef::new(1, COL_AG));
    println!(
        "open sheets={} cells={} formulas={} empty_cached_formulas={} AG2={ag2_open:?}",
        d.wb.sheets.len(),
        open_cells,
        open_formulas,
        empty_formulas
    );
    assert_eq!(open_cells, cells);
    assert_eq!(open_formulas, formulas);
    assert_eq!(ag2_open, Value::Number(expected_a));

    // 5. Edit a cell inside the COUNTIF criteria range; a dependent COUNTIF must drop by 1.
    let y_addr = a1(1, COL_Y);
    let t0 = Instant::now();
    s.execute("cell.set", json!({"cell": y_addr, "input": "Z", "sheet": 0})).unwrap();
    let edit_elapsed = t0.elapsed();
    let d = s.doc().unwrap();
    let ag2_after = d.wb.sheet(0).unwrap().value(CellRef::new(1, COL_AG));
    let y2_after = d.wb.sheet(0).unwrap().value(CellRef::new(1, COL_Y));
    println!(
        "edit {y_addr} 'A' -> 'Z': {edit_elapsed:?} Y2={y2_after:?} AG2={ag2_after:?} expected Number({}) delta_ok={}",
        expected_a - 1.0,
        ag2_after == Value::Number(expected_a - 1.0)
    );
    assert_eq!(y2_after, Value::text("Z"));
    assert_eq!(ag2_after, Value::Number(expected_a - 1.0));

    // 6. Save and re-parse; the edit and the COUNTIF value must survive.
    let t0 = Instant::now();
    s.execute("file.save", json!({"path": saved_path})).unwrap();
    let saved_bytes = std::fs::metadata(&saved_path).unwrap().len();
    println!("file.save: {:?} bytes={saved_bytes} rss={:?} KiB", t0.elapsed(), rss_kib());
    let t0 = Instant::now();
    let (wb2, warnings) = gridcraft_xlsx::read_xlsx(&std::fs::read(&saved_path).unwrap()).unwrap();
    let sh2 = wb2.sheet(0).unwrap();
    let y2_saved = sh2.value(CellRef::new(1, COL_Y));
    let ag2_saved = sh2.value(CellRef::new(1, COL_AG));
    let saved_cells: usize = sh2.cells.iter().count();
    let saved_formulas: usize = sh2.cells.iter().filter(|(_, c)| c.formula.is_some()).count();
    println!(
        "re-parse saved: {:?} warnings={} cells={saved_cells} formulas={saved_formulas} Y2={y2_saved:?} AG2={ag2_saved:?}",
        t0.elapsed(),
        warnings.warnings.len()
    );
    assert_eq!(y2_saved, Value::text("Z"));
    assert_eq!(ag2_saved, Value::Number(expected_a - 1.0));
    assert_eq!(saved_cells, cells);
    assert_eq!(saved_formulas, formulas);
    println!("SYNTHETIC OK: open–edit–save round-trip verified at 135k rows");
}
