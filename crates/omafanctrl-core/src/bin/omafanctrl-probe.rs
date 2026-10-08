//! `omafanctrl-probe` — read-only EC inspection tool.
//!
//! Dumps the 256-byte EC register window and decodes the known fan and
//! temperature registers. Writes are only performed when explicitly requested
//! with `--force`.

use std::process::ExitCode;

use omafanctrl_core::ec::{AccessMode, Ec, EcBackend, EcError, FileBackend};
use omafanctrl_core::probe;

const USAGE: &str = "\
omafanctrl-probe — inspect the Embedded Controller

USAGE:
    omafanctrl-probe [OPTIONS]

OPTIONS:
    --json              Emit machine-readable JSON
    --set-level <N>     Write manual fan level N (1-7); requires --force
    --auto              Write BIOS auto (0x00); requires --force
    --disengage         Write disengaged/max (0x80); requires --force
    --force             Confirm a write action
    -h, --help          Print this help

Without a write option the probe is strictly read-only.
";

/// The action the probe should perform.
enum Action {
    /// Dump the EC without writing anything.
    Dump,
    /// Write a manual fan level.
    SetLevel(u8),
    /// Write BIOS auto.
    Auto,
    /// Write disengaged/max.
    Disengage,
}

/// Parsed command-line arguments.
struct Args {
    json: bool,
    force: bool,
    action: Action,
}

fn parse_args() -> Result<Args, String> {
    let mut json = false;
    let mut force = false;
    let mut action = Action::Dump;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--force" => force = true,
            "--auto" => action = Action::Auto,
            "--disengage" => action = Action::Disengage,
            "--set-level" => {
                let value = args.next().ok_or("--set-level requires a value")?;
                let level: u8 = value
                    .parse()
                    .map_err(|_| format!("invalid fan level `{value}`"))?;
                action = Action::SetLevel(level);
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(Args {
        json,
        force,
        action,
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("error: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let is_write = !matches!(args.action, Action::Dump);
    if is_write && !args.force {
        eprintln!(
            "error: refusing to write to the EC without --force.\n\
             Writing the wrong register can harm the machine; see plans/risks.md (R1)."
        );
        return ExitCode::FAILURE;
    }

    let mode = if is_write {
        AccessMode::ReadWrite
    } else {
        AccessMode::ReadOnly
    };
    let backend = match FileBackend::open(mode) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut ec = Ec::new(backend);

    if let Err(error) = run(&mut ec, &args) {
        eprintln!("error: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run<B: EcBackend>(ec: &mut Ec<B>, args: &Args) -> Result<(), EcError> {
    match args.action {
        Action::Dump => {}
        Action::SetLevel(level) => ec.set_fan_level(level)?,
        Action::Auto => ec.set_fan_auto()?,
        Action::Disengage => ec.set_fan_disengaged()?,
    }

    let report = probe::probe(ec)?;
    if args.json {
        print!("{}", probe::format_json(&report));
    } else {
        print!("{}", probe::format_report(&report));
    }
    Ok(())
}
