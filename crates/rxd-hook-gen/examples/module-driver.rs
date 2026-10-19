//! Fresh-process public generation/argument driver, not the product CLI.
//! RXD_HOOK_TEST_SPECS supplies explicitly prepared resources; never changes env.
#[path = "../../../src/cli/hook_gen.rs"]
mod hook_gen;

use clap::Parser;

#[derive(Parser)]
struct Driver {
    #[command(subcommand)]
    command: hook_gen::Command,
}

fn main() {
    let driver = Driver::parse();
    let result = hook_gen::run(
        driver.command,
        || Ok(std::env::var_os("RXD_HOOK_TEST_SPECS").map(Into::into)),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    if let Err(error) = result {
        // Even diagnostics are outside successful source stdout.
        eprintln!("{error}");
        std::process::exit(error.exit_code());
    }
}
