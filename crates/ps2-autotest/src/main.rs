//! Headless CLI runner for `ps2autotests` `.elf` and `.irx` test suites.

use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "ps2-autotest", about = "Headless PS2 autotest runner")]
struct Args {
    /// Path to a single `.elf` or `.irx` test binary.
    #[arg(long)]
    test: Option<PathBuf>,

    /// Path to the `ps2autotests` root or `tests/` directory.
    #[arg(long, default_value = "local/ps2autotests/tests")]
    suite_dir: PathBuf,

    /// Optional path to a 4 MB PS2 BIOS dump directory or file.
    #[arg(long, default_value = "local/bios")]
    bios: PathBuf,

    /// Maximum EE cycles to execute per test before timing out.
    #[arg(long, default_value_t = 50_000_000)]
    max_cycles: u64,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    tracing::info!(
        test = ?args.test,
        suite_dir = %args.suite_dir.display(),
        bios = %args.bios.display(),
        max_cycles = args.max_cycles,
        "ps2-autotest initialized (skeleton)"
    );
}
