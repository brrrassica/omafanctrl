//! `omafanctrl-probe` — read-only EC inspection tool.
//!
//! Dumps the 256-byte EC register window and decodes the known fan and
//! temperature registers. Writes are only performed when explicitly requested
//! with `--force`.
//!
//! The probe also reads the realtime fan speed from `/proc/acpi/ibm/fan`
//! (`thinkpad_acpi`) and reports it alongside the EC tachometer, so the two
//! independent views can be cross-checked.
//!
//! The `--sweep` mode automates the observation that produced
//! `config/E14G4-quirks`: it steps the fan through every manual level, records
//! the resulting RPM from both sources, and compares it against the static curve
//! in [`omafanctrl_core::fan_curve`]. It always restores BIOS auto control when
//! it finishes.

use std::ops::{Deref, DerefMut};
use std::process::ExitCode;
use std::time::Duration;

use omafanctrl_core::ec::{
    AccessMode, Ec, EcBackend, EcError, FAN_LEVEL_MAX, FAN_LEVEL_MIN, FileBackend,
};
use omafanctrl_core::fan_curve;
use omafanctrl_core::probe;
use omafanctrl_core::thinkpad_acpi;

/// Default settle time per sweep step, in milliseconds.
const DEFAULT_SETTLE_MS: u64 = 3000;

const USAGE: &str = "\
omafanctrl-probe — inspect the Embedded Controller

USAGE:
    omafanctrl-probe [OPTIONS]

OPTIONS:
    --json              Emit machine-readable JSON
    --set-level <N>     Write manual fan level N (1-7); requires --force
    --auto              Write BIOS auto (0x80); requires --force
    --disengage         Write disengaged/advanced (0x40); requires --force
    --sweep             Sweep levels 1-7 and record RPM; requires --force
    --settle <MS>       Settle time per sweep step in ms (default 3000)
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
    /// Write disengaged/advanced.
    Disengage,
    /// Sweep every manual level and record the resulting RPM.
    Sweep,
}

/// Parsed command-line arguments.
struct Args {
    json: bool,
    force: bool,
    settle: Duration,
    action: Action,
}

fn parse_args() -> Result<Args, String> {
    let mut json = false;
    let mut force = false;
    let mut settle = Duration::from_millis(DEFAULT_SETTLE_MS);
    let mut action = Action::Dump;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--force" => force = true,
            "--auto" => action = Action::Auto,
            "--disengage" => action = Action::Disengage,
            "--sweep" => action = Action::Sweep,
            "--settle" => {
                let value = args.next().ok_or("--settle requires a value")?;
                let millis: u64 = value
                    .parse()
                    .map_err(|_| format!("invalid settle time `{value}`"))?;
                settle = Duration::from_millis(millis);
            }
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
        settle,
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
        Action::Sweep => return run_sweep(ec, args.settle, args.json),
        Action::Dump => {}
        Action::SetLevel(level) => ec.set_fan_level(level)?,
        Action::Auto => ec.set_fan_auto()?,
        Action::Disengage => ec.set_fan_disengaged()?,
    }

    let report = probe::probe_with_acpi(ec, read_acpi_fan())?;
    if args.json {
        print!("{}", probe::format_json(&report));
    } else {
        print!("{}", probe::format_report(&report));
    }
    Ok(())
}

/// Best-effort read of `/proc/acpi/ibm/fan`.
///
/// A missing or unreadable interface is not fatal: the EC tachometer is still
/// reported. The reason is printed to stderr so the user knows why the ACPI
/// section is absent.
fn read_acpi_fan() -> Option<thinkpad_acpi::FanStatus> {
    match thinkpad_acpi::read_fan_status() {
        Ok(status) => Some(status),
        Err(error) => {
            eprintln!("note: {error}");
            None
        }
    }
}

/// One step of a fan-level sweep.
struct SweepPoint {
    /// The EC fan-control level that was applied.
    level: u8,
    /// The fan speed read from the EC tachometer registers.
    ec_rpm: u16,
    /// The realtime fan speed from `/proc/acpi/ibm/fan`, when available.
    acpi_rpm: Option<u16>,
}

/// Tracks an interrupt request without doing EC I/O from the signal handler.
mod interrupt {
    use std::sync::atomic::{AtomicBool, Ordering};

    static INTERRUPTED: AtomicBool = AtomicBool::new(false);

    #[cfg(unix)]
    const SIGINT: i32 = 2;
    #[cfg(unix)]
    const SIGTERM: i32 = 15;

    #[cfg(unix)]
    unsafe extern "C" {
        fn signal(signum: i32, handler: extern "C" fn(i32)) -> *mut core::ffi::c_void;
    }

    #[cfg(unix)]
    extern "C" fn handle_signal(_signum: i32) {
        INTERRUPTED.store(true, Ordering::SeqCst);
    }

    /// Best-effort installation of handlers that request a clean sweep abort.
    pub fn install() {
        #[cfg(unix)]
        unsafe {
            signal(SIGINT, handle_signal);
            signal(SIGTERM, handle_signal);
        }
    }

    /// Whether a signal has requested an abort since the process started.
    pub fn interrupted() -> bool {
        INTERRUPTED.load(Ordering::SeqCst)
    }
}

/// Sleep for `duration`, returning early when an interrupt is requested.
fn sleep_interruptible(duration: Duration) -> bool {
    const CHECK_INTERVAL: Duration = Duration::from_millis(100);
    let mut remaining = duration;
    while !remaining.is_zero() {
        if interrupt::interrupted() {
            return true;
        }
        let chunk = remaining.min(CHECK_INTERVAL);
        std::thread::sleep(chunk);
        remaining = remaining.saturating_sub(chunk);
    }
    interrupt::interrupted()
}

/// Restores BIOS auto control when dropped, unless it was already restored.
struct FanAutoGuard<'a, B: EcBackend> {
    ec: &'a mut Ec<B>,
    armed: bool,
}

impl<'a, B: EcBackend> FanAutoGuard<'a, B> {
    fn new(ec: &'a mut Ec<B>) -> Self {
        Self { ec, armed: true }
    }

    fn restore(&mut self) -> Result<(), EcError> {
        let result = self.ec.set_fan_auto();
        if result.is_ok() {
            self.armed = false;
        }
        result
    }
}

impl<B: EcBackend> Deref for FanAutoGuard<'_, B> {
    type Target = Ec<B>;

    fn deref(&self) -> &Self::Target {
        self.ec
    }
}

impl<B: EcBackend> DerefMut for FanAutoGuard<'_, B> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ec
    }
}

impl<B: EcBackend> Drop for FanAutoGuard<'_, B> {
    fn drop(&mut self) {
        if self.armed {
            // Best effort: cannot report an error from `drop`.
            let _ = self.ec.set_fan_auto();
        }
    }
}

/// Step the fan through every manual level, recording the observed RPM.
///
/// The fan is always handed back to the firmware (BIOS auto) before returning,
/// even if a step fails or the sweep is interrupted, so a sweep can never leave
/// the machine on a manual level.
fn run_sweep<B: EcBackend>(ec: &mut Ec<B>, settle: Duration, json: bool) -> Result<(), EcError> {
    let mut guard = FanAutoGuard::new(ec);
    interrupt::install();
    let mut points: Vec<SweepPoint> = Vec::new();
    let mut result = Ok(());
    for level in FAN_LEVEL_MIN..=FAN_LEVEL_MAX {
        if interrupt::interrupted() {
            eprintln!("sweep: interrupted; restoring BIOS auto control");
            break;
        }
        eprintln!("sweep: level {level}, settling for {settle:?}...");
        if let Err(error) = guard.set_fan_level(level) {
            result = Err(error);
            break;
        }
        if sleep_interruptible(settle) {
            eprintln!("sweep: interrupted; restoring BIOS auto control");
            break;
        }
        match guard.read_fan_rpm() {
            Ok(ec_rpm) => {
                let acpi_rpm = thinkpad_acpi::read_fan_speed().ok();
                points.push(SweepPoint {
                    level,
                    ec_rpm,
                    acpi_rpm,
                });
            }
            Err(error) => {
                result = Err(error);
                break;
            }
        }
    }

    // Safety: always restore BIOS auto control, regardless of the outcome.
    if let Err(error) = guard.restore() {
        eprintln!("error: failed to restore BIOS auto control: {error}");
        if result.is_ok() {
            result = Err(error);
        }
    } else {
        eprintln!("sweep: restored BIOS auto control");
    }

    if json {
        print!("{}", format_sweep_json(&points));
    } else {
        print!("{}", format_sweep(&points));
    }
    result
}

/// Render a sweep result as a human-readable table, comparing the EC tachometer
/// against the static observed curve.
fn format_sweep(points: &[SweepPoint]) -> String {
    let mut out = String::new();
    out.push_str("Fan level sweep (level -> EC RPM / ACPI RPM, expected RPM):\n");
    for point in points {
        let expected = fan_curve::rpm_for_level(point.level);
        let marker = match expected {
            Some(expected) if expected == point.ec_rpm => "ok",
            Some(_) => "differs",
            None => "n/a",
        };
        let acpi = point
            .acpi_rpm
            .map_or_else(|| "?".to_string(), |rpm| rpm.to_string());
        out.push_str(&format!(
            "  level {} -> EC {} RPM / ACPI {} RPM (expected {}) [{}]\n",
            point.level,
            point.ec_rpm,
            acpi,
            expected.map_or_else(|| "?".to_string(), |value| value.to_string()),
            marker
        ));
    }
    out.push_str(&fan_curve::format_curve());
    out
}

/// Render a sweep result as JSON.
fn format_sweep_json(points: &[SweepPoint]) -> String {
    let mut out = String::new();
    out.push_str("{\n  \"sweep\": [");
    for (index, point) in points.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        let acpi = point
            .acpi_rpm
            .map_or_else(|| "null".to_string(), |rpm| rpm.to_string());
        out.push_str(&format!(
            "{{\"level\": {}, \"ec_rpm\": {}, \"acpi_rpm\": {}}}",
            point.level, point.ec_rpm, acpi
        ));
    }
    out.push_str("]\n}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use omafanctrl_core::ec::{EC_WINDOW_SIZE, FAN_BIOS_AUTO, REG_FAN_CONTROL};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct SharedBackend {
        bytes: Arc<Mutex<[u8; EC_WINDOW_SIZE]>>,
    }

    impl SharedBackend {
        fn new() -> (Self, Arc<Mutex<[u8; EC_WINDOW_SIZE]>>) {
            let bytes = Arc::new(Mutex::new([0u8; EC_WINDOW_SIZE]));
            (
                Self {
                    bytes: Arc::clone(&bytes),
                },
                bytes,
            )
        }
    }

    impl EcBackend for SharedBackend {
        fn read_byte(&mut self, offset: usize) -> Result<u8, EcError> {
            Ok(self.bytes.lock().unwrap()[offset])
        }

        fn write_byte(&mut self, offset: usize, value: u8) -> Result<(), EcError> {
            self.bytes.lock().unwrap()[offset] = value;
            Ok(())
        }

        fn read_window(&mut self) -> Result<[u8; EC_WINDOW_SIZE], EcError> {
            Ok(*self.bytes.lock().unwrap())
        }
    }

    #[test]
    fn sweep_restores_bios_auto() {
        let (backend, bytes) = SharedBackend::new();
        let mut ec = Ec::new(backend).with_min_write_interval(Duration::ZERO);
        run_sweep(&mut ec, Duration::ZERO, true).unwrap();
        assert_eq!(
            bytes.lock().unwrap()[usize::from(REG_FAN_CONTROL)],
            FAN_BIOS_AUTO
        );
    }

    #[test]
    fn guard_restores_bios_auto_when_dropped() {
        let (backend, bytes) = SharedBackend::new();
        let mut ec = Ec::new(backend).with_min_write_interval(Duration::ZERO);
        {
            let mut guard = FanAutoGuard::new(&mut ec);
            guard.set_fan_level(3).unwrap();
            assert_eq!(bytes.lock().unwrap()[usize::from(REG_FAN_CONTROL)], 3);
        }
        assert_eq!(
            bytes.lock().unwrap()[usize::from(REG_FAN_CONTROL)],
            FAN_BIOS_AUTO
        );
    }
}
