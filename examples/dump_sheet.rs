//! Dump a single sheet fully for debugging the importer.
//! Run with: `cargo run --example dump_sheet -- <xlsx> "<sheet name>"`

use calamine::{Data, Reader, open_workbook_auto};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap();
    let name = args.next().unwrap();
    let mut wb = open_workbook_auto(&path)?;
    let range = wb.worksheet_range(&name)?;
    let (rows, cols) = range.get_size();
    println!("{name}: {rows} rows x {cols} cols");
    for (r, row) in range.rows().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            if !matches!(cell, Data::Empty) {
                println!("  [{r},{c}] {:?}", cell);
            }
        }
    }
    Ok(())
}
