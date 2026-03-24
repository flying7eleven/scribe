use clap::CommandFactory;
use clap_complete::{generate, Shell};
use std::io;

/// Generate and print shell completion script to stdout.
pub fn run(shell: Shell) {
    let mut cmd = crate::Cli::command();
    generate(shell, &mut cmd, "scribe", &mut io::stdout());
}
