use std::io::{self, BufRead};

use anyhow::Result;
use qnx_gdb_mi::parse_record;

fn main() -> Result<()> {
    for line in io::stdin().lock().lines() {
        let line = line?;
        let record = parse_record(&line)?;

        println!("{record:#?}");
    }

    Ok(())
}
