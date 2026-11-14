//! Save check: does `file.save` to a `.xlsm` path preserve the macro project?
//! (The repro's saveBytes path uses format "xlsx" and drops VBA by design.)
//! Structure only — no cell contents are printed.
//! Usage: cargo run --release -p gridcraft-engine --example save_check -- <path.xlsm> <out.xlsm>

use std::time::Instant;

use gridcraft_core::Value;
use gridcraft_engine::Session;
use serde_json::json;

fn main() {
    let path = std::env::args().nth(1).expect("path to .xlsm");
    let out = std::env::args().nth(2).expect("output .xlsm path");
    assert!(out.ends_with(".xlsm"), "output must end in .xlsm");
    let bytes = std::fs::read(&path).unwrap();

    // Open via the real (fixed) path.
    let t0 = Instant::now();
    let mut s = Session::new();
    s.execute("file.open", json!({"path": path})).unwrap();
    println!("open: {:?} vba_project={}", t0.elapsed(), s.doc().unwrap().wb.vba_project.is_some());

    // Edit one cell in column Y (col 24) of sheet 0: first text cell -> a sentinel number.
    let mut target: Option<(u32, String)> = None;
    {
        let d = s.doc().unwrap();
        let sh0 = d.wb.sheet(0).unwrap();
        for (c, cell) in sh0.cells.iter() {
            if c.col == 24 && matches!(&cell.value, Value::Text(_)) {
                target = Some((c.row, format!("Y{}", c.row + 1)));
                break;
            }
        }
    }
    let (y_row, y_addr) = target.expect("no text cell in column Y of sheet 0");
    let sentinel = 123456789.0f64;
    let t0 = Instant::now();
    s.execute("cell.set", json!({"cell": y_addr, "input": sentinel, "sheet": 0})).unwrap();
    println!("edit {} -> sentinel: {:?}", y_addr, t0.elapsed());

    // Save via file.save to a .xlsm path (NOT the original file).
    let t0 = Instant::now();
    s.execute("file.save", json!({"path": out})).unwrap();
    println!("file.save -> {}: {:?}", out, t0.elapsed());
    let saved = std::fs::read(&out).unwrap();
    println!("saved_bytes={} (original {})", saved.len(), bytes.len());

    // Re-parse the saved file and compare structure + the edit + VBA.
    let (wb2, _) = gridcraft_xlsx::read_xlsx(&saved).unwrap();
    let cells: usize = wb2.sheets.iter().map(|s| s.cells.iter().count()).sum();
    let formulas: usize = wb2
        .sheets
        .iter()
        .map(|s| s.cells.iter().filter(|(_, c)| c.formula.is_some()).count())
        .sum();
    let edited = wb2
        .sheet(0)
        .unwrap()
        .cells
        .get(gridcraft_core::CellRef::new(y_row, 24))
        .map(|c| c.value == Value::Number(sentinel))
        .unwrap_or(false);
    println!(
        "re-parsed: sheets={} cells={} formulas={} vba_project={} edited_is_sentinel={}",
        wb2.sheets.len(),
        cells,
        formulas,
        wb2.vba_project.is_some(),
        edited
    );
    // Also confirm the vbaProject.bin part is present in the zip (its name is stored
    // in plaintext in the zip headers).
    let vba_in_zip = saved.windows(10).any(|w| w == b"vbaProject");
    println!("xl/vbaProject.bin present in saved zip: {}", vba_in_zip);
    println!("done");
}
