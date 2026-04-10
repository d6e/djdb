//! One-shot exploration of the source xlsx so we can plan the importer.
//! Run with: `cargo run --example explore_xlsx -- data/kaleidosky.xlsx`

use calamine::{Data, Reader, open_workbook_auto};

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "data/kaleidosky.xlsx".to_string());
    let mut wb = open_workbook_auto(&path)?;

    let names: Vec<String> = wb.sheet_names().to_vec();
    println!("Total sheets: {}", names.len());
    println!();

    // Print all sheet names.
    for (i, name) in names.iter().enumerate() {
        println!("  [{i}] {name}");
    }
    println!();

    // Dump full contents of the first 3 sheets and the last 2.
    let sample: Vec<usize> = {
        let mut s: Vec<usize> = (0..names.len().min(3)).collect();
        if names.len() > 5 {
            s.push(names.len() - 2);
            s.push(names.len() - 1);
        }
        s
    };

    for idx in sample {
        let name = &names[idx];
        println!("=== Sheet [{idx}] {name} ===");
        let range = wb.worksheet_range(name)?;
        let (rows, cols) = range.get_size();
        println!("  size: {rows} rows x {cols} cols");
        for (r, row) in range.rows().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                if !matches!(cell, Data::Empty) {
                    println!("    [{r},{c}] {:?}", cell);
                }
            }
        }
        println!();
    }

    Ok(())
}
