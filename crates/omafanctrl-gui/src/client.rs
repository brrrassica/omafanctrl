//! Background D-Bus client for the GUI.
//!
//! GTK runs its own main loop, so the D-Bus work happens on a dedicated thread
//! with a small Tokio runtime. The thread subscribes to the daemon's
//! `StateChanged` and `ConfigChanged` signals, polls `GetState`/`GetConfig` as a
//! fallback, and forwards updates to the UI over an [`async_channel`]. Commands
//! travel the other way.

use std::time::Duration;

use async_channel::{Receiver, Sender};
use futures_util::StreamExt;
use omafanctrl_core::dbus::{BUS_NAME, INTERFACE_NAME, OBJECT_PATH, State};

/// A command sent from the UI to the background client.
#[derive(Debug, Clone)]
pub enum Command {
    /// Switch the active mode.
    SetMode(String),
    /// Set the manual fan level.
    SetManualLevel(u8),
    /// Enable or disable smart-mode hysteresis.
    SetHysteresis(bool),
    /// Re-read the configuration from disk.
    ReloadConfig,
    /// Replace the configuration.
    SetConfig(String),
}

/// An update sent from the background client to the UI.
#[derive(Debug, Clone)]
pub enum Update {
    /// A fresh state snapshot.
    State(State),
    /// The current configuration as `.ini` text.
    Config(String),
    /// The daemon could not be reached or returned an error.
    Error(String),
}

/// A handle to the background client.
#[derive(Clone)]
pub struct ClientHandle {
    /// Updates from the daemon.
    pub updates: Receiver<Update>,
    /// Commands to the daemon.
    pub commands: Sender<Command>,
}

impl ClientHandle {
    /// Queue a command, ignoring a closed channel.
    pub fn send(&self, command: Command) {
        let _ = self.commands.try_send(command);
    }
}

/// Spawn the background client thread and return a handle to it.
pub fn spawn() -> ClientHandle {
    let (update_tx, update_rx) = async_channel::unbounded();
    let (command_tx, command_rx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("omafanctrl-dbus".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = update_tx.send_blocking(Update::Error(format!(
                        "failed to start the async runtime: {error}"
                    )));
                    return;
                }
            };
            runtime.block_on(run(update_tx, command_rx));
        })
        .expect("failed to spawn the D-Bus client thread");
    ClientHandle {
        updates: update_rx,
        commands: command_tx,
    }
}

/// The background event loop.
async fn run(updates: Sender<Update>, commands: Receiver<Command>) {
    let connection = match zbus::Connection::system().await {
        Ok(connection) => connection,
        Err(error) => {
            let _ = updates
                .send(Update::Error(format!(
                    "failed to connect to the system bus: {error}"
                )))
                .await;
            return;
        }
    };
    let proxy = match zbus::Proxy::new(&connection, BUS_NAME, OBJECT_PATH, INTERFACE_NAME).await {
        Ok(proxy) => proxy,
        Err(error) => {
            let _ = updates
                .send(Update::Error(format!(
                    "the omafanctrl daemon is not available: {error}"
                )))
                .await;
            return;
        }
    };

    let mut state_signals = match proxy.receive_signal("StateChanged").await {
        Ok(signals) => signals,
        Err(error) => {
            let _ = updates
                .send(Update::Error(format!(
                    "failed to subscribe to StateChanged: {error}"
                )))
                .await;
            return;
        }
    };
    let mut config_signals = match proxy.receive_signal("ConfigChanged").await {
        Ok(signals) => signals,
        Err(error) => {
            let _ = updates
                .send(Update::Error(format!(
                    "failed to subscribe to ConfigChanged: {error}"
                )))
                .await;
            return;
        }
    };

    refresh_state(&proxy, &updates).await;
    refresh_config(&proxy, &updates).await;

    let mut ticker = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                refresh_state(&proxy, &updates).await;
            }
            Some(message) = state_signals.next() => {
                match message.body().deserialize::<State>() {
                    Ok(state) => {
                        let _ = updates.send(Update::State(state)).await;
                    }
                    Err(error) => {
                        let _ = updates
                            .send(Update::Error(format!("malformed StateChanged: {error}")))
                            .await;
                    }
                }
            }
            Some(message) = config_signals.next() => {
                match message.body().deserialize::<String>() {
                    Ok(config) => {
                        let _ = updates.send(Update::Config(config)).await;
                    }
                    Err(error) => {
                        let _ = updates
                            .send(Update::Error(format!("malformed ConfigChanged: {error}")))
                            .await;
                    }
                }
            }
            Ok(command) = commands.recv() => {
                if let Err(error) = apply(&proxy, command).await {
                    let _ = updates.send(Update::Error(error)).await;
                }
                refresh_state(&proxy, &updates).await;
                refresh_config(&proxy, &updates).await;
            }
        }
    }
}

/// Fetch the current state and forward it.
async fn refresh_state(proxy: &zbus::Proxy<'_>, updates: &Sender<Update>) {
    match proxy.call::<_, _, State>("GetState", &()).await {
        Ok(state) => {
            let _ = updates.send(Update::State(state)).await;
        }
        Err(error) => {
            let _ = updates
                .send(Update::Error(format!(
                    "failed to read the daemon state: {error}"
                )))
                .await;
        }
    }
}

/// Fetch the current configuration and forward it.
async fn refresh_config(proxy: &zbus::Proxy<'_>, updates: &Sender<Update>) {
    match proxy.call::<_, _, String>("GetConfig", &()).await {
        Ok(config) => {
            let _ = updates.send(Update::Config(config)).await;
        }
        Err(error) => {
            let _ = updates
                .send(Update::Error(format!(
                    "failed to read the configuration: {error}"
                )))
                .await;
        }
    }
}

/// Apply a command to the daemon.
async fn apply(proxy: &zbus::Proxy<'_>, command: Command) -> Result<(), String> {
    let result = match command {
        Command::SetMode(mode) => proxy.call::<_, _, ()>("SetMode", &(mode,)).await,
        Command::SetManualLevel(level) => proxy.call::<_, _, ()>("SetManualLevel", &(level,)).await,
        Command::SetHysteresis(enabled) => {
            proxy.call::<_, _, ()>("SetHysteresis", &(enabled,)).await
        }
        Command::ReloadConfig => proxy.call::<_, _, ()>("ReloadConfig", &()).await,
        Command::SetConfig(contents) => proxy.call::<_, _, ()>("SetConfig", &(contents,)).await,
    };
    result.map_err(|error| error.to_string())
}
