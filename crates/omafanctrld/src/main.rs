//! `omafanctrld` — the privileged `omafanctrl` daemon.
//!
//! Runs as a systemd system service, owns the EC file handle, runs the control
//! loop, and exposes the control API over the D-Bus system bus.
//!
//! The daemon is the only component that writes to the EC. It holds a
//! [`Watchdog`](omafanctrl_core::engine::Watchdog) that reverts the fan to BIOS
//! auto control when the process exits or panics; the systemd unit adds an
//! `ExecStopPost` safety net (`omafanctrld --revert`) for `kill -9`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use omafanctrl_core::config::Config;
use omafanctrl_core::daemon::Daemon;
use omafanctrl_core::dbus::{self, Control, State};
use omafanctrl_core::ec::{AccessMode, Ec, FileBackend};
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

/// The default configuration path.
const DEFAULT_CONFIG_PATH: &str = "/etc/omafanctrl/TPFanControl.ini";

/// The default control-loop interval, in seconds.
const DEFAULT_INTERVAL_SECS: u64 = 5;

/// Command-line arguments for the daemon.
#[derive(Debug, Parser)]
#[command(name = "omafanctrld", version, about = "omafanctrl fan control daemon")]
struct Args {
    /// Path to the TPFanCtrl2-compatible configuration file.
    #[arg(long, short, default_value = DEFAULT_CONFIG_PATH)]
    config: PathBuf,

    /// Control-loop interval, in seconds.
    #[arg(long, default_value_t = DEFAULT_INTERVAL_SECS)]
    interval: u64,

    /// Revert the fan to BIOS auto control and exit.
    ///
    /// Used by the systemd unit's `ExecStopPost` as a safety net for `kill -9`.
    #[arg(long)]
    revert: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    init_tracing();

    if args.revert {
        return revert_to_bios_auto();
    }

    let config = load_config(&args.config);
    let backend = FileBackend::open(AccessMode::ReadWrite)
        .context("failed to open the EC; is `ec_sys write_support=1` loaded?")?;
    let ec = Ec::new(backend);

    let mut daemon = Daemon::new(ec, config, args.config.clone());
    let control = Control::new(daemon.control_service());

    let connection = dbus::serve(control)
        .await
        .context("failed to serve the D-Bus interface on the system bus")?;
    tracing::info!(name = dbus::BUS_NAME, "serving the control interface");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Forward state and configuration changes as D-Bus signals.
    let signal_task = tokio::spawn(forward_signals(
        connection.clone(),
        daemon.state_receiver(),
        daemon.config_receiver(),
    ));

    // Run the control loop. Dropping the daemon (when this task ends) reverts
    // the fan to BIOS auto control.
    let interval = Duration::from_secs(args.interval.max(1));
    let loop_task = tokio::spawn(async move {
        if let Err(error) = daemon.run(interval, shutdown_rx).await {
            tracing::error!(%error, "control loop stopped");
        }
    });

    shutdown_signal().await;
    tracing::info!("shutting down");
    let _ = shutdown_tx.send(true);

    let _ = loop_task.await;
    signal_task.abort();
    Ok(())
}

/// Initialise tracing from `RUST_LOG`, defaulting to `info`.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Load the configuration, falling back to defaults on error.
fn load_config(path: &Path) -> Config {
    match Config::from_file(path) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(
                %error,
                path = %path.display(),
                "failed to load configuration; using defaults"
            );
            Config::default()
        }
    }
}

/// Open the EC and hand the fan back to the firmware.
fn revert_to_bios_auto() -> Result<()> {
    let backend = FileBackend::open(AccessMode::ReadWrite)
        .context("failed to open the EC for the safety revert")?;
    let mut ec = Ec::new(backend);
    ec.set_fan_auto()
        .context("failed to revert the fan to BIOS auto")?;
    tracing::info!("fan reverted to BIOS auto control");
    Ok(())
}

/// Forward state and configuration changes as D-Bus signals.
async fn forward_signals(
    connection: zbus::Connection,
    mut state_rx: watch::Receiver<State>,
    mut config_rx: watch::Receiver<String>,
) {
    loop {
        tokio::select! {
            result = state_rx.changed() => {
                if result.is_err() {
                    break;
                }
                let state = state_rx.borrow_and_update().clone();
                if let Err(error) = dbus::emit_state_changed(&connection, state).await {
                    tracing::warn!(%error, "failed to emit StateChanged");
                }
            }
            result = config_rx.changed() => {
                if result.is_err() {
                    break;
                }
                let contents = config_rx.borrow_and_update().clone();
                if let Err(error) = dbus::emit_config_changed(&connection, contents).await {
                    tracing::warn!(%error, "failed to emit ConfigChanged");
                }
            }
        }
    }
}

/// Resolve when the process receives `SIGINT` or `SIGTERM`.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install the Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install the SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
