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

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};

use crate::config::{Config, ConfigWatcher};
use crate::dbus::{Command, ControlService, SensorState, State};
use crate::ec::{Ec, EcBackend, EcError};
use crate::engine::{
    Engine, EngineError, EnginePolicy, SensorReading, Watchdog, apply_decision,
    read_sensor_readings,
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
}

impl<B: EcBackend> Daemon<B> {
    /// Create a daemon around an EC handle and an initial configuration.
    pub fn new(ec: Ec<B>, config: Config, config_path: PathBuf) -> Self {
        let engine = Engine::new(EnginePolicy::from_config(&config));
        let watcher = ConfigWatcher::new(config_path.clone());
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
        }
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
    fn read_sensors(&mut self) -> Result<(Vec<SensorReading>, u16), EngineError> {
        let readings = read_sensor_readings(self.watchdog.ec_mut(), &self.sensor_offsets)?;
        let fan_rpm = self.watchdog.ec_mut().read_fan_rpm()?;
        Ok((readings, fan_rpm))
    }

    /// Run one control iteration.
    pub fn tick(&mut self, now: Instant) -> Result<(), EngineError> {
        self.poll_config();
        let (readings, fan_rpm) = self.read_sensors()?;
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
        let (readings, fan_rpm) = self.read_sensors()?;
        self.publish_state(&readings, fan_rpm);
        Ok(())
    }

    /// Apply a command from the D-Bus layer, then re-evaluate immediately.
    pub fn handle_command(&mut self, command: Command) -> Result<(), EngineError> {
        match command {
            Command::SetMode(mode) => self.engine.set_mode(mode),
            Command::SetManualLevel(level) => self.engine.set_manual_level(level),
            Command::SetHysteresis(enabled) => self.engine.set_hysteresis_enabled(enabled),
            Command::ReloadConfig => {
                let config = Config::from_file(&self.config_path).map_err(EngineError::Config)?;
                self.apply_config(config);
            }
        }
        // An explicit command should take effect immediately, bypassing the
        // anti-oscillation dwell timer.
        self.engine.clear_dwell();
        self.tick(Instant::now())
    }

    /// Run the control loop until `shutdown` is signalled or the command channel
    /// closes.
    pub async fn run(
        &mut self,
        interval: Duration,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), EngineError> {
        // Sample and publish at the fast telemetry cadence; evaluate the fan
        // control decision only once per `interval`.
        let mut ticker = tokio::time::interval(TELEMETRY_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_control: Option<Instant> = None;
        loop {
            let received = tokio::select! {
                _ = ticker.tick() => None,
                command = self.commands.recv() => Some(command),
                result = shutdown.changed() => {
                    if result.is_err() || *shutdown.borrow() {
                        break;
                    }
                    continue;
                }
            };
            match received {
                None => {
                    let now = Instant::now();
                    let control_due = last_control.is_none_or(|last| {
                        now.saturating_duration_since(last) >= interval
                    });
                    if control_due {
                        self.tick(now)?;
                        last_control = Some(now);
                    } else {
                        self.sample()?;
                    }
                }
                Some(Some(command)) => {
                    self.handle_command(command)?;
                    last_control = Some(Instant::now());
                }
                Some(None) => break,
            }
        }
        Ok(())
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
    fn apply_config(&mut self, config: Config) {
        self.engine.set_policy(EnginePolicy::from_config(&config));
        self.engine.reset();
        let ini = config.to_ini();
        self.config = config;
        let _ = self.config_tx.send(ini);
    }

    /// Publish a state snapshot for the D-Bus layer.
    fn publish_state(&self, readings: &[SensorReading], fan_rpm: u16) {
        let state = State {
            mode: self.engine.mode().as_str().to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ec::{EC_WINDOW_SIZE, EcError, FAN_BIOS_AUTO, REG_FAN_CONTROL};
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
    fn ec(backend: SharedBackend) -> Ec<SharedBackend> {
        Ec::new(backend).with_min_write_interval(Duration::ZERO)
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
    fn daemon_applies_manual_level() {
        let (backend, bytes) = SharedBackend::new();
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
}
