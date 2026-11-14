//! Repro for the huge .xlsm open ANR: times each stage of opening the user's file
//! (structure only — no cell contents are printed), then exercises the fixed open path,
//! an edit inside a COUNTIF range, and a save round-trip.
//!
//! Usage: cargo run --release -p gridcraft-engine --example repro_huge -- <path.xlsm>

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use gridcraft_core::Value;
use gridcraft_engine::Session;
use serde_json::json;

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

/// Parse a simple A1-style cell ref like "AD37" (no $) into (row, col), 0-based.
fn parse_a1(t: &str) -> Option<(u32, u32)> {
    let t = t.trim().trim_matches('$');
    let split = t.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = t.split_at(split);
    if letters.is_empty() || digits.is_empty() {
        return None;
    }
    let mut col: u32 = 0;
    for b in letters.bytes() {
        if !b.is_ascii_alphabetic() {
            return None;
        }
        col = col * 26 + (b.to_ascii_uppercase() - b'A') as u32 + 1;
    }
    let row: u32 = digits.parse().ok()?;
    Some((row - 1, col - 1))
}

fn main() {
    let path = std::env::args().nth(1).expect("path to .xlsm");
    let bytes = std::fs::read(&path).unwrap();
    println!("read {} bytes from disk", bytes.len());
    println!("rss_after_read: {:?} KiB", rss_kib());

    // Stage A: parse only.
    let t0 = Instant::now();
    let (mut wb, warnings) = gridcraft_xlsx::read_xlsx(&bytes).unwrap();
    let cells: usize = wb.sheets.iter().map(|s| s.cells.iter().count()).sum();
    let formulas: usize = wb
        .sheets
        .iter()
        .map(|s| s.cells.iter().filter(|(_, c)| c.formula.is_some()).count())
        .sum();
    println!(
        "stage A read_xlsx: {:?} sheets={} cells={} formulas={} warnings={}",
        t0.elapsed(),
        wb.sheets.len(),
        cells,
        formulas,
        warnings.warnings.len()
    );
    println!("rss_after_parse: {:?} KiB", rss_kib());

    // Stage B: full open_bytes (parse + fit rows), as before.
    let t0 = Instant::now();
    let (_wb2, warnings) = gridcraft_engine::io::open_bytes(&path, &bytes).unwrap();
    println!("stage B open_bytes (parse + fit rows): {:?} warnings={}", t0.elapsed(), warnings.len());
    println!("rss_after_open_bytes: {:?} KiB", rss_kib());

    // Stage C (baseline, old behaviour): full recalc after parse.
    let t0 = Instant::now();
    let mut calc = gridcraft_engine::calc::recalc::Calc::new();
    calc.recalc_all(&mut wb);
    println!("stage C recalc_all (OLD open path): {:?} last_recalc_cells={}", t0.elapsed(), calc.last_recalc_cells);
    println!("rss_after_recalc: {:?} KiB", rss_kib());
    drop(wb);

    // Stage D: the real open path with the fix (Session + file.open).
    let t0 = Instant::now();
    let mut s = Session::new();
    s.execute("file.open", json!({"path": path})).unwrap();
    println!("stage D file.open (FIXED path): {:?} rss={:?} KiB", t0.elapsed(), rss_kib());
    let d = s.doc().unwrap();
    println!(
        "open sheets={} active_sheet={} vba_project={}",
        d.wb.sheets.len(),
        d.wb.active_sheet,
        d.wb.vba_project.is_some()
    );
    // How many formula cells ended up without a cached value (should be ~0: nothing to evaluate)?
    let empty_formulas: usize = d
        .wb
        .sheets
        .iter()
        .map(|sh| sh.cells.iter().filter(|(_, c)| c.formula.is_some() && c.value == Value::Empty).count())
        .sum();
    println!("open formula_cells_with_empty_cached_value={}", empty_formulas);

    // Stage E: edit a cell inside a COUNTIF criteria range and check a dependent COUNTIF updates.
    // Sheet 0 holds COUNTIF($Y$2:$Y$133080, AD<row>) formulas; Y is column 24.
    let sh0 = d.wb.sheet(0).unwrap();
    // Build a frequency map of column Y (numbers by bits, texts by string) + first row per key.
    let mut y_num: HashMap<u64, (usize, u32)> = HashMap::new();
    let mut y_txt: HashMap<String, (usize, u32)> = HashMap::new();
    for (c, cell) in sh0.cells.iter() {
        if c.col != 24 {
            continue;
        }
        match &cell.value {
            Value::Number(n) => {
                let e = y_num.entry(n.to_bits()).or_insert((0, c.row));
                e.0 += 1;
            }
            Value::Text(t) => {
                let e = y_txt.entry(t.to_string()).or_insert((0, c.row));
                e.0 += 1;
            }
            _ => {}
        }
    }
    println!("stage E prep: distinct Y numbers={} distinct Y texts={}", y_num.len(), y_txt.len());
    // Find the first COUNTIF formula cell whose criteria value occurs in column Y.
    let mut target: Option<(u32, u32, f64, u32)> = None; // (frow, fcol, count_before, y_row)
    for (c, cell) in sh0.cells.iter() {
        let Some(f) = &cell.formula else { continue };
        if !f.text.starts_with("COUNTIF(") {
            continue;
        }
        // Text looks like COUNTIF($Y$2:$Y$133080,AD37): criteria ref is after the comma.
        let Some(comma) = f.text.find(',') else { continue };
        let crit_ref = f.text[comma + 1..].trim_end_matches(')');
        let Some((crow, ccol)) = parse_a1(crit_ref) else { continue };
        let Some(crit_cell) = sh0.cells.get(gridcraft_core::CellRef::new(crow, ccol)) else { continue };
        let crit_val = crit_cell.value.clone();
        let hit = match &crit_val {
            Value::Number(n) => y_num.get(&n.to_bits()).map(|(n, r)| (*n, *r)),
            Value::Text(t) => y_txt.get(t.as_ref()).map(|(n, r)| (*n, *r)),
            _ => None,
        };
        if let Some((n, y_row)) = hit
            && n >= 1
            && let Value::Number(v) = cell.value
        {
            target = Some((c.row, c.col, v, y_row));
            break;
        }
    }
    let (frow, fcol, count_before, y_row) = target.expect("no COUNTIF with a matching Y cell found");
    println!("stage E target: COUNTIF at {} count_before={} (criteria value present in Y)", a1(frow, fcol), count_before);
    // Sentinel: a number that does not occur in column Y.
    let mut sentinel = 123456789.0f64;
    while y_num.contains_key(&sentinel.to_bits()) {
        sentinel += 1.0;
    }
    let _ = HashSet::<u64>::new();
    let y_addr = a1(y_row, 24);
    let t0 = Instant::now();
    s.execute("cell.set", json!({"cell": y_addr, "input": sentinel, "sheet": 0})).unwrap();
    let edit_elapsed = t0.elapsed();
    let d = s.doc().unwrap();
    let count_after = match d.wb.sheet(0).unwrap().cells.get(gridcraft_core::CellRef::new(frow, fcol)) {
        Some(cell) => match cell.value {
            Value::Number(n) => n,
            _ => f64::NAN,
        },
        None => f64::NAN,
    };
    println!(
        "stage E edit {} (was a counted criteria value) -> sentinel: {:?} count_after={} expected={} delta_ok={}",
        y_addr,
        edit_elapsed,
        count_after,
        count_before - 1.0,
        (count_after - (count_before - 1.0)).abs() < 0.5
    );
    println!("rss_after_edit: {:?} KiB", rss_kib());

    // Stage F: save round-trip — saveBytes, re-parse, compare structure + the edited values.
    let t0 = Instant::now();
    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let b64 = saved.get("base64").and_then(|v| v.as_str()).expect("saveBytes base64");
    let saved_bytes = gridcraft_engine::io::base64_decode(b64).expect("base64 decode");
    println!("stage F saveBytes: {:?} saved_bytes={} rss={:?} KiB", t0.elapsed(), saved_bytes.len(), rss_kib());
    let (wb_saved, _) = gridcraft_xlsx::read_xlsx(&saved_bytes).unwrap();
    let saved_cells: usize = wb_saved.sheets.iter().map(|x| x.cells.iter().count()).sum();
    let saved_formulas: usize =
        wb_saved.sheets.iter().map(|x| x.cells.iter().filter(|(_, c)| c.formula.is_some()).count()).sum();
    let saved_y = wb_saved
        .sheet(0)
        .unwrap()
        .cells
        .get(gridcraft_core::CellRef::new(y_row, 24))
        .map(|c| c.value.clone());
    let saved_f = wb_saved
        .sheet(0)
        .unwrap()
        .cells
        .get(gridcraft_core::CellRef::new(frow, fcol))
        .map(|c| c.value.clone());
    println!(
        "stage F re-parsed saved file: sheets={} cells={} (was {}) formulas={} (was {}) vba_project={} edited_Y_is_sentinel={} countif_value_matches_post_edit={}",
        wb_saved.sheets.len(),
        saved_cells,
        cells,
        saved_formulas,
        formulas,
        wb_saved.vba_project.is_some(),
        matches!(saved_y, Some(Value::Number(n)) if (n - sentinel).abs() < 0.5),
        matches!(saved_f, Some(Value::Number(n)) if (n - count_after).abs() < 0.5),
    );
    println!("rss_end: {:?} KiB", rss_kib());
    println!("done");
}
