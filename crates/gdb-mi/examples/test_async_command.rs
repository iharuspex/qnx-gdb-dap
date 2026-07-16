use std::env;

use anyhow::Result;
use qnx_gdb_mi::{GdbExecutionEvent, GdbProcess, GdbProcessConfig, commands};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_ansi(false)
        .init();

    let gdb_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "ntoarm-gdb".to_owned());

    let config = GdbProcessConfig::new(gdb_path);
    let mut gdb = GdbProcess::spawn(&config)?;

    gdb.synchronize()?;

    let started = gdb.start_execution(commands::exec_next)?;

    println!(
        "initial result: token={} class={}",
        started.token, started.result.class
    );

    for _ in 0..4 {
        let Some(event) = gdb.next_execution_event()? else {
            break;
        };

        println!("event: {event:#?}");

        if matches!(
            event,
            GdbExecutionEvent::Result(ref result)
                if result.class == "error"
        ) {
            break;
        }
    }

    gdb.shutdown()?;

    Ok(())
}
