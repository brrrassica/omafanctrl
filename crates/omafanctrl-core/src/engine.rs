//! Fan control engine.
//!
//! Implements the three TPFanCtrl2 control modes:
//!
//! - **BIOS** — the firmware manages the fan.
//! - **Manual** — a fixed fan level is applied.
//! - **Smart** — the maximum non-ignored sensor temperature is mapped to a fan
//!   level, with optional hysteresis to avoid oscillation.
//!
//! The engine is deliberately **pure**: [`Engine::evaluate`] takes a slice of
//! [`SensorReading`]s and the current [`Instant`], and returns a [`Decision`].
//! It performs no I/O, which makes it trivial to unit-test with synthetic sensor
//! traces. The daemon applies the decision to the EC via [`apply_decision`].
//!
//! # Safety
//!
//! Two independent safety layers protect the hardware:
//!
//! - The engine clamps every commanded level to the configured
//!   [`EnginePolicy::safety_floor`]..=[`EnginePolicy::safety_ceiling`] range and
//!   forces the ceiling when the maximum temperature reaches
//!   [`EnginePolicy::emergency_temperature`].
//! - The [`Watchdog`] reverts the fan to BIOS auto control when it is dropped,
//!   which covers normal shutdown, `kill -9` (via the daemon's `ExecStopPost`),
//!   and panics (via [`run_guarded`] and the `Drop` implementation).

use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use crate::config::{Config, SmartLevel};
use crate::ec::{Ec, EcBackend, EcError, FAN_LEVEL_MAX, TEMP_MAX_C};

/// The default minimum time between two fan-level changes.
pub const DEFAULT_MIN_DWELL: Duration = Duration::from_secs(5);

/// The default lowest fan level the engine will command.
pub const DEFAULT_SAFETY_FLOOR: u8 = 1;

/// The default highest fan level the engine will command.
pub const DEFAULT_SAFETY_CEILING: u8 = FAN_LEVEL_MAX;

/// The default temperature (°C) at which the engine forces the safety ceiling.
pub const DEFAULT_EMERGENCY_TEMPERATURE: u8 = 85;

/// The active fan control mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The firmware manages the fan.
    Bios,
    /// A fixed manual fan level is applied.
    Manual,
    /// The fan level is derived from sensor temperatures.
    Smart,
}

impl Mode {
    /// The next mode in the BIOS → Manual → Smart → BIOS cycle.
    pub fn next(self) -> Self {
        match self {
            Mode::Bios => Mode::Manual,
            Mode::Manual => Mode::Smart,
            Mode::Smart => Mode::Bios,
        }
    }

    /// A short, stable identifier suitable for the CLI and D-Bus.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Bios => "bios",
            Mode::Manual => "manual",
            Mode::Smart => "smart",
        }
    }
}

/// A named temperature reading from a single sensor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorReading {
    /// The EC register offset the reading came from.
    pub offset: u8,
    /// The sensor name, used for the ignore list and display.
    pub name: String,
    /// The temperature in degrees Celsius.
    pub celsius: u8,
}

impl SensorReading {
    /// Create a reading.
    pub fn new(offset: u8, name: impl Into<String>, celsius: u8) -> Self {
        Self {
            offset,
            name: name.into(),
            celsius,
        }
    }
}

/// The default name of a known E14 Gen 4 sensor offset.
///
/// The mapping mirrors the verified profile in `data/profiles/e14-gen4.ini`.
/// Unknown offsets return `None`; callers should fall back to a hex label.
pub fn default_sensor_name(offset: u8) -> Option<&'static str> {
    match offset {
        0x78 => Some("cpu"),
        0x79 => Some("gpu"),
        0x7D => Some("no5"),
        0xC0 => Some("pwr"),
        0xC1 => Some("aps"),
        0xC2 => Some("bus"),
        0xC3 => Some("pci"),
        _ => None,
    }
}

/// Read a set of sensor offsets into named [`SensorReading`]s.
///
/// Unknown offsets are labelled `x<hex>` (for example `x7a`), matching the
/// convention used by the shipped profile's ignore list.
pub fn read_sensor_readings<B: EcBackend>(
    ec: &mut Ec<B>,
    offsets: &[u8],
) -> Result<Vec<SensorReading>, EcError> {
    let mut readings = Vec::with_capacity(offsets.len());
    for &offset in offsets {
        let celsius = ec.read_byte(offset)?;
        let name = default_sensor_name(offset)
            .map(str::to_string)
            .unwrap_or_else(|| format!("x{offset:02x}"));
        readings.push(SensorReading {
            offset,
            name,
            celsius,
        });
    }
    Ok(readings)
}

/// The engine's tunable policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnginePolicy {
    /// The minimum time between two fan-level changes.
    pub min_dwell: Duration,
    /// The lowest fan level the engine will command.
    pub safety_floor: u8,
    /// The highest fan level the engine will command.
    pub safety_ceiling: u8,
    /// The temperature (°C) at which the ceiling is forced.
    pub emergency_temperature: u8,
    /// Sensor names (or `x<hex>` offsets) excluded from the maximum.
    pub ignore: Vec<String>,
    /// The smart-mode thresholds, in ascending temperature order.
    pub smart_levels: Vec<SmartLevel>,
    /// Whether smart-mode hysteresis is applied.
    ///
    /// When `false`, the fan level follows the thresholds exactly. This is more
    /// responsive but can oscillate around a threshold. Toggleable at runtime
    /// from the GUI and CLI.
    pub hysteresis_enabled: bool,
}

impl Default for EnginePolicy {
    fn default() -> Self {
        Self {
            min_dwell: DEFAULT_MIN_DWELL,
            safety_floor: DEFAULT_SAFETY_FLOOR,
            safety_ceiling: DEFAULT_SAFETY_CEILING,
            emergency_temperature: DEFAULT_EMERGENCY_TEMPERATURE,
            ignore: Vec::new(),
            smart_levels: Vec::new(),
            hysteresis_enabled: true,
        }
    }
}

impl EnginePolicy {
    /// Build a policy from a parsed [`Config`].
    ///
    /// The first smart mode supplies the thresholds, the `[Sensors]` ignore list
    /// is carried over, and the `Hysteresis` general option controls whether
    /// hysteresis is applied.
    pub fn from_config(config: &Config) -> Self {
        let smart_levels = config
            .smart_modes
            .first()
            .map(|mode| mode.levels.clone())
            .unwrap_or_default();
        Self {
            ignore: config.sensors.ignore.clone(),
            smart_levels,
            hysteresis_enabled: config.general.hysteresis,
            ..Self::default()
        }
    }
}

/// The action the engine wants the caller to take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// No change is required.
    Hold,
    /// Apply a fixed manual fan level.
    SetLevel(u8),
    /// Hand the fan back to the firmware (BIOS auto).
    BiosAuto,
}

/// The fan control state machine.
#[derive(Debug, Clone)]
pub struct Engine {
    policy: EnginePolicy,
    mode: Mode,
    manual_level: u8,
    current_level: Option<u8>,
    last_change: Option<Instant>,
}

impl Engine {
    /// Create an engine in BIOS mode with the given policy.
    pub fn new(policy: EnginePolicy) -> Self {
        Self {
            policy,
            mode: Mode::Bios,
            manual_level: DEFAULT_SAFETY_FLOOR,
            current_level: None,
            last_change: None,
        }
    }

    /// The active mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Switch the active mode.
    ///
    /// The change takes effect on the next [`Engine::evaluate`].
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// The manual fan level applied in [`Mode::Manual`].
    pub fn manual_level(&self) -> u8 {
        self.manual_level
    }

    /// Set the manual fan level, clamped to the safety range.
    pub fn set_manual_level(&mut self, level: u8) {
        self.manual_level = level.clamp(self.policy.safety_floor, self.policy.safety_ceiling);
    }

    /// The last level the engine commanded, or `None` for BIOS auto.
    pub fn current_level(&self) -> Option<u8> {
        self.current_level
    }

    /// The active policy.
    pub fn policy(&self) -> &EnginePolicy {
        &self.policy
    }

    /// Replace the policy (for example after a configuration reload).
    pub fn set_policy(&mut self, policy: EnginePolicy) {
        self.policy = policy;
        self.manual_level = self
            .manual_level
            .clamp(self.policy.safety_floor, self.policy.safety_ceiling);
    }

    /// Whether smart-mode hysteresis is currently applied.
    pub fn hysteresis_enabled(&self) -> bool {
        self.policy.hysteresis_enabled
    }

    /// Enable or disable smart-mode hysteresis at runtime.
    pub fn set_hysteresis_enabled(&mut self, enabled: bool) {
        self.policy.hysteresis_enabled = enabled;
    }

    /// Forget the last commanded level and dwell timer.
    ///
    /// Call this after a configuration reload so the next evaluation re-applies
    /// the (possibly changed) policy immediately.
    pub fn reset(&mut self) {
        self.current_level = None;
        self.last_change = None;
    }

    /// The maximum usable temperature across the readings, if any.
    pub fn max_temperature(&self, readings: &[SensorReading]) -> Option<u8> {
        readings
            .iter()
            .filter(|reading| self.is_usable(reading))
            .map(|reading| reading.celsius)
            .max()
    }

    /// Whether a reading is in range and not ignored.
    fn is_usable(&self, reading: &SensorReading) -> bool {
        // TPFanCtrl2 ignores 0x00 and 0x80; the latter is also above TEMP_MAX_C.
        if reading.celsius == 0 || reading.celsius > TEMP_MAX_C {
            return false;
        }
        !self.is_ignored(reading)
    }

    /// Whether a reading matches the ignore list by name or `x<hex>` offset.
    fn is_ignored(&self, reading: &SensorReading) -> bool {
        let name = reading.name.to_ascii_lowercase();
        let offset_key = format!("x{:02x}", reading.offset);
        self.policy.ignore.iter().any(|entry| {
            let entry = entry.trim().to_ascii_lowercase();
            entry == name || entry == offset_key
        })
    }

    /// Evaluate the current readings and return the action to take.
    ///
    /// `now` is injected so the dwell logic is deterministic in tests.
    pub fn evaluate(&mut self, readings: &[SensorReading], now: Instant) -> Decision {
        let max_temperature = self.max_temperature(readings);
        let emergency = max_temperature
            .is_some_and(|temperature| temperature >= self.policy.emergency_temperature);

        let desired = match self.mode {
            Mode::Bios => None,
            Mode::Manual => Some(self.manual_level),
            Mode::Smart => smart_target(
                &self.policy.smart_levels,
                max_temperature.unwrap_or(0),
                self.current_level,
                self.policy.hysteresis_enabled,
            )
            .or(Some(self.policy.safety_floor)),
        };

        let desired = desired.map(|level| {
            let clamped = level.clamp(self.policy.safety_floor, self.policy.safety_ceiling);
            if emergency {
                self.policy.safety_ceiling
            } else {
                clamped
            }
        });

        if desired == self.current_level {
            return Decision::Hold;
        }

        // Reverting to BIOS and an emergency escalation are never delayed.
        let bypass_dwell = desired.is_none() || emergency;
        if !bypass_dwell {
            if let Some(last) = self.last_change {
                if now.saturating_duration_since(last) < self.policy.min_dwell {
                    return Decision::Hold;
                }
            }
        }

        self.current_level = desired;
        self.last_change = Some(now);
        match desired {
            Some(level) => Decision::SetLevel(level),
            None => Decision::BiosAuto,
        }
    }
}

/// Map a temperature to a fan level using the smart-mode thresholds.
///
/// The base target is the highest level whose threshold is at or below the
/// temperature, falling back to the first level's fan level below the first
/// threshold. When `hysteresis` is enabled, a step up requires the temperature
/// to clear the target threshold by its `hyst_up`, and a step down requires the
/// temperature to fall below the current threshold by its `hyst_down`.
///
/// Returns `None` when no thresholds are configured.
fn smart_target(
    levels: &[SmartLevel],
    temperature: u8,
    current: Option<u8>,
    hysteresis: bool,
) -> Option<u8> {
    let first = levels.first()?;
    let base = levels
        .iter()
        .rev()
        .find(|level| temperature >= level.temperature)
        .map(|level| level.fan_level)
        .unwrap_or(first.fan_level);

    if !hysteresis {
        return Some(base);
    }

    let Some(current) = current else {
        return Some(base);
    };
    if base == current {
        return Some(base);
    }

    if base > current {
        // Stepping up: require the temperature to clear the target threshold by
        // its upward hysteresis.
        if let Some(target) = levels.iter().find(|level| level.fan_level == base) {
            if temperature < target.temperature.saturating_add(target.hyst_up) {
                return Some(current);
            }
        }
    } else {
        // Stepping down: require the temperature to fall below the current
        // threshold by its downward hysteresis.
        if let Some(current_level) = levels.iter().find(|level| level.fan_level == current) {
            if temperature
                >= current_level
                    .temperature
                    .saturating_sub(current_level.hyst_down)
            {
                return Some(current);
            }
        }
    }

    Some(base)
}

/// Apply a [`Decision`] to the EC.
pub fn apply_decision<B: EcBackend>(ec: &mut Ec<B>, decision: Decision) -> Result<(), EcError> {
    match decision {
        Decision::Hold => Ok(()),
        Decision::SetLevel(level) => ec.set_fan_level(level),
        Decision::BiosAuto => ec.set_fan_auto(),
    }
}

/// Errors produced by the control engine and its watchdog.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The underlying EC access failed.
    #[error(transparent)]
    Ec(#[from] EcError),

    /// The guarded control loop panicked; the fan was reverted to BIOS auto.
    #[error("the control loop panicked; the fan was reverted to BIOS auto")]
    Panicked,
}

/// Owns the EC and guarantees a revert to BIOS auto when dropped.
///
/// The watchdog is the last line of defence: whether the daemon exits cleanly,
/// is killed, or unwinds from a panic, dropping the watchdog hands the fan back
/// to the firmware. Disarm it only when another component has taken ownership of
/// the fan.
#[derive(Debug)]
pub struct Watchdog<B: EcBackend> {
    ec: Ec<B>,
    armed: bool,
}

impl<B: EcBackend> Watchdog<B> {
    /// Wrap an EC handle, arming the watchdog.
    pub fn new(ec: Ec<B>) -> Self {
        Self { ec, armed: true }
    }

    /// Mutable access to the underlying EC handle.
    pub fn ec_mut(&mut self) -> &mut Ec<B> {
        &mut self.ec
    }

    /// Whether the watchdog will revert on drop.
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// Disarm the watchdog so dropping it does not touch the EC.
    pub fn disarm(&mut self) {
        self.armed = false;
    }

    /// Immediately revert the fan to BIOS auto control.
    pub fn revert(&mut self) -> Result<(), EcError> {
        self.ec.set_fan_auto()
    }
}

impl<B: EcBackend> Drop for Watchdog<B> {
    fn drop(&mut self) {
        if self.armed {
            // Best effort: a failure here cannot be reported from `drop`.
            let _ = self.ec.set_fan_auto();
        }
    }
}

/// Run a control-loop body, reverting to BIOS auto if it panics.
///
/// This complements the [`Watchdog`]'s `Drop` implementation: it converts a
/// panic into an [`EngineError::Panicked`] and guarantees the revert happens
/// before the error propagates.
pub fn run_guarded<B, F, T>(watchdog: &mut Watchdog<B>, body: F) -> Result<T, EngineError>
where
    B: EcBackend,
    F: FnOnce(&mut Watchdog<B>) -> Result<T, EngineError>,
{
    match std::panic::catch_unwind(AssertUnwindSafe(|| body(watchdog))) {
        Ok(result) => result,
        Err(_) => {
            let _ = watchdog.revert();
            Err(EngineError::Panicked)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ec::{EC_WINDOW_SIZE, FAN_BIOS_AUTO, REG_FAN_CONTROL};
    use std::sync::{Arc, Mutex};

    /// A backend whose register window is shared, so tests can inspect it after
    /// the owning [`Ec`] (or [`Watchdog`]) has been dropped.
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
            if offset >= EC_WINDOW_SIZE {
                return Err(EcError::OutOfBounds { offset });
            }
            Ok(self.bytes.lock().unwrap()[offset])
        }

        fn write_byte(&mut self, offset: usize, value: u8) -> Result<(), EcError> {
            if offset >= EC_WINDOW_SIZE {
                return Err(EcError::OutOfBounds { offset });
            }
            self.bytes.lock().unwrap()[offset] = value;
            Ok(())
        }

        fn read_window(&mut self) -> Result<[u8; EC_WINDOW_SIZE], EcError> {
            Ok(*self.bytes.lock().unwrap())
        }
    }

    fn reading(offset: u8, name: &str, celsius: u8) -> SensorReading {
        SensorReading::new(offset, name, celsius)
    }

    fn smart_level(temperature: u8, fan_level: u8, hyst_up: u8, hyst_down: u8) -> SmartLevel {
        SmartLevel {
            temperature,
            fan_level,
            hyst_up,
            hyst_down,
        }
    }

    fn policy(smart_levels: Vec<SmartLevel>) -> EnginePolicy {
        EnginePolicy {
            min_dwell: Duration::ZERO,
            safety_floor: 1,
            safety_ceiling: FAN_LEVEL_MAX,
            emergency_temperature: DEFAULT_EMERGENCY_TEMPERATURE,
            ignore: Vec::new(),
            smart_levels,
            hysteresis_enabled: true,
        }
    }

    #[test]
    fn bios_mode_holds_when_already_auto() {
        let mut engine = Engine::new(policy(vec![]));
        engine.set_mode(Mode::Bios);
        // A fresh engine is already in BIOS auto, so nothing needs to change.
        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 70)], Instant::now()),
            Decision::Hold
        );
        assert_eq!(engine.current_level(), None);
    }

    #[test]
    fn switching_to_bios_requests_bios_auto() {
        let mut engine = Engine::new(policy(vec![]));
        engine.set_mode(Mode::Manual);
        engine.set_manual_level(3);
        let now = Instant::now();
        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 40)], now),
            Decision::SetLevel(3)
        );
        engine.set_mode(Mode::Bios);
        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 40)], now),
            Decision::BiosAuto
        );
        assert_eq!(engine.current_level(), None);
    }

    #[test]
    fn manual_mode_applies_the_manual_level() {
        let mut engine = Engine::new(policy(vec![]));
        engine.set_mode(Mode::Manual);
        engine.set_manual_level(3);
        let decision = engine.evaluate(&[reading(0x78, "cpu", 40)], Instant::now());
        assert_eq!(decision, Decision::SetLevel(3));
        assert_eq!(engine.current_level(), Some(3));
    }

    #[test]
    fn smart_mode_maps_temperature_to_level() {
        let levels = vec![smart_level(50, 2, 0, 0), smart_level(60, 3, 0, 0)];
        // Below the first threshold uses the first level's fan level; at or
        // above a threshold uses that level's fan level.
        let cases = [(40u8, 2u8), (50, 2), (60, 3), (70, 3)];
        for (temperature, expected) in cases {
            let mut engine = Engine::new(policy(levels.clone()));
            engine.set_mode(Mode::Smart);
            assert_eq!(
                engine.evaluate(&[reading(0x78, "cpu", temperature)], Instant::now()),
                Decision::SetLevel(expected),
                "temperature {temperature} °C"
            );
        }
    }

    #[test]
    fn hysteresis_holds_when_stepping_down() {
        let levels = vec![smart_level(50, 2, 0, 0), smart_level(60, 3, 0, 5)];
        let mut engine = Engine::new(policy(levels));
        engine.set_mode(Mode::Smart);
        let now = Instant::now();

        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 60)], now),
            Decision::SetLevel(3)
        );
        // 57 °C is within 5 °C below the 60 °C threshold: hold at level 3.
        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 57)], now),
            Decision::Hold
        );
        // 54 °C is below 60 - 5: step down to level 2.
        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 54)], now),
            Decision::SetLevel(2)
        );
    }

    #[test]
    fn hysteresis_holds_when_stepping_up() {
        let levels = vec![smart_level(50, 2, 0, 0), smart_level(60, 3, 5, 0)];
        let mut engine = Engine::new(policy(levels));
        engine.set_mode(Mode::Smart);
        let now = Instant::now();

        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 50)], now),
            Decision::SetLevel(2)
        );
        // 62 °C has not cleared 60 + 5: hold at level 2.
        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 62)], now),
            Decision::Hold
        );
        // 65 °C clears the upward hysteresis: step up to level 3.
        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 65)], now),
            Decision::SetLevel(3)
        );
    }

    #[test]
    fn hysteresis_can_be_disabled() {
        let levels = vec![smart_level(50, 2, 0, 0), smart_level(60, 3, 5, 0)];
        let mut engine = Engine::new(policy(levels));
        engine.set_mode(Mode::Smart);
        engine.set_hysteresis_enabled(false);
        assert!(!engine.hysteresis_enabled());
        let now = Instant::now();

        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 50)], now),
            Decision::SetLevel(2)
        );
        // With hysteresis disabled, 62 °C steps up immediately.
        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 62)], now),
            Decision::SetLevel(3)
        );
    }

    #[test]
    fn minimum_dwell_time_delays_changes() {
        let mut engine = Engine::new(EnginePolicy {
            min_dwell: Duration::from_secs(10),
            ..policy(vec![])
        });
        engine.set_mode(Mode::Manual);
        engine.set_manual_level(2);
        let start = Instant::now();

        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 40)], start),
            Decision::SetLevel(2)
        );
        engine.set_manual_level(3);
        // Too soon: hold.
        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 40)], start + Duration::from_secs(5)),
            Decision::Hold
        );
        // Dwell elapsed: apply.
        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 40)], start + Duration::from_secs(10)),
            Decision::SetLevel(3)
        );
    }

    #[test]
    fn emergency_temperature_forces_the_ceiling_and_bypasses_dwell() {
        let mut engine = Engine::new(EnginePolicy {
            min_dwell: Duration::from_secs(3600),
            emergency_temperature: 85,
            safety_ceiling: 7,
            ..policy(vec![])
        });
        engine.set_mode(Mode::Manual);
        engine.set_manual_level(1);
        let start = Instant::now();

        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 50)], start),
            Decision::SetLevel(1)
        );
        // Emergency overrides both the manual level and the dwell timer.
        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 90)], start + Duration::from_secs(1)),
            Decision::SetLevel(7)
        );
    }

    #[test]
    fn safety_floor_and_ceiling_clamp_the_level() {
        let mut engine = Engine::new(EnginePolicy {
            safety_floor: 2,
            safety_ceiling: 3,
            ..policy(vec![])
        });
        engine.set_mode(Mode::Manual);

        engine.set_manual_level(1);
        assert_eq!(engine.manual_level(), 2);
        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 40)], Instant::now()),
            Decision::SetLevel(2)
        );

        engine.set_manual_level(7);
        assert_eq!(engine.manual_level(), 3);
        assert_eq!(
            engine.evaluate(&[reading(0x78, "cpu", 40)], Instant::now()),
            Decision::SetLevel(3)
        );
    }

    #[test]
    fn ignored_sensors_are_excluded_from_the_maximum() {
        let levels = vec![smart_level(50, 2, 0, 0), smart_level(80, 3, 0, 0)];
        let mut engine = Engine::new(EnginePolicy {
            ignore: vec!["pwr".to_string()],
            ..policy(levels)
        });
        engine.set_mode(Mode::Smart);

        let readings = vec![
            reading(0x78, "cpu", 50),
            reading(0xC0, "pwr", 90), // ignored
        ];
        assert_eq!(engine.max_temperature(&readings), Some(50));
        assert_eq!(
            engine.evaluate(&readings, Instant::now()),
            Decision::SetLevel(2)
        );
    }

    #[test]
    fn ignore_list_accepts_hex_offsets() {
        let levels = vec![smart_level(50, 2, 0, 0), smart_level(80, 3, 0, 0)];
        let mut engine = Engine::new(EnginePolicy {
            ignore: vec!["x7d".to_string()],
            ..policy(levels)
        });
        engine.set_mode(Mode::Smart);

        let readings = vec![
            reading(0x78, "cpu", 50),
            reading(0x7D, "no5", 90), // ignored by offset
        ];
        assert_eq!(engine.max_temperature(&readings), Some(50));
    }

    #[test]
    fn out_of_range_sensors_are_excluded() {
        let levels = vec![smart_level(40, 2, 0, 0), smart_level(60, 3, 0, 0)];
        let mut engine = Engine::new(policy(levels));
        engine.set_mode(Mode::Smart);

        let readings = vec![
            reading(0x78, "cpu", 0),   // 0x00 is ignored
            reading(0x79, "gpu", 200), // above TEMP_MAX_C
            reading(0x7D, "no5", 45),
        ];
        assert_eq!(engine.max_temperature(&readings), Some(45));
        assert_eq!(
            engine.evaluate(&readings, Instant::now()),
            Decision::SetLevel(2)
        );
    }

    #[test]
    fn no_usable_sensors_uses_the_baseline_level() {
        let levels = vec![smart_level(50, 2, 0, 0), smart_level(60, 3, 0, 0)];
        let mut engine = Engine::new(policy(levels));
        engine.set_mode(Mode::Smart);
        let readings = vec![reading(0x78, "cpu", 0)];
        assert_eq!(engine.max_temperature(&readings), None);
        // With no usable temperature the first smart level's fan level applies.
        assert_eq!(
            engine.evaluate(&readings, Instant::now()),
            Decision::SetLevel(2)
        );
    }

    #[test]
    fn mode_cycles_bios_manual_smart() {
        assert_eq!(Mode::Bios.next(), Mode::Manual);
        assert_eq!(Mode::Manual.next(), Mode::Smart);
        assert_eq!(Mode::Smart.next(), Mode::Bios);
        assert_eq!(Mode::Smart.as_str(), "smart");
    }

    #[test]
    fn policy_from_config_reads_thresholds_ignore_and_hysteresis() {
        let config: Config = "\
IgnoreSensors=pwr,no5
Hysteresis=0
Level=50 2 0 0
Level=60 3 0 0
"
        .parse()
        .unwrap();
        let policy = EnginePolicy::from_config(&config);
        assert_eq!(policy.ignore, vec!["pwr", "no5"]);
        assert_eq!(policy.smart_levels.len(), 2);
        assert!(!policy.hysteresis_enabled);
    }

    #[test]
    fn apply_decision_writes_to_the_ec() {
        let (backend, bytes) = SharedBackend::new();
        let mut ec = Ec::new(backend);

        apply_decision(&mut ec, Decision::SetLevel(3)).unwrap();
        assert_eq!(bytes.lock().unwrap()[usize::from(REG_FAN_CONTROL)], 3);

        apply_decision(&mut ec, Decision::BiosAuto).unwrap();
        assert_eq!(
            bytes.lock().unwrap()[usize::from(REG_FAN_CONTROL)],
            FAN_BIOS_AUTO
        );

        // Hold is a no-op.
        apply_decision(&mut ec, Decision::Hold).unwrap();
        assert_eq!(
            bytes.lock().unwrap()[usize::from(REG_FAN_CONTROL)],
            FAN_BIOS_AUTO
        );
    }

    #[test]
    fn watchdog_reverts_to_bios_auto_on_drop() {
        let (backend, bytes) = SharedBackend::new();
        let mut watchdog = Watchdog::new(Ec::new(backend));
        watchdog.ec_mut().set_fan_level(3).unwrap();
        assert_eq!(bytes.lock().unwrap()[usize::from(REG_FAN_CONTROL)], 3);

        drop(watchdog);
        assert_eq!(
            bytes.lock().unwrap()[usize::from(REG_FAN_CONTROL)],
            FAN_BIOS_AUTO
        );
    }

    #[test]
    fn watchdog_can_be_disarmed() {
        let (backend, bytes) = SharedBackend::new();
        let mut watchdog = Watchdog::new(Ec::new(backend));
        watchdog.ec_mut().set_fan_level(3).unwrap();
        watchdog.disarm();
        assert!(!watchdog.is_armed());

        drop(watchdog);
        // Disarmed: the fan control register is left untouched.
        assert_eq!(bytes.lock().unwrap()[usize::from(REG_FAN_CONTROL)], 3);
    }

    #[test]
    fn run_guarded_reverts_on_panic() {
        let (backend, bytes) = SharedBackend::new();
        let mut watchdog = Watchdog::new(Ec::new(backend));
        watchdog.ec_mut().set_fan_level(3).unwrap();

        // Silence the panic message for a clean test run.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result: Result<(), EngineError> =
            run_guarded(&mut watchdog, |_| -> Result<(), EngineError> {
                panic!("simulated control-loop failure")
            });
        std::panic::set_hook(previous);

        assert!(matches!(result, Err(EngineError::Panicked)));
        assert_eq!(
            bytes.lock().unwrap()[usize::from(REG_FAN_CONTROL)],
            FAN_BIOS_AUTO
        );
    }

    #[test]
    fn run_guarded_passes_through_success() {
        let (backend, _bytes) = SharedBackend::new();
        let mut watchdog = Watchdog::new(Ec::new(backend));
        let result = run_guarded(&mut watchdog, |_| Ok(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn read_sensor_readings_names_known_and_unknown_offsets() {
        let (backend, bytes) = SharedBackend::new();
        bytes.lock().unwrap()[0x78] = 45;
        bytes.lock().unwrap()[0x7A] = 50;
        let mut ec = Ec::new(backend);
        let readings = read_sensor_readings(&mut ec, &[0x78, 0x7A]).unwrap();
        assert_eq!(readings[0], SensorReading::new(0x78, "cpu", 45));
        assert_eq!(readings[1], SensorReading::new(0x7A, "x7a", 50));
    }
}
