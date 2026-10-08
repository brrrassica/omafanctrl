//! D-Bus client for the `omafanctrl` daemon.
//!
//! Every call is wrapped in a timeout so a hotkey binding can never hang: if the
//! daemon is unresponsive or a polkit prompt is left unanswered, the command
//! fails fast with a clear message instead of blocking the compositor.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use omafanctrl_core::dbus::{BUS_NAME, INTERFACE_NAME, OBJECT_PATH, State};

/// A client for the daemon's control interface.
pub struct Client {
    proxy: zbus::Proxy<'static>,
    timeout: Duration,
}

impl Client {
    /// Connect to the system bus and resolve the control interface.
    pub async fn connect(timeout: Duration) -> Result<Self> {
        let connection = zbus::Connection::system()
            .await
            .context("failed to connect to the D-Bus system bus")?;
        let proxy = zbus::Proxy::new(&connection, BUS_NAME, OBJECT_PATH, INTERFACE_NAME)
            .await
            .context("failed to resolve the omafanctrl interface; is omafanctrld running?")?;
        Ok(Self { proxy, timeout })
    }

    /// Fetch the current state.
    pub async fn get_state(&self) -> Result<State> {
        self.call("GetState", &()).await
    }

    /// Set the mode (`bios`, `manual`, or `smart`).
    pub async fn set_mode(&self, mode: &str) -> Result<()> {
        self.call("SetMode", &(mode.to_string(),)).await
    }

    /// Set the manual fan level.
    pub async fn set_manual_level(&self, level: u8) -> Result<()> {
        self.call("SetManualLevel", &(level,)).await
    }

    /// Enable or disable smart-mode hysteresis.
    pub async fn set_hysteresis(&self, enabled: bool) -> Result<()> {
        self.call("SetHysteresis", &(enabled,)).await
    }

    /// Fetch the configuration as `.ini` text.
    pub async fn get_config(&self) -> Result<String> {
        self.call("GetConfig", &()).await
    }

    /// Replace the configuration.
    pub async fn set_config(&self, contents: &str) -> Result<()> {
        self.call("SetConfig", &(contents.to_string(),)).await
    }

    /// Reload the configuration from disk.
    pub async fn reload_config(&self) -> Result<()> {
        self.call("ReloadConfig", &()).await
    }

    /// Call a method with a timeout and a friendly error message.
    async fn call<R, B>(&self, method: &str, body: &B) -> Result<R>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        let future = self.proxy.call(method, body);
        let result = tokio::time::timeout(self.timeout, future)
            .await
            .map_err(|_| anyhow!("timed out after {:?} waiting for the daemon", self.timeout))?;
        result.map_err(|error| anyhow!(describe_error(&error)))
    }
}

/// Turn a zbus error into a message that is useful in a hotkey context.
fn describe_error(error: &zbus::Error) -> String {
    if let zbus::Error::MethodError(name, _, _) = error {
        return describe_method_error(name.as_str());
    }
    error.to_string()
}

/// Describe a D-Bus method error by its name.
fn describe_method_error(name: &str) -> String {
    if name.contains("NotAuthorized") || name.contains("AccessDenied") {
        "authorization denied: authenticate when prompted, or grant the polkit \
         action org.omarchy.omafanctrl.control"
            .to_string()
    } else {
        format!("the daemon rejected the request: {name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_authorization_errors() {
        let message = describe_method_error("org.freedesktop.PolicyKit1.Error.NotAuthorized");
        assert!(message.contains("authorization denied"));
        let message = describe_method_error("org.freedesktop.DBus.Error.AccessDenied");
        assert!(message.contains("authorization denied"));
    }

    #[test]
    fn describes_other_method_errors() {
        let message = describe_method_error("org.omarchy.omafanctrl.Error.UnknownMode");
        assert!(message.contains("UnknownMode"));
    }
}
