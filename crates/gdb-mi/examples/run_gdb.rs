use std::env;

use anyhow::{Context, Result, bail};
use qnx_gdb_mi::{GdbEvent, GdbProcess, GdbProcessConfig, MiRecord, commands};

use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_ansi(true)
        .init();

    let gdb_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "ntoarm-gdb".to_owned());

    let config = GdbProcessConfig::new(&gdb_path);
    let mut gdb =
        GdbProcess::spawn(&config).with_context(|| format!("failed to start {gdb_path}"))?;

    eprintln!("started GDB with PID {}", gdb.id());

    let startup_records = gdb.synchronize()?;

    for record in startup_records {
        match record {
            MiRecord::ConsoleStream(text) => print!("{text}"),
            MiRecord::LogStream(text) => eprint!("{text}"),
            MiRecord::TargetStream(text) => print!("{text}"),
            other => println!("{other:#?}"),
        }
    }

    let result = gdb.execute(commands::gdb_version)?;

    println!("result class: {}", result.result.class);

    for event in &result.events {
        match event {
            GdbEvent::Stream(MiRecord::ConsoleStream(text)) => {
                print!("{text}");
            }
            GdbEvent::Stream(MiRecord::LogStream(text)) => {
                eprint!("{text}");
            }
            GdbEvent::Stream(MiRecord::TargetStream(text)) => {
                print!("{text}");
            }
            GdbEvent::Stream(record) | GdbEvent::Async(record) => {
                println!("{record:#?}");
            }
        }
    }

    if !result.is_class("done") {
        bail!(
            "gdb-version returned unexpected result class {:?}",
            result.result.class
        );
    }

    let status = gdb.shutdown()?;
    println!("GDB exited with status: {status}");

    Ok(())
}
