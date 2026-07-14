use anyhow::Result;
use qnx_gdb_mi::{MiTokenGenerator, commands};

fn main() -> Result<()> {
    let mut tokens = MiTokenGenerator::new();

    let commands = [
        commands::gdb_version(tokens.next_token()),
        commands::file_exec_and_symbols(
            tokens.next_token(),
            "/home/user/project/build/application",
        ),
        commands::target_select_qnx(tokens.next_token(), "192.168.1.20:8000"),
        commands::break_insert(tokens.next_token(), "/home/user/project/src/main.cpp:42"),
    ];

    for command in commands {
        println!("{}", command.encode()?);
    }

    Ok(())
}
