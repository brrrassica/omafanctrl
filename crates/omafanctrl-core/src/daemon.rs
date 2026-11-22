//! Daemon control loop and lifecycle.
//!
//! [`Daemon`] owns the EC handle (through a [`Watchdog`]), the control
//! [`Engine`], and the active [`Config`]. On every control tick it:
//!
//! 1. reloads the configuration if the file changed,
//! 2. reads the sensor bank,
//! 3. evaluates the engine and applies the resulting [`Decision`] to the EC,
//! 4. publishes a [`State`] snapshot for the D-Bus layer.
//!
//! Sensor readings are additionally sampled and published at
//! [`TELEMETRY_INTERVAL`] (4 Hz) so the GUI's temperature history stays fresh,
//! while the control decision is still evaluated once per configured interval.
//!
//! Commands arriving from the D-Bus layer are applied immediately (the daemon
//! re-evaluates after each command), so a mode change is visible without waiting
//! for the next tick.
//!
//! The loop is generic over [`EcBackend`], so the whole daemon can be
//! integration-tested against a mocked EC.
//!
//! [`Decision`]: crate::engine::Decision

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};

use crate::config::{Config, ConfigWatcher};
use crate::dbus::{Command, ControlService, SensorState, State};
use crate::ec::{Ec, EcBackend, EcError};
use crate::engine::{
    Engine, EngineError, EnginePolicy, Mode, SensorReading, Watchdog, apply_decision,
    preferred_smart_slot, read_sensor_readings,
};
use crate::probe::KNOWN_SENSOR_OFFSETS;

/// The capacity of the command channel between the D-Bus layer and the loop.
pub const COMMAND_CHANNEL_CAPACITY: usize = 16;

/// The interval at which sensor readings are sampled and published for
/// telemetry, independent of the fan-control cadence.
///
/// 250 ms gives the GUI a 4 Hz temperature history while the control decision
/// still runs at the configured interval.
pub const TELEMETRY_INTERVAL: Duration = Duration::from_millis(250);

/// The event that woke the control loop.
enum LoopEvent {
    /// The telemetry ticker fired: publish a sample without evaluating control.
    Telemetry,
    /// The dedicated control deadline elapsed: evaluate the fan decision.
    Control,
    /// A command arrived from the D-Bus layer, or the channel closed.
    Command(Option<Command>),
}

/// The daemon control loop.
pub struct Daemon<B: EcBackend> {
    watchdog: Watchdog<B>,
    engine: Engine,
    config: Config,
    watcher: ConfigWatcher,
    sensor_offsets: Vec<u8>,
    state_tx: watch::Sender<State>,
    config_tx: watch::Sender<String>,
    command_tx: mpsc::Sender<Command>,
    commands: mpsc::Receiver<Command>,
    config_path: PathBuf,
    telemetry_interval: Duration,
    /// Consecutive *control* iterations with at least one failed EC read.
    ///
    /// `MaxReadErrors` is documented per control `Cycle`, so only the control
    /// cadence counts toward the abort budget. Telemetry samples run at 4 Hz
    /// and must neither consume nor reset this budget: a shared counter made
    /// the daemon abort after ~2.5 s of EC contention instead of 10 control
    /// cycles, and any successful telemetry sample masked a control path
    /// failing every cycle. Telemetry failures are logged and surfaced through
    /// the last-known-good snapshot instead.
    read_errors: u32,
    /// The last successfully read sensor bank, used to fill in sensors whose
    /// read failed on the current iteration.
    last_readings: Vec<SensorReading>,
    /// The last successfully read fan speed.
    last_fan_rpm: u16,
}

impl<B: EcBackend> Daemon<B> {
    /// Create a daemon around an EC handle and an initial configuration.
    pub fn new(ec: Ec<B>, config: Config, config_path: PathBuf) -> Self {
        let mut engine = Engine::new(EnginePolicy::from_config(&config));
        // Start in the mode selected by the configuration's `Active` value
        // (0 = BIOS, 1 = Smart 1, 2 = Smart 2, 3 = Manual). The default
        // configuration uses Smart, so a fresh install boots into smart control.
        engine.set_mode(Mode::from_active(config.general.active));
        engine.set_active_smart_profile(preferred_smart_slot(config.general.active));
        // Apply the configured manual level so a configuration that starts in
        // Manual mode commands `ManFanSpeed` rather than the safety floor.
        engine.set_manual_level(config.general.man_fan_speed);
        let mut watcher = ConfigWatcher::new(config_path.clone());
        // Prime the watcher with the on-disk contents when they match the loaded
        // configuration, so the first control tick does not treat startup as an
        // external edit and spuriously reload/reset the engine.
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            if contents.parse::<Config>().ok().as_ref() == Some(&config) {
                watcher.note_written(&contents);
            }
        }
        let (state_tx, _state_rx) = watch::channel(State::default());
        let (config_tx, _config_rx) = watch::channel(config.to_ini());
        let (command_tx, commands) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        Self {
            watchdog: Watchdog::new(ec),
            engine,
            config,
            watcher,
            sensor_offsets: KNOWN_SENSOR_OFFSETS.to_vec(),
            state_tx,
            config_tx,
            command_tx,
            commands,
            config_path,
            telemetry_interval: TELEMETRY_INTERVAL,
            read_errors: 0,
            last_readings: Vec::new(),
            last_fan_rpm: 0,
        }
    }

    /// Override the telemetry sampling interval.
    ///
    /// Telemetry drives the GUI's temperature history independently of the
    /// fan-control cadence. A longer interval reduces idle CPU wakeups at the
    /// cost of a less responsive history; see `docs/profiling.md`. The interval
    /// is clamped to at least 1 ms so the ticker can never be zero.
    pub fn with_telemetry_interval(mut self, interval: Duration) -> Self {
        self.telemetry_interval = interval.max(Duration::from_millis(1));
        self
    }

    /// The active configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The control engine.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The path of the active configuration file.
    pub fn config_path(&self) -> &std::path::Path {
        &self.config_path
    }

    /// A [`ControlService`] wired to this daemon's channels.
    pub fn control_service(&self) -> ControlService {
        ControlService::new(
            self.state_tx.subscribe(),
            self.config_tx.subscribe(),
            self.command_tx.clone(),
            self.config_path.clone(),
        )
    }

    /// A receiver for state snapshots, used to emit `StateChanged` signals.
    pub fn state_receiver(&self) -> watch::Receiver<State> {
        self.state_tx.subscribe()
    }

    /// A receiver for configuration text, used to emit `ConfigChanged` signals.
    pub fn config_receiver(&self) -> watch::Receiver<String> {
        self.config_tx.subscribe()
    }

    /// Read the sensor bank and the fan speed.
    ///
    /// A failed sensor offset is filled from the last successful sample when
    /// available. Any sensor read that could not be refreshed (including a
    /// partial read where some offsets failed) marks the iteration as failed,
    /// so the control path can retain its last decision instead of acting on
    /// incomplete or stale data. A failed tachometer read keeps the last known
    /// RPM and also marks the iteration as failed. A tachometer reading of 0
    /// RPM while the engine commands a nonzero fan level is implausible (a
    /// stalled fan, a disconnected tachometer, or a bad EC read): it also keeps
    /// the last known RPM and marks the iteration as failed. After
    /// [`General::max_read_errors`] consecutive failed *control* iterations an
    /// error is returned so the daemon exits and the watchdog reverts the fan.
    ///
    /// Telemetry-only samples (see `sample`) do not count toward this budget
    /// and do not reset it: `MaxReadErrors` is a per-`Cycle` budget, and the
    /// 4 Hz telemetry cadence would otherwise consume it 16x faster than
    /// documented (or mask a control-path failure streak with a 4 Hz reset).
    ///
    /// [`General::max_read_errors`]: crate::config::General::max_read_errors
    fn read_sensors(
        &mut self,
        count_toward_budget: bool,
    ) -> Result<(Vec<SensorReading>, u16, bool), EngineError> {
        let mut last_error = None;
        let (readings, sensors_failed) = match read_sensor_readings(
            self.watchdog.ec_mut(),
            &self.sensor_offsets,
            &self.last_readings,
        ) {
            Ok((readings, sensor_error)) => {
                if let Some(error) = sensor_error {
                    last_error = Some(error);
                }
                (readings, last_error.is_some())
            }
            Err(error) => {
                last_error = Some(error);
                (Vec::new(), true)
            }
        };
        let (fan_rpm, fan_failed) = match self.watchdog.ec_mut().read_fan_rpm() {
            // While the engine commands a nonzero fan level, a tachometer read
            // of 0 RPM is implausible: the fan cannot be stationary while it is
            // being driven. Treat it like a failed read so a stalled fan (or a
            // disconnected tachometer, or a bad EC read) accumulates toward
            // `MaxReadErrors` instead of silently resetting the error streak,
            // and keep the last known RPM for telemetry. A genuine 0 RPM in
            // BIOS-auto or idle is accepted.
            Ok(0) if self.engine.current_level().is_some_and(|level| level > 0) => {
                last_error.get_or_insert(EcError::FanRpmUnplausible { value: 0 });
                (self.last_fan_rpm, true)
            }
            Ok(fan_rpm) => (fan_rpm, false),
            Err(error) => {
                last_error.get_or_insert(error);
                (self.last_fan_rpm, true)
            }
        };

        let readings = if readings.len() == self.sensor_offsets.len() {
            readings
        } else {
            self.merge_last_readings(readings)
        };

        if sensors_failed || fan_failed {
            if count_toward_budget {
                self.read_errors = self.read_errors.saturating_add(1);
                if self.read_errors > self.config.general.max_read_errors {
                    let error = last_error.expect("a failed read iteration must carry an EC error");
                    return Err(error.into());
                }
            } else {
                tracing::debug!(?last_error, "telemetry sample hit a failed EC read");
            }
        } else if count_toward_budget {
            self.read_errors = 0;
        }

        self.last_readings = readings.clone();
        self.last_fan_rpm = fan_rpm;
        Ok((readings, fan_rpm, sensors_failed || fan_failed))
    }

    /// Fill sensors missing from `readings` with their last successful value.
    fn merge_last_readings(&self, readings: Vec<SensorReading>) -> Vec<SensorReading> {
        self.sensor_offsets
            .iter()
            .filter_map(|&offset| {
                readings
                    .iter()
                    .find(|reading| reading.offset == offset)
                    .cloned()
                    .or_else(|| {
                        self.last_readings
                            .iter()
                            .find(|reading| reading.offset == offset)
                            .cloned()
                    })
            })
            .collect()
    }

    /// Run one control iteration.
    pub fn tick(&mut self, now: Instant) -> Result<(), EngineError> {
        self.poll_config();
        let (readings, fan_rpm, read_failed) = self.read_sensors(true)?;
        if read_failed {
            // Keep the last commanded level and publish the retained snapshot
            // rather than acting on partial or stale sensor data.
            self.publish_state(&readings, fan_rpm);
            return Ok(());
        }
        if self.engine.exit_manual_if_hot(&readings) {
            // The thermal safety exit took over from Manual, so mirror the
            // `SetMode` bookkeeping: record the new mode in the live config
            // and persist it. Otherwise any later `persist_config` (for
            // example `SetHysteresis`) would write `Active=3` to disk and a
            // restart would silently boot back into the unsafe Manual mode.
            self.config.general.active = self.engine.active_smart_profile();
            self.persist_config();
            let _ = self.config_tx.send(self.config.to_ini());
        }
        let decision = self.engine.decide(&readings, now);
        match apply_decision(self.watchdog.ec_mut(), decision) {
            Ok(()) => self.engine.commit(decision, now),
            Err(EcError::RateLimited { retry_after }) => {
                // Leave the engine state untouched so the change is retried on
                // the next tick rather than being silently dropped.
                tracing::debug!(
                    ?retry_after,
                    "EC write rate-limited; retrying on the next tick"
                );
            }
            Err(error) => return Err(error.into()),
        }
        self.publish_state(&readings, fan_rpm);
        Ok(())
    }

    /// Sample the sensors and publish a state snapshot without evaluating
    /// control.
    ///
    /// This drives the fast telemetry cadence so the GUI's temperature history
    /// stays fresh between control iterations, without touching the fan.
    pub fn sample(&mut self) -> Result<(), EngineError> {
        // Telemetry never consumes or resets the control-path error budget
        // (`MaxReadErrors` is a per-Cycle budget); see `read_sensors`.
        let (readings, fan_rpm, _) = self.read_sensors(false)?;
        self.publish_state(&readings, fan_rpm);
        Ok(())
    }

    /// Apply a command from the D-Bus layer, then re-evaluate immediately.
    pub fn handle_command(&mut self, command: Command) -> Result<(), EngineError> {
        match command {
            Command::SetMode(mode) => {
                self.engine.set_mode(mode);
                // Do not re-derive the smart profile slot from the live config:
                // while outside Smart mode `config.general.active` holds the
                // transient BIOS/Manual encoding (0/3), so re-deriving here
                // would clobber the user's Smart slot selection on re-entry.
                // The engine keeps its slot across non-Smart modes; it is only
                // re-derived from the config in `apply_config`.
                self.config.general.active = match mode {
                    Mode::Bios => 0,
                    Mode::Manual => 3,
                    Mode::Smart => self.engine.active_smart_profile(),
                };
                // Persist runtime mode changes so explicit/external reloads and
                // restarts do not revert to stale file-backed values, and keep the
                // watcher in sync like other self-writes.
                self.persist_config();
                let _ = self.config_tx.send(self.config.to_ini());
            }
            Command::SetManualLevel(level) => {
                self.engine.set_manual_level(level);
                self.config.general.man_fan_speed = self.engine.manual_level();
                // Persist runtime manual speed changes so reloads/restarts do not
                // revert them and unrelated persistence does not have to capture
                // them as a side effect.
                self.persist_config();
                let _ = self.config_tx.send(self.config.to_ini());
            }
            Command::SetHysteresis(enabled) => {
                self.config.general.hysteresis = enabled;
                self.engine.set_hysteresis_enabled(enabled);
                self.persist_config();
                let _ = self.config_tx.send(self.config.to_ini());
            }
            Command::SetConfig(contents) => match contents.parse::<Config>() {
                Ok(config) => {
                    self.apply_config(config);
                    // Write the exact document the caller sent, and advance the
                    // watcher to the same text, so the next poll cannot mistake
                    // this write for an external edit.
                    self.write_config(&contents);
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "ignoring invalid configuration from the control interface; \
                         keeping the last good configuration"
                    );
                }
            },
            Command::ReloadConfig => match self.watcher.reload() {
                Ok(config) => self.apply_config(config),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "ignoring configuration reload error; keeping the last good configuration"
                    );
                }
            },
        }
        // An explicit command should take effect immediately, bypassing the
        // anti-oscillation dwell timer.
        self.engine.clear_dwell();
        self.tick(Instant::now())
    }

    /// Run the control loop until `shutdown` is signalled or the command channel
    /// closes.
    ///
    /// `interval` overrides the control-loop cadence. When `None`, the cadence
    /// comes from `[General] Cycle` in the live configuration and is re-read
    /// after every control tick, so a configuration reload takes effect without
    /// restarting the daemon.
    ///
    /// Blocking EC I/O and fan-apply verification run on Tokio's blocking pool so
    /// the async runtime is not stalled by slow or contended EC access.
    pub async fn run(
        mut self,
        interval: Option<Duration>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), EngineError>
    where
        B: 'static,
    {
        // Sample and publish at the telemetry cadence, but drive the fan
        // control decision from its own deadline. Sharing one ticker made the
        // effective control cadence `max(telemetry_interval, Cycle)`, so a long
        // telemetry interval could starve control entirely.
        let mut ticker = tokio::time::interval(self.telemetry_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut next_control = Instant::now();
        loop {
            let control_deadline = tokio::time::Instant::from_std(next_control);
            let event = tokio::select! {
                _ = ticker.tick() => LoopEvent::Telemetry,
                _ = tokio::time::sleep_until(control_deadline) => LoopEvent::Control,
                command = self.commands.recv() => LoopEvent::Command(command),
                result = shutdown.changed() => {
                    if result.is_err() || *shutdown.borrow() {
                        break;
                    }
                    continue;
                }
            };
            match event {
                LoopEvent::Telemetry => {
                    self = tokio::task::spawn_blocking(move || self.sample().map(|()| self))
                        .await
                        .map_err(|_| EngineError::Panicked)??;
                }
                LoopEvent::Control => {
                    let now = Instant::now();
                    self = tokio::task::spawn_blocking(move || self.tick(now).map(|()| self))
                        .await
                        .map_err(|_| EngineError::Panicked)??;
                    next_control = Instant::now() + self.control_interval(interval);
                }
                LoopEvent::Command(Some(command)) => {
                    self = tokio::task::spawn_blocking(move || {
                        self.handle_command(command).map(|()| self)
                    })
                    .await
                    .map_err(|_| EngineError::Panicked)??;
                    next_control = Instant::now() + self.control_interval(interval);
                }
                LoopEvent::Command(None) => break,
            }
        }
        Ok(())
    }

    /// Resolve the control-loop cadence.
    ///
    /// A CLI override wins; otherwise the current `[General] Cycle` is used
    /// (clamped to at least one second, matching config validation).
    fn control_interval(&self, override_interval: Option<Duration>) -> Duration {
        override_interval
            .unwrap_or_else(|| Duration::from_secs(u64::from(self.config.general.cycle.max(1))))
    }

    /// Reload the configuration if the file changed, keeping the last good
    /// configuration on error.
    fn poll_config(&mut self) {
        match self.watcher.poll() {
            Ok(Some(config)) => self.apply_config(config),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    %error,
                    "ignoring configuration reload error; keeping the last good configuration"
                );
            }
        }
    }

    /// Adopt a new configuration and reset the engine so it takes effect.
    ///
    /// When the loaded configuration is identical to the live one (an explicit
    /// `ReloadConfig` of an unchanged file, or a watcher re-read of the same
    /// contents), the engine is left untouched: a reload must not cause a
    /// redundant reset that clears dwell/anti-oscillation state.
    fn apply_config(&mut self, config: Config) {
        if config == self.config {
            tracing::debug!("configuration unchanged; skipping engine reset");
            return;
        }
        self.engine.set_policy(EnginePolicy::from_config(&config));
        // Keep the engine's manual level in sync with the reloaded file; the
        // setter clamps to the safety floor/ceiling.
        self.engine.set_manual_level(config.general.man_fan_speed);
        self.engine
            .set_mode(Mode::from_active(config.general.active));
        self.engine
            .set_active_smart_profile(preferred_smart_slot(config.general.active));
        self.engine.reset();
        let ini = config.to_ini();
        self.config = config;
        let _ = self.config_tx.send(ini);
    }

    /// Write the live configuration back to the configured path.
    ///
    /// Used by runtime settings such as `SetHysteresis`, whose values are
    /// documented as persisted in `[General]`. A write failure is logged rather
    /// than aborting the control loop; the live setting still applies.
    fn persist_config(&mut self) {
        let ini = self.config.to_ini();
        self.write_config(&ini);
    }

    /// Write configuration text and record it with the watcher.
    ///
    /// This is the only writer of the configuration file: both runtime
    /// persistence and `SetConfig` go through the control loop, so the two can
    /// no longer clobber each other. The write is atomic, so a reader (or the
    /// watcher) never observes a truncated document.
    fn write_config(&mut self, contents: &str) {
        if let Err(error) = write_config_atomic(&self.config_path, contents) {
            tracing::warn!(
                %error,
                path = %self.config_path.display(),
                "failed to persist configuration; the live setting still applies"
            );
            return;
        }
        // Advance the watcher past our own write so the next poll does not
        // treat it as an external edit and reset live runtime state.
        self.watcher.note_written(contents);
    }

    /// Publish a state snapshot for the D-Bus layer.
    fn publish_state(&self, readings: &[SensorReading], fan_rpm: u16) {
        let state = State {
            mode: self.engine.mode().as_str().to_string(),
            active_smart_profile: self.engine.active_smart_profile(),
            manual_level: self.engine.manual_level(),
            current_level: self.engine.current_level().unwrap_or(0),
            bios_auto: self.engine.current_level().is_none(),
            fan_rpm,
            hysteresis_enabled: self.engine.hysteresis_enabled(),
            temperatures: readings
                .iter()
                .map(|reading| SensorState {
                    name: reading.name.clone(),
                    offset: reading.offset,
                    celsius: reading.celsius,
                })
                .collect(),
            config_path: self.config_path.display().to_string(),
        };
        let _ = self.state_tx.send(state);
    }
}

/// Write `contents` to `path` via a temporary file and an atomic rename.
///
/// The daemon loop is the single writer, so a per-process temporary name is
/// sufficient. A failed rename removes the temporary file rather than leaving
/// it behind.
fn write_config_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "configuration path has no file name",
        )
    })?;
    let mut temp_name = std::ffi::OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".tmp.{}", std::process::id()));
    let temp_path = path.with_file_name(temp_name);

    std::fs::write(&temp_path, contents)?;
    if let Err(error) = std::fs::rename(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ec::{
        EC_WINDOW_SIZE, EcError, FAN_BIOS_AUTO, REG_FAN_CONTROL, REG_FAN_RPM_HIGH, REG_FAN_RPM_LOW,
        TEMP_MAX_C,
    };
    use crate::engine::Mode;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    /// A backend whose register window is shared, so tests can inspect it after
    /// the daemon (and its watchdog) has been dropped.
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

        /// Lock the window, recovering from poisoning so a failed assertion in
        /// one test cannot abort the process during unwinding.
        fn lock(&self) -> std::sync::MutexGuard<'_, [u8; EC_WINDOW_SIZE]> {
            self.bytes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }

    impl EcBackend for SharedBackend {
        fn read_byte(&mut self, offset: usize) -> Result<u8, EcError> {
            if offset >= EC_WINDOW_SIZE {
                return Err(EcError::OutOfBounds { offset });
            }
            Ok(self.lock()[offset])
        }

        fn write_byte(&mut self, offset: usize, value: u8) -> Result<(), EcError> {
            if offset >= EC_WINDOW_SIZE {
                return Err(EcError::OutOfBounds { offset });
            }
            self.lock()[offset] = value;
            Ok(())
        }

        fn read_window(&mut self) -> Result<[u8; EC_WINDOW_SIZE], EcError> {
            Ok(*self.lock())
        }
    }

    /// Build an EC with the write rate limit disabled, so tests can issue
    /// several commands in quick succession.
    fn ec<B: EcBackend>(backend: B) -> Ec<B> {
        Ec::new(backend).with_min_write_interval(Duration::ZERO)
    }

    /// A backend that can inject read failures for selected offsets or for
    /// every read, leaving writes untouched.
    #[derive(Clone)]
    struct FlakyBackend {
        inner: SharedBackend,
        failing_offsets: Arc<Mutex<Vec<usize>>>,
        fail_all: Arc<AtomicU32>,
    }

    impl FlakyBackend {
        fn new(inner: SharedBackend) -> Self {
            Self {
                inner,
                failing_offsets: Arc::new(Mutex::new(Vec::new())),
                fail_all: Arc::new(AtomicU32::new(0)),
            }
        }

        fn failing_offsets(&self) -> std::sync::MutexGuard<'_, Vec<usize>> {
            self.failing_offsets
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }

    impl EcBackend for FlakyBackend {
        fn read_byte(&mut self, offset: usize) -> Result<u8, EcError> {
            if self.fail_all.load(Ordering::Relaxed) > 0 {
                self.fail_all.fetch_sub(1, Ordering::Relaxed);
                return Err(injected_read_error());
            }
            if self.failing_offsets().contains(&offset) {
                return Err(injected_read_error());
            }
            self.inner.read_byte(offset)
        }

        fn write_byte(&mut self, offset: usize, value: u8) -> Result<(), EcError> {
            self.inner.write_byte(offset, value)
        }

        fn read_window(&mut self) -> Result<[u8; EC_WINDOW_SIZE], EcError> {
            self.inner.read_window()
        }
    }

    fn injected_read_error() -> EcError {
        EcError::Io {
            path: PathBuf::from("/dev/null"),
            source: std::io::Error::other("injected read error"),
        }
    }

    /// Read the fan control register without holding the lock across a panic.
    fn control_register(bytes: &Arc<Mutex<[u8; EC_WINDOW_SIZE]>>) -> u8 {
        bytes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())[usize::from(REG_FAN_CONTROL)]
    }

    /// Set a register byte without holding the lock across a panic.
    fn set_byte(bytes: &Arc<Mutex<[u8; EC_WINDOW_SIZE]>>, offset: u8, value: u8) {
        bytes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())[usize::from(offset)] = value;
    }

    fn temp_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "omafanctrl-daemon-{}-{tag}-{unique}.ini",
            std::process::id()
        ))
    }

    fn smart_config() -> Config {
        "Level=50 2 0 0\nLevel=60 3 0 0\n".parse().unwrap()
    }

    #[test]
    fn daemon_starts_in_smart_mode_by_default() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        let mut daemon = Daemon::new(
            ec(backend),
            smart_config(),
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );
        let mut state_rx = daemon.state_receiver();

        // No explicit SetMode: the default `Active=2` selects smart control.
        daemon.tick(Instant::now()).unwrap();

        assert_eq!(daemon.engine().mode(), Mode::Smart);
        assert_eq!(control_register(&bytes), 3);
        let state = state_rx.borrow_and_update().clone();
        assert_eq!(state.mode, "smart");
    }

    #[test]
    fn first_tick_does_not_spuriously_reload_unchanged_config() {
        let path = temp_path("startup-no-spurious-reload");
        std::fs::write(&path, "Level=50 2 0 0\nLevel=60 3 0 0\n").unwrap();
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        let config = Config::from_file(&path).unwrap();
        let mut daemon = Daemon::new(ec(backend), config, path.clone());
        let mut config_rx = daemon.config_receiver();
        let _ = config_rx.borrow_and_update();

        daemon.tick(Instant::now()).unwrap();

        assert!(
            !config_rx.has_changed().unwrap_or(false),
            "first tick must not treat startup config as an external reload"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn daemon_honours_active_bios() {
        let (backend, _bytes) = SharedBackend::new();
        let config: Config = "Active=0\nLevel=50 2 0 0\nLevel=60 3 0 0\n"
            .parse()
            .unwrap();
        let daemon = Daemon::new(
            ec(backend),
            config,
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );
        assert_eq!(daemon.engine().mode(), Mode::Bios);
    }

    #[test]
    fn daemon_applies_smart_mode_and_publishes_state() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        let mut daemon = Daemon::new(
            ec(backend),
            smart_config(),
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );
        let mut state_rx = daemon.state_receiver();

        daemon
            .handle_command(Command::SetMode(Mode::Smart))
            .unwrap();

        assert_eq!(control_register(&bytes), 3);
        let state = state_rx.borrow_and_update().clone();
        assert_eq!(state.mode, "smart");
        assert_eq!(state.current_level, 3);
        assert!(!state.bios_auto);
        assert!(
            state
                .temperatures
                .iter()
                .any(|sensor| sensor.name == "cpu" && sensor.celsius == 60)
        );
    }

    #[test]
    fn daemon_applies_configured_manual_level_at_startup() {
        let (backend, bytes) = SharedBackend::new();
        let config: Config = "Active=3\nManFanSpeed=5\nLevel=50 2 0 0\nLevel=60 3 0 0\n"
            .parse()
            .unwrap();
        let mut daemon = Daemon::new(
            ec(backend),
            config,
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );

        assert_eq!(daemon.engine().mode(), Mode::Manual);
        assert_eq!(daemon.engine().manual_level(), 5);

        daemon.tick(Instant::now()).unwrap();
        assert_eq!(control_register(&bytes), 5);
    }

    #[test]
    fn daemon_applies_manual_level_when_config_is_reloaded() {
        let path = temp_path("manual-reload");
        std::fs::write(&path, "Level=50 2 0 0\nLevel=60 3 0 0\n").unwrap();
        let (backend, bytes) = SharedBackend::new();
        let config = Config::from_file(&path).unwrap();
        let mut daemon = Daemon::new(ec(backend), config, path.clone());

        // Start in Manual mode with the default level (the safety floor).
        daemon
            .handle_command(Command::SetMode(Mode::Manual))
            .unwrap();
        assert_eq!(control_register(&bytes), 1);

        // Reload a configuration that raises the manual level.
        std::fs::write(
            &path,
            "Active=3\nManFanSpeed=5\nLevel=50 2 0 0\nLevel=60 3 0 0\n",
        )
        .unwrap();
        daemon.handle_command(Command::ReloadConfig).unwrap();
        assert_eq!(daemon.engine().manual_level(), 5);
        assert_eq!(control_register(&bytes), 5);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn daemon_applies_manual_level() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, REG_FAN_RPM_LOW, 0x3C);
        set_byte(&bytes, REG_FAN_RPM_HIGH, 0x0F);
        let mut daemon = Daemon::new(
            ec(backend),
            smart_config(),
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );
        daemon
            .handle_command(Command::SetMode(Mode::Manual))
            .unwrap();
        daemon.handle_command(Command::SetManualLevel(2)).unwrap();
        assert_eq!(control_register(&bytes), 2);
    }

    #[test]
    fn daemon_exits_manual_mode_above_man_mode_exit() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 50);
        let config: Config =
            "Active=3\nManFanSpeed=1\nManModeExit=40\nLevel=50 2 0 0\nLevel=60 3 0 0\n"
                .parse()
                .unwrap();
        let mut daemon = Daemon::new(
            ec(backend),
            config,
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );
        assert_eq!(daemon.engine().mode(), Mode::Manual);

        daemon.tick(Instant::now()).unwrap();

        assert_eq!(daemon.engine().mode(), Mode::Smart);
        assert_eq!(control_register(&bytes), 2);
        // The automatic safety exit must update the live config like `SetMode`
        // does, so a later persist does not write the stale Manual `Active=3`.
        assert_eq!(daemon.config().general.active, 1);
    }

    #[test]
    fn daemon_persists_manual_exit_as_smart() {
        let path = temp_path("persist-manual-exit");
        std::fs::write(
            &path,
            "Active=3\nManFanSpeed=1\nManModeExit=40\nHysteresis=1\nLevel=50 2 0 0\nLevel=60 3 0 0\n",
        )
        .unwrap();
        let config = Config::from_file(&path).unwrap();
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 50);
        let mut daemon = Daemon::new(ec(backend), config, path.clone());

        daemon.tick(Instant::now()).unwrap();
        assert_eq!(daemon.engine().mode(), Mode::Smart);

        // A later, unrelated persisted setting must capture the safety exit,
        // not resurrect the stale `Active=3` Manual mode on the next restart.
        daemon
            .handle_command(Command::SetHysteresis(false))
            .unwrap();
        let persisted = Config::from_file(&path).unwrap();
        assert_eq!(persisted.general.active, 1);
        assert!(!persisted.general.hysteresis);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn daemon_stays_manual_below_man_mode_exit() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 40);
        let config: Config =
            "Active=3\nManFanSpeed=1\nManModeExit=40\nLevel=50 2 0 0\nLevel=60 3 0 0\n"
                .parse()
                .unwrap();
        let mut daemon = Daemon::new(
            ec(backend),
            config,
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );
        assert_eq!(daemon.engine().mode(), Mode::Manual);

        daemon.tick(Instant::now()).unwrap();

        assert_eq!(daemon.engine().mode(), Mode::Manual);
        assert_eq!(control_register(&bytes), 1);
    }

    #[test]
    fn sample_publishes_state_without_touching_the_fan() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        set_byte(&bytes, REG_FAN_CONTROL, 3);
        let mut daemon = Daemon::new(
            ec(backend),
            smart_config(),
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );
        let mut state_rx = daemon.state_receiver();

        daemon.sample().unwrap();

        // Sampling must not change the fan control register.
        assert_eq!(control_register(&bytes), 3);
        let state = state_rx.borrow_and_update().clone();
        assert!(
            state
                .temperatures
                .iter()
                .any(|sensor| sensor.name == "cpu" && sensor.celsius == 60)
        );
    }

    #[test]
    fn sample_does_not_publish_implausible_temperatures() {
        let (backend, bytes) = SharedBackend::new();
        // 0x80 is the BIOS-auto sentinel / a transient glitch byte; it must not
        // reach the D-Bus state as a 128 °C reading.
        set_byte(&bytes, 0x78, 0x80);
        set_byte(&bytes, 0x79, 55);
        let mut daemon = Daemon::new(
            ec(backend),
            smart_config(),
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );
        let mut state_rx = daemon.state_receiver();

        daemon.sample().unwrap();

        let state = state_rx.borrow_and_update().clone();
        assert!(
            state
                .temperatures
                .iter()
                .all(|sensor| sensor.celsius <= TEMP_MAX_C)
        );
        assert!(
            state
                .temperatures
                .iter()
                .any(|sensor| sensor.name == "gpu" && sensor.celsius == 55)
        );
        assert!(
            !state
                .temperatures
                .iter()
                .any(|sensor| sensor.offset == 0x78)
        );
    }

    #[test]
    fn daemon_toggles_hysteresis() {
        let (backend, _bytes) = SharedBackend::new();
        let mut daemon = Daemon::new(
            ec(backend),
            smart_config(),
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );
        assert!(daemon.engine().hysteresis_enabled());
        daemon
            .handle_command(Command::SetHysteresis(false))
            .unwrap();
        assert!(!daemon.engine().hysteresis_enabled());
    }

    #[test]
    fn daemon_persists_hysteresis_toggle() {
        let path = temp_path("hysteresis");
        std::fs::write(&path, "Hysteresis=1\nLevel=50 2 0 0\nLevel=60 3 0 0\n").unwrap();
        let (backend, _bytes) = SharedBackend::new();
        let config = Config::from_file(&path).unwrap();
        let mut daemon = Daemon::new(ec(backend), config, path.clone());

        daemon
            .handle_command(Command::SetHysteresis(false))
            .unwrap();

        assert!(!daemon.config().general.hysteresis);
        assert!(!daemon.engine().hysteresis_enabled());
        // The toggle is written to disk, so it survives a reload and restart.
        assert!(!Config::from_file(&path).unwrap().general.hysteresis);
        daemon.handle_command(Command::ReloadConfig).unwrap();
        assert!(!daemon.engine().hysteresis_enabled());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn daemon_persists_current_mode_and_manual_level_on_hysteresis_toggle() {
        let path = temp_path("persist-runtime-state");
        std::fs::write(
            &path,
            "Active=2\nManFanSpeed=1\nHysteresis=1\nLevel=50 2 0 0\nLevel=60 3 0 0\n",
        )
        .unwrap();
        let (backend, _bytes) = SharedBackend::new();
        let config = Config::from_file(&path).unwrap();
        let mut daemon = Daemon::new(ec(backend), config, path.clone());

        daemon.handle_command(Command::SetManualLevel(5)).unwrap();
        daemon
            .handle_command(Command::SetMode(Mode::Manual))
            .unwrap();
        daemon
            .handle_command(Command::SetHysteresis(false))
            .unwrap();

        let persisted = Config::from_file(&path).unwrap();
        assert_eq!(persisted.general.active, 3);
        assert_eq!(persisted.general.man_fan_speed, 5);
        assert!(!persisted.general.hysteresis);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn daemon_set_mode_smart_preserves_engine_smart_slot_across_mode_cycle() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        let config: Config = "Active=2\nLevel=50 2 0 0\nLevel=60 3 0 0\n"
            .parse()
            .unwrap();
        let mut daemon = Daemon::new(
            ec(backend),
            config,
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );

        // Startup with `Active=2`: Smart 2 is the active slot.
        assert_eq!(daemon.engine().active_smart_profile(), 2);

        // Cycle BIOS -> Manual -> Smart: the transient `Active=0`/`Active=3`
        // written by the intermediate arms must not clobber the slot when the
        // user returns to Smart.
        daemon.handle_command(Command::SetMode(Mode::Bios)).unwrap();
        daemon
            .handle_command(Command::SetMode(Mode::Manual))
            .unwrap();
        daemon
            .handle_command(Command::SetMode(Mode::Smart))
            .unwrap();

        assert_eq!(daemon.engine().mode(), Mode::Smart);
        assert_eq!(daemon.engine().active_smart_profile(), 2);
        assert_eq!(daemon.config().general.active, 2);

        // The persisted document keeps the user's Smart 2 selection too.
        let live_ini = daemon.config().to_ini();
        assert!(live_ini.contains("Active=2"));
    }

    #[test]
    fn daemon_applies_and_persists_set_config() {
        let path = temp_path("set-config");
        std::fs::write(&path, "Active=1\nLevel=50 2 0 0\nLevel=60 3 0 0\n").unwrap();
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        let config = Config::from_file(&path).unwrap();
        let mut daemon = Daemon::new(ec(backend), config, path.clone());

        daemon
            .handle_command(Command::SetConfig(
                "Active=2\nLevel=50 2 0 0\nLevel=60 3 0 0\n".to_string(),
            ))
            .unwrap();

        // The new document is live, applied, and written verbatim to disk.
        assert_eq!(daemon.config().general.active, 2);
        assert_eq!(daemon.engine().mode(), Mode::Smart);
        assert_eq!(control_register(&bytes), 3);
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("Active=2"));

        // The daemon's own write is not treated as an external edit.
        daemon.handle_command(Command::ReloadConfig).unwrap();
        assert_eq!(daemon.engine().mode(), Mode::Smart);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn daemon_hysteresis_toggle_preserves_live_manual_level() {
        let path = temp_path("hysteresis-manual-level");
        std::fs::write(
            &path,
            "Active=3\nManFanSpeed=1\nHysteresis=1\nLevel=50 2 0 0\nLevel=60 3 0 0\n",
        )
        .unwrap();
        let (backend, _bytes) = SharedBackend::new();
        let config = Config::from_file(&path).unwrap();
        let mut daemon = Daemon::new(ec(backend), config, path.clone());

        // Prime the watcher before changing the live runtime level.
        daemon.tick(Instant::now()).unwrap();
        daemon.handle_command(Command::SetManualLevel(5)).unwrap();
        assert_eq!(daemon.engine().manual_level(), 5);

        daemon
            .handle_command(Command::SetHysteresis(false))
            .unwrap();

        // The daemon's own write must not look like an external config edit.
        assert_eq!(daemon.engine().manual_level(), 5);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn daemon_treats_partial_sensor_failure_as_failed_iteration() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        set_byte(&bytes, 0x79, 55);
        set_byte(&bytes, REG_FAN_RPM_LOW, 0x3C);
        set_byte(&bytes, REG_FAN_RPM_HIGH, 0x0F);
        let backend = FlakyBackend::new(backend);
        let failures = Arc::clone(&backend.failing_offsets);
        let mut daemon = Daemon::new(
            ec(backend),
            smart_config(),
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );

        // Prime with a complete reading so control has a last commanded state.
        daemon.tick(Instant::now()).unwrap();
        assert_eq!(control_register(&bytes), 3);
        assert_eq!(daemon.read_errors, 0);

        // A partial sensor read is treated as a failed iteration: telemetry keeps
        // the last known value for the missing sensor, but control is not
        // evaluated and read errors are counted.
        failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(usize::from(0x79u8));
        let prev_control = control_register(&bytes);
        daemon.tick(Instant::now()).unwrap();
        assert!(daemon.last_readings.iter().any(|s| s.name == "cpu"));
        assert!(daemon.last_readings.iter().any(|s| s.name == "gpu"));
        assert_eq!(daemon.read_errors, 1);
        assert_eq!(control_register(&bytes), prev_control);

        // Once the sensor recovers, a complete sample clears the error counter
        // and control resumes normally.
        failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        daemon.tick(Instant::now()).unwrap();
        assert_eq!(control_register(&bytes), 3);
        assert_eq!(daemon.read_errors, 0);
    }

    #[test]
    fn daemon_counts_persistent_partial_sensor_failures() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        set_byte(&bytes, 0x79, 55);
        set_byte(&bytes, REG_FAN_RPM_LOW, 0x3C);
        set_byte(&bytes, REG_FAN_RPM_HIGH, 0x0F);
        let backend = FlakyBackend::new(backend);
        let failures = Arc::clone(&backend.failing_offsets);
        failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(usize::from(0x79u8));
        let mut config = smart_config();
        config.general.max_read_errors = 2;
        let mut daemon = Daemon::new(
            ec(backend),
            config,
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );

        // A chronically unreadable sensor must count toward MaxReadErrors so the
        // watchdog can revert instead of masking it indefinitely. Telemetry
        // samples between control ticks never consume the budget (it is a
        // per-Cycle budget; see `read_sensors`).
        assert!(daemon.tick(Instant::now()).is_ok());
        assert!(daemon.sample().is_ok());
        assert!(daemon.tick(Instant::now()).is_ok());
        assert!(daemon.tick(Instant::now()).is_err());
    }

    #[test]
    fn daemon_tolerates_max_read_errors_before_failing() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        set_byte(&bytes, REG_FAN_RPM_LOW, 0x3C);
        set_byte(&bytes, REG_FAN_RPM_HIGH, 0x0F);
        let backend = FlakyBackend::new(backend);
        let fail_all = Arc::clone(&backend.fail_all);
        let mut config = smart_config();
        config.general.max_read_errors = 2;
        let mut daemon = Daemon::new(
            ec(backend),
            config,
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );

        fail_all.store(100, Ordering::Relaxed);

        // Two consecutive failed control iterations are tolerated, then the
        // daemon reports an error so the watchdog can take over. Telemetry
        // samples in between never consume the budget.
        assert!(daemon.tick(Instant::now()).is_ok());
        assert!(daemon.sample().is_ok());
        assert!(daemon.tick(Instant::now()).is_ok());
        assert!(daemon.tick(Instant::now()).is_err());
    }

    #[test]
    fn daemon_counts_stuck_implausible_sensor_toward_max_read_errors() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        set_byte(&bytes, 0x79, 55);
        let mut config = smart_config();
        config.general.max_read_errors = 2;
        let mut daemon = Daemon::new(
            ec(backend),
            config,
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );

        // Prime with complete, plausible readings.
        daemon.tick(Instant::now()).unwrap();
        assert_eq!(daemon.read_errors, 0);

        // The GPU sensor stops returning a plausible reading (stuck at 0): the
        // iteration must count as failed even though the register read itself
        // succeeds, so MaxReadErrors eventually fires instead of the stale
        // cached value masking the fault forever.
        set_byte(&bytes, 0x79, 0);
        assert!(daemon.tick(Instant::now()).is_ok());
        assert!(daemon.sample().is_ok());
        assert!(daemon.tick(Instant::now()).is_ok());
        assert!(daemon.tick(Instant::now()).is_err());
    }

    #[test]
    fn daemon_counts_zero_rpm_while_commanding_nonzero_level_as_failed() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        let mut config = smart_config();
        config.general.max_read_errors = 2;
        let mut daemon = Daemon::new(
            ec(backend),
            config,
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );
        let mut state_rx = daemon.state_receiver();

        // Prime with a healthy iteration: the engine commands level 3.
        set_byte(&bytes, REG_FAN_RPM_LOW, 0x3C);
        set_byte(&bytes, REG_FAN_RPM_HIGH, 0x0F);
        daemon.tick(Instant::now()).unwrap();
        assert_eq!(daemon.read_errors, 0);
        // Level 3 maps to 3900 RPM on the E14 Gen 4 curve.
        assert_eq!(state_rx.borrow_and_update().fan_rpm, 3900);

        // The tachometer reads 0x0000 while level 3 is commanded: a stalled
        // fan, a disconnected tach, or a bad EC read. This must count as a
        // failed iteration instead of silently resetting the error streak, and
        // telemetry must retain the last known RPM.
        set_byte(&bytes, REG_FAN_RPM_LOW, 0);
        set_byte(&bytes, REG_FAN_RPM_HIGH, 0);
        daemon.tick(Instant::now()).unwrap();
        assert_eq!(daemon.read_errors, 1);
        assert_eq!(state_rx.borrow_and_update().fan_rpm, 3900);

        // The streak accumulates toward MaxReadErrors and eventually fails.
        // Telemetry samples never consume the per-Cycle budget.
        assert!(daemon.sample().is_ok());
        assert_eq!(daemon.read_errors, 1);
        assert!(daemon.tick(Instant::now()).is_ok());
        assert!(daemon.tick(Instant::now()).is_err());
    }

    #[test]
    fn daemon_accepts_zero_rpm_without_a_commanded_level() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        let mut daemon = Daemon::new(
            ec(backend),
            smart_config(),
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );
        let mut state_rx = daemon.state_receiver();

        // In BIOS auto no level is commanded, so a genuine 0 RPM tach reading
        // stays a healthy success.
        daemon.handle_command(Command::SetMode(Mode::Bios)).unwrap();
        daemon.tick(Instant::now()).unwrap();
        assert_eq!(daemon.read_errors, 0);
        assert_eq!(state_rx.borrow_and_update().fan_rpm, 0);
    }

    #[test]
    fn telemetry_failures_do_not_consume_or_reset_the_error_budget() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        set_byte(&bytes, REG_FAN_RPM_LOW, 0x3C);
        set_byte(&bytes, REG_FAN_RPM_HIGH, 0x0F);
        let backend = FlakyBackend::new(backend);
        let fail_all = Arc::clone(&backend.fail_all);
        let mut config = smart_config();
        config.general.max_read_errors = 2;
        let mut daemon = Daemon::new(
            ec(backend),
            config,
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );

        fail_all.store(u32::MAX, Ordering::Relaxed);

        // Telemetry samples fail at 4 Hz but must never consume the control
        // budget: ten failing samples in a row leave the counter untouched.
        for _ in 0..10 {
            assert!(daemon.sample().is_ok());
            assert_eq!(daemon.read_errors, 0);
        }

        // Only consecutive failed *control* iterations count toward
        // MaxReadErrors: two are tolerated, the third aborts.
        assert!(daemon.tick(Instant::now()).is_ok());
        assert_eq!(daemon.read_errors, 1);
        // A successful telemetry sample must not mask a failing control path.
        fail_all.store(0, Ordering::Relaxed);
        assert!(daemon.sample().is_ok());
        fail_all.store(u32::MAX, Ordering::Relaxed);
        assert_eq!(daemon.read_errors, 1);
        assert!(daemon.tick(Instant::now()).is_ok());
        assert!(daemon.tick(Instant::now()).is_err());
    }

    #[test]
    fn daemon_reloads_config_from_disk() {
        let path = temp_path("reload");
        std::fs::write(&path, "Level=50 2 0 0\nLevel=60 3 0 0\n").unwrap();
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        let config = Config::from_file(&path).unwrap();
        let mut daemon = Daemon::new(ec(backend), config, path.clone());

        daemon
            .handle_command(Command::SetMode(Mode::Smart))
            .unwrap();
        assert_eq!(control_register(&bytes), 3);

        // Lower the top threshold to level 2 and reload.
        std::fs::write(&path, "Level=50 2 0 0\nLevel=60 2 0 0\n").unwrap();
        daemon.handle_command(Command::ReloadConfig).unwrap();
        assert_eq!(control_register(&bytes), 2);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn daemon_keeps_last_good_config_when_reload_fails() {
        let path = temp_path("invalid-reload");
        std::fs::write(&path, "Level=50 2 0 0\nLevel=60 3 0 0\n").unwrap();
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        let config = Config::from_file(&path).unwrap();
        let mut daemon = Daemon::new(ec(backend), config, path.clone());

        daemon
            .handle_command(Command::SetMode(Mode::Smart))
            .unwrap();
        assert_eq!(control_register(&bytes), 3);

        // An unparseable file must not stop the loop or discard the last good
        // configuration.
        std::fs::write(&path, "not a valid configuration\n").unwrap();
        daemon.handle_command(Command::ReloadConfig).unwrap();
        assert_eq!(daemon.engine().mode(), Mode::Smart);
        assert_eq!(control_register(&bytes), 3);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn daemon_reverts_to_bios_auto_on_drop() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        let mut daemon = Daemon::new(
            ec(backend),
            smart_config(),
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );
        daemon
            .handle_command(Command::SetMode(Mode::Smart))
            .unwrap();
        assert_eq!(control_register(&bytes), 3);

        drop(daemon);
        assert_eq!(control_register(&bytes), FAN_BIOS_AUTO);
    }

    #[test]
    fn control_service_round_trips_through_the_daemon() {
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 60);
        let mut daemon = Daemon::new(
            ec(backend),
            smart_config(),
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );
        let service = daemon.control_service();

        service.set_mode("smart").unwrap();
        // Drain the queued command through the daemon.
        let command = daemon.commands.try_recv().unwrap();
        daemon.handle_command(command).unwrap();

        assert_eq!(control_register(&bytes), 3);
        assert_eq!(service.get_state().mode, "smart");
    }

    #[test]
    fn telemetry_interval_is_clamped_to_at_least_one_millisecond() {
        let (backend, _bytes) = SharedBackend::new();
        let daemon = Daemon::new(
            ec(backend),
            smart_config(),
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        )
        .with_telemetry_interval(Duration::ZERO);
        assert_eq!(daemon.telemetry_interval, Duration::from_millis(1));

        let (backend, _bytes) = SharedBackend::new();
        let daemon = Daemon::new(
            ec(backend),
            smart_config(),
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        )
        .with_telemetry_interval(Duration::from_secs(2));
        assert_eq!(daemon.telemetry_interval, Duration::from_secs(2));
    }

    #[test]
    fn control_interval_uses_config_cycle_unless_overridden() {
        let (backend, _bytes) = SharedBackend::new();
        let mut config = smart_config();
        config.general.cycle = 17;
        let daemon = Daemon::new(
            ec(backend),
            config,
            PathBuf::from("/nonexistent/omafanctrl.ini"),
        );

        assert_eq!(daemon.control_interval(None), Duration::from_secs(17));
        assert_eq!(
            daemon.control_interval(Some(Duration::from_secs(2))),
            Duration::from_secs(2)
        );
    }

    #[tokio::test]
    async fn control_deadline_is_independent_of_the_telemetry_interval() {
        let path = temp_path("run-control");
        std::fs::write(&path, "Level=50 2 0 0\nLevel=60 3 0 0\n").unwrap();
        let (backend, bytes) = SharedBackend::new();
        set_byte(&bytes, 0x78, 50);
        set_byte(&bytes, REG_FAN_RPM_LOW, 0x3C);
        set_byte(&bytes, REG_FAN_RPM_HIGH, 0x0F);
        let config = Config::from_file(&path).unwrap();
        let mut daemon = Daemon::new(ec(backend), config, path.clone())
            // A telemetry interval far longer than the control interval must not
            // delay a control decision.
            .with_telemetry_interval(Duration::from_secs(5));
        // Prime the watcher so the first control tick does not reload the
        // configuration (and reset the test's dwell policy) underneath us.
        daemon.tick(Instant::now()).unwrap();
        let mut policy = daemon.engine().policy().clone();
        policy.min_dwell = Duration::ZERO;
        daemon.engine.set_policy(policy);

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let loop_task = tokio::spawn(async move {
            daemon
                .run(Some(Duration::from_millis(20)), shutdown_rx)
                .await
        });

        let wait_for = |expected: u8| {
            let bytes = Arc::clone(&bytes);
            async move {
                let deadline = Instant::now() + Duration::from_secs(1);
                while control_register(&bytes) != expected && Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                control_register(&bytes)
            }
        };

        // The first control tick runs immediately and selects level 2.
        assert_eq!(wait_for(2).await, 2);

        // A temperature change is picked up by the next control deadline even
        // though the telemetry ticker will not fire for another five seconds.
        set_byte(&bytes, 0x78, 60);
        assert_eq!(wait_for(3).await, 3);

        shutdown_tx.send(true).unwrap();
        loop_task.await.unwrap().unwrap();
        std::fs::remove_file(&path).ok();
    }
}
