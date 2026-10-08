//! D-Bus interface for the `omafanctrl` daemon.
//!
//! The daemon owns the well-known system-bus name [`BUS_NAME`] and serves the
//! [`Control`] interface at [`OBJECT_PATH`]. The interface exposes:
//!
//! | Method | Purpose |
//! | --- | --- |
//! | `GetState` | A [`State`] snapshot (mode, levels, RPM, temperatures) |
//! | `SetMode` | Switch between `bios`, `manual`, and `smart` |
//! | `SetManualLevel` | Set the manual fan level |
//! | `SetHysteresis` | Enable or disable smart-mode hysteresis |
//! | `GetConfig` | The current configuration as `.ini` text |
//! | `SetConfig` | Validate, persist, and apply new `.ini` text |
//! | `ReloadConfig` | Re-read the configuration file from disk |
//!
//! and the signals `StateChanged` and `ConfigChanged`.
//!
//! The transport-agnostic [`ControlService`] holds the shared state and a
//! command channel to the control loop. [`Control`] is a thin zbus wrapper, so
//! the service can be unit-tested without a bus.

use std::path::{Path, PathBuf};

use tokio::sync::{mpsc, watch};

use crate::config::{Config, ConfigError};
use crate::engine::Mode;

/// The well-known D-Bus name owned by the daemon.
pub const BUS_NAME: &str = "org.omarchy.omafanctrl";

/// The object path the control interface is served at.
pub const OBJECT_PATH: &str = "/org/omarchy/omafanctrl";

/// The D-Bus interface name.
pub const INTERFACE_NAME: &str = "org.omarchy.omafanctrl";

/// A snapshot of a single sensor, as exposed over D-Bus.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, zbus::zvariant::Type,
)]
pub struct SensorState {
    /// The sensor name.
    pub name: String,
    /// The EC register offset the reading came from.
    pub offset: u8,
    /// The temperature in degrees Celsius.
    pub celsius: u8,
}

/// A snapshot of the daemon state, as exposed over D-Bus.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, zbus::zvariant::Type,
)]
pub struct State {
    /// The active mode: `bios`, `manual`, or `smart`.
    pub mode: String,
    /// The configured manual fan level.
    pub manual_level: u8,
    /// The last commanded fan level (`0` when the fan is under BIOS control).
    pub current_level: u8,
    /// Whether the fan is currently under BIOS control.
    pub bios_auto: bool,
    /// The realtime fan speed in RPM.
    pub fan_rpm: u16,
    /// Whether smart-mode hysteresis is enabled.
    pub hysteresis_enabled: bool,
    /// The latest sensor readings.
    pub temperatures: Vec<SensorState>,
    /// The path of the active configuration file.
    pub config_path: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            mode: Mode::Bios.as_str().to_string(),
            manual_level: 1,
            current_level: 0,
            bios_auto: true,
            fan_rpm: 0,
            hysteresis_enabled: true,
            temperatures: Vec::new(),
            config_path: String::new(),
        }
    }
}

/// A command sent from the D-Bus layer to the control loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Switch the active mode.
    SetMode(Mode),
    /// Set the manual fan level.
    SetManualLevel(u8),
    /// Enable or disable smart-mode hysteresis.
    SetHysteresis(bool),
    /// Re-read the configuration file from disk.
    ReloadConfig,
}

/// Errors returned by [`ControlService`].
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// The requested mode name is not recognised.
    #[error("unknown mode `{0}`; expected `bios`, `manual`, or `smart`")]
    UnknownMode(String),

    /// The control loop is no longer accepting commands.
    #[error("the daemon is not accepting commands")]
    ChannelClosed,

    /// The supplied configuration is invalid.
    #[error(transparent)]
    Config(#[from] ConfigError),

    /// The configuration file could not be written.
    #[error("I/O error writing config `{path}`: {source}")]
    Io {
        /// The path that was written.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Parse a mode name into a [`Mode`].
pub fn parse_mode(value: &str) -> Result<Mode, ServiceError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "bios" | "auto" => Ok(Mode::Bios),
        "manual" => Ok(Mode::Manual),
        "smart" => Ok(Mode::Smart),
        other => Err(ServiceError::UnknownMode(other.to_string())),
    }
}

/// The transport-agnostic control service.
///
/// It reads the latest [`State`] and configuration from watch channels and
/// forwards mutations to the control loop over a command channel.
#[derive(Debug, Clone)]
pub struct ControlService {
    state: watch::Receiver<State>,
    config: watch::Receiver<String>,
    commands: mpsc::Sender<Command>,
    config_path: PathBuf,
}

impl ControlService {
    /// Create a service from its channels and configuration path.
    pub fn new(
        state: watch::Receiver<State>,
        config: watch::Receiver<String>,
        commands: mpsc::Sender<Command>,
        config_path: PathBuf,
    ) -> Self {
        Self {
            state,
            config,
            commands,
            config_path,
        }
    }

    /// The latest state snapshot.
    pub fn get_state(&self) -> State {
        self.state.borrow().clone()
    }

    /// The current configuration as `.ini` text.
    pub fn get_config(&self) -> String {
        self.config.borrow().clone()
    }

    /// The path of the active configuration file.
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Switch the active mode.
    pub fn set_mode(&self, mode: &str) -> Result<(), ServiceError> {
        let mode = parse_mode(mode)?;
        self.send(Command::SetMode(mode))
    }

    /// Set the manual fan level.
    pub fn set_manual_level(&self, level: u8) -> Result<(), ServiceError> {
        self.send(Command::SetManualLevel(level))
    }

    /// Enable or disable smart-mode hysteresis.
    pub fn set_hysteresis(&self, enabled: bool) -> Result<(), ServiceError> {
        self.send(Command::SetHysteresis(enabled))
    }

    /// Validate, persist, and apply new configuration text.
    ///
    /// The configuration is validated before it is written, so an invalid
    /// document never reaches disk.
    pub fn set_config(&self, contents: &str) -> Result<(), ServiceError> {
        let _validated: Config = contents.parse()?;
        std::fs::write(&self.config_path, contents).map_err(|source| ServiceError::Io {
            path: self.config_path.clone(),
            source,
        })?;
        self.reload_config()
    }

    /// Re-read the configuration file from disk.
    pub fn reload_config(&self) -> Result<(), ServiceError> {
        self.send(Command::ReloadConfig)
    }

    fn send(&self, command: Command) -> Result<(), ServiceError> {
        self.commands
            .try_send(command)
            .map_err(|_| ServiceError::ChannelClosed)
    }
}

/// The zbus interface object served on the system bus.
#[derive(Debug, Clone)]
pub struct Control {
    service: ControlService,
}

impl Control {
    /// Wrap a [`ControlService`] in the D-Bus interface.
    pub fn new(service: ControlService) -> Self {
        Self { service }
    }

    /// The underlying service.
    pub fn service(&self) -> &ControlService {
        &self.service
    }
}

#[zbus::interface(name = "org.omarchy.omafanctrl")]
impl Control {
    /// Return a snapshot of the daemon state.
    async fn get_state(&self) -> zbus::fdo::Result<State> {
        Ok(self.service.get_state())
    }

    /// Switch the active mode (`bios`, `manual`, or `smart`).
    async fn set_mode(&self, mode: String) -> zbus::fdo::Result<()> {
        self.service.set_mode(&mode).map_err(to_fdo)
    }

    /// Set the manual fan level.
    async fn set_manual_level(&self, level: u8) -> zbus::fdo::Result<()> {
        self.service.set_manual_level(level).map_err(to_fdo)
    }

    /// Enable or disable smart-mode hysteresis.
    async fn set_hysteresis(&self, enabled: bool) -> zbus::fdo::Result<()> {
        self.service.set_hysteresis(enabled).map_err(to_fdo)
    }

    /// Return the current configuration as `.ini` text.
    async fn get_config(&self) -> zbus::fdo::Result<String> {
        Ok(self.service.get_config())
    }

    /// Validate, persist, and apply new configuration text.
    async fn set_config(&self, contents: String) -> zbus::fdo::Result<()> {
        self.service.set_config(&contents).map_err(to_fdo)
    }

    /// Re-read the configuration file from disk.
    async fn reload_config(&self) -> zbus::fdo::Result<()> {
        self.service.reload_config().map_err(to_fdo)
    }

    /// Emitted whenever the daemon state changes.
    #[zbus(signal)]
    async fn state_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        state: State,
    ) -> zbus::Result<()>;

    /// Emitted whenever the configuration changes.
    #[zbus(signal)]
    async fn config_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        contents: String,
    ) -> zbus::Result<()>;
}

/// Convert a [`ServiceError`] into a D-Bus failure.
fn to_fdo(error: ServiceError) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(error.to_string())
}

/// Serve the control interface on the system bus.
pub async fn serve(control: Control) -> zbus::Result<zbus::Connection> {
    zbus::connection::Builder::system()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, control)?
        .build()
        .await
}

/// Emit the `StateChanged` signal.
pub async fn emit_state_changed(connection: &zbus::Connection, state: State) -> zbus::Result<()> {
    let interface = connection
        .object_server()
        .interface::<_, Control>(OBJECT_PATH)
        .await?;
    Control::state_changed(interface.signal_emitter(), state).await
}

/// Emit the `ConfigChanged` signal.
pub async fn emit_config_changed(
    connection: &zbus::Connection,
    contents: String,
) -> zbus::Result<()> {
    let interface = connection
        .object_server()
        .interface::<_, Control>(OBJECT_PATH)
        .await?;
    Control::config_changed(interface.signal_emitter(), contents).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(
        config_path: PathBuf,
    ) -> (
        ControlService,
        watch::Sender<State>,
        mpsc::Receiver<Command>,
    ) {
        let (state_tx, state_rx) = watch::channel(State::default());
        let (_config_tx, config_rx) = watch::channel(String::new());
        let (command_tx, command_rx) = mpsc::channel(16);
        (
            ControlService::new(state_rx, config_rx, command_tx, config_path),
            state_tx,
            command_rx,
        )
    }

    #[test]
    fn parses_known_modes() {
        assert_eq!(parse_mode("bios").unwrap(), Mode::Bios);
        assert_eq!(parse_mode("AUTO").unwrap(), Mode::Bios);
        assert_eq!(parse_mode(" manual ").unwrap(), Mode::Manual);
        assert_eq!(parse_mode("Smart").unwrap(), Mode::Smart);
    }

    #[test]
    fn rejects_unknown_modes() {
        assert!(matches!(
            parse_mode("turbo"),
            Err(ServiceError::UnknownMode(mode)) if mode == "turbo"
        ));
    }

    #[test]
    fn get_state_reflects_the_watch_channel() {
        let (service, state_tx, _command_rx) = service(PathBuf::from("/tmp/omafanctrl-test.ini"));
        assert_eq!(service.get_state().mode, "bios");
        let _ = state_tx.send(State {
            mode: "smart".to_string(),
            current_level: 3,
            bios_auto: false,
            ..State::default()
        });
        let state = service.get_state();
        assert_eq!(state.mode, "smart");
        assert_eq!(state.current_level, 3);
        assert!(!state.bios_auto);
    }

    #[test]
    fn set_config_rejects_invalid_documents_without_writing() {
        let path = std::env::temp_dir().join(format!(
            "omafanctrl-dbus-invalid-{}.ini",
            std::process::id()
        ));
        let (service, _state_tx, _command_rx) = service(path.clone());
        let error = service.set_config("Active=99\n").unwrap_err();
        assert!(matches!(error, ServiceError::Config(_)));
        assert!(!path.exists());
    }

    #[test]
    fn set_config_writes_valid_documents() {
        let path =
            std::env::temp_dir().join(format!("omafanctrl-dbus-valid-{}.ini", std::process::id()));
        let (service, _state_tx, _command_rx) = service(path.clone());
        service
            .set_config("Active=1\nLevel=50 2 0 0\n")
            .expect("valid config should be written");
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("Active=1"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn commands_fail_when_the_loop_is_gone() {
        let (state_tx, state_rx) = watch::channel(State::default());
        let (_config_tx, config_rx) = watch::channel(String::new());
        let (command_tx, command_rx) = mpsc::channel(1);
        drop(command_rx);
        let service = ControlService::new(
            state_rx,
            config_rx,
            command_tx,
            PathBuf::from("/tmp/omafanctrl-test.ini"),
        );
        drop(state_tx);
        assert!(matches!(
            service.set_mode("smart"),
            Err(ServiceError::ChannelClosed)
        ));
    }
}
