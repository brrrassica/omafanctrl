//! `thinkpad_acpi` fan interface.
//!
//! The `thinkpad_acpi` kernel module exposes a small, human-readable fan
//! interface at `/proc/acpi/ibm/fan`:
//!
//! ```text
//! status:         enabled
//! speed:          3900
//! level:          7
//! ```
//!
//! This is the interface used to observe the E14 Gen 4 fan curve (see
//! `config/E14G4-quirks`). It is a **read-only, realtime** source of the fan
//! speed and is complementary to the EC tachometer registers
//! ([`crate::ec::REG_FAN_RPM_LOW`]/[`crate::ec::REG_FAN_RPM_HIGH`]):
//!
//! - The EC tachometer is read through `ec_sys` and requires `write_support=1`
//!   and root, but is the same source the control engine writes to.
//! - `/proc/acpi/ibm/fan` is provided by `thinkpad_acpi`, is world-readable, and
//!   reports the firmware's own view of the fan (including `level: auto`).
//!
//! The probe tool reports both so they can be cross-checked.

use std::path::{Path, PathBuf};

/// The default path of the `thinkpad_acpi` fan interface.
pub const FAN_PROC_PATH: &str = "/proc/acpi/ibm/fan";

/// A parsed snapshot of `/proc/acpi/ibm/fan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanStatus {
    /// The fan status, e.g. `enabled` or `disabled`.
    pub status: String,
    /// The realtime fan speed in RPM.
    pub speed: u16,
    /// The fan level as reported by the firmware.
    ///
    /// This is a string because the firmware may report `auto`, `disengaged`,
    /// `full-speed`, or `unknown` in addition to the numeric levels `0`–`7`.
    pub level: String,
}

impl FanStatus {
    /// Whether the fan is enabled.
    pub fn is_enabled(&self) -> bool {
        self.status.eq_ignore_ascii_case("enabled")
    }

    /// The numeric fan level, if the firmware reported one.
    ///
    /// Returns `None` for non-numeric levels such as `auto` or `disengaged`.
    pub fn level_number(&self) -> Option<u8> {
        self.level.parse().ok()
    }
}

/// Errors produced when reading `/proc/acpi/ibm/fan`.
#[derive(Debug, thiserror::Error)]
pub enum ThinkpadAcpiError {
    /// The `thinkpad_acpi` fan interface does not exist.
    #[error(
        "the `thinkpad_acpi` fan interface `{path}` was not found; load the `thinkpad_acpi` module"
    )]
    NotFound {
        /// The path that was probed.
        path: PathBuf,
    },

    /// An I/O error occurred while reading the interface.
    #[error("I/O error reading `{path}`: {source}")]
    Io {
        /// The path that was read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A required field was missing from the interface.
    #[error("malformed `{path}`: missing the `{field}` field")]
    MissingField {
        /// The path that was read.
        path: PathBuf,
        /// The missing field name.
        field: &'static str,
    },

    /// A field had an unparseable value.
    #[error("malformed `{path}`: invalid `{field}` value `{value}`")]
    InvalidField {
        /// The path that was read.
        path: PathBuf,
        /// The offending field name.
        field: &'static str,
        /// The offending value.
        value: String,
    },
}

/// Parse the contents of `/proc/acpi/ibm/fan`.
///
/// Unknown lines (such as `commands:` or `fan_control:`) are ignored.
pub fn parse_fan_status(input: &str) -> Result<FanStatus, ThinkpadAcpiError> {
    let path = PathBuf::from(FAN_PROC_PATH);
    let mut status = None;
    let mut speed = None;
    let mut level = None;

    for line in input.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "status" => status = Some(value.to_string()),
            "speed" => {
                let parsed = value
                    .parse::<u16>()
                    .map_err(|_| ThinkpadAcpiError::InvalidField {
                        path: path.clone(),
                        field: "speed",
                        value: value.to_string(),
                    })?;
                speed = Some(parsed);
            }
            "level" => level = Some(value.to_string()),
            _ => {}
        }
    }

    Ok(FanStatus {
        status: status.ok_or_else(|| ThinkpadAcpiError::MissingField {
            path: path.clone(),
            field: "status",
        })?,
        speed: speed.ok_or_else(|| ThinkpadAcpiError::MissingField {
            path: path.clone(),
            field: "speed",
        })?,
        level: level.ok_or_else(|| ThinkpadAcpiError::MissingField {
            path: path.clone(),
            field: "level",
        })?,
    })
}

/// Read and parse the default `/proc/acpi/ibm/fan` interface.
pub fn read_fan_status() -> Result<FanStatus, ThinkpadAcpiError> {
    read_fan_status_from(Path::new(FAN_PROC_PATH))
}

/// Read and parse a specific `thinkpad_acpi` fan interface path.
pub fn read_fan_status_from(path: &Path) -> Result<FanStatus, ThinkpadAcpiError> {
    let contents = std::fs::read_to_string(path).map_err(|source| match source.kind() {
        std::io::ErrorKind::NotFound => ThinkpadAcpiError::NotFound {
            path: path.to_path_buf(),
        },
        _ => ThinkpadAcpiError::Io {
            path: path.to_path_buf(),
            source,
        },
    })?;
    parse_fan_status(&contents)
}

/// Read only the realtime fan speed in RPM.
pub fn read_fan_speed() -> Result<u16, ThinkpadAcpiError> {
    Ok(read_fan_status()?.speed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
status:         enabled
speed:          3900
level:          7
commands:       level <level> (<level> is 0-7, auto, disengaged, full-speed)
fan_control:    enabled
";

    #[test]
    fn parses_a_typical_interface() {
        let status = parse_fan_status(SAMPLE).unwrap();
        assert_eq!(status.status, "enabled");
        assert_eq!(status.speed, 3900);
        assert_eq!(status.level, "7");
        assert!(status.is_enabled());
        assert_eq!(status.level_number(), Some(7));
    }

    #[test]
    fn parses_auto_level() {
        let input = "status: enabled\nspeed: 0\nlevel: auto\n";
        let status = parse_fan_status(input).unwrap();
        assert_eq!(status.level, "auto");
        assert_eq!(status.level_number(), None);
    }

    #[test]
    fn rejects_missing_fields() {
        let error = parse_fan_status("status: enabled\nspeed: 3900\n").unwrap_err();
        assert!(matches!(
            error,
            ThinkpadAcpiError::MissingField { field: "level", .. }
        ));
    }

    #[test]
    fn rejects_invalid_speed() {
        let error = parse_fan_status("status: enabled\nspeed: fast\nlevel: 7\n").unwrap_err();
        assert!(matches!(
            error,
            ThinkpadAcpiError::InvalidField { field: "speed", .. }
        ));
    }

    #[test]
    fn reports_missing_file() {
        let error = read_fan_status_from(Path::new("/nonexistent/omafanctrl/fan")).unwrap_err();
        assert!(matches!(error, ThinkpadAcpiError::NotFound { .. }));
    }
}
