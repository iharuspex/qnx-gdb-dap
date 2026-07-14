use std::io::{self, BufReader};

use anyhow::Result;
use qnx_dap::DapReader;
use serde_json::Value;

fn main() -> Result<()> {
    let stdin = io::stdin();
    let mut reader = DapReader::new(BufReader::new(stdin.lock()));

    while let Some(message) = reader.read_message::<Value>()? {
        eprintln!("{message:#}");
    }

    Ok(())
}
