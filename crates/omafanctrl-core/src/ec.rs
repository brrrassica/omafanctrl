//! Embedded Controller (EC) access layer.
//!
//! On Linux the EC register window is exposed by the `ec_sys` kernel module with
//! write support enabled:
//!
//! ```sh
//! modprobe ec_sys write_support=1
//! ```
//!
//! which makes a 256-byte register window available at
//! `/sys/kernel/debug/ec/ec0/io`.
//!
//! # Verified register map
//!
//! The map below was verified against the TPFanCtrl2 2.3.3 source
//! (`fancontrol/fanstuff.cpp`) and a live probe on the ThinkPad E14 Gen 4:
//!
//! | Register | Meaning |
//! | --- | --- |
//! | `0x2F` | Fan control: `0x80` = BIOS controlled, `0x00` = off, `0x01`–`0x07` = manual levels, `0x40` = disengaged/advanced |
//! | `0x31` | Fan switch: `0x00` = fan 1, `0x01` = fan 2 |
//! | `0x84`/`0x85` | Fan tachometer, low/high byte |
//! | `0x78`–`0x7F` | Temperature sensors (8, primary bank) |
//! | `0xC0`–`0xC3` | Temperature sensors (4, secondary bank) |
//!
//! # Safety
//!
//! Writing the wrong EC register can harm the machine. All access is
//! bounds-checked and restricted to the documented, verified register set. See
//! `plans/risks.md` (R1, R2).
//!
//! The layer enforces the following invariants:
//!
//! - Register offsets are [`u8`], so they can never address outside the 256-byte
//!   window; the backend additionally re-checks the offset defensively.
//! - Only [`REG_FAN_CONTROL`] and [`REG_FAN_SWITCH`] may ever be written (see
//!   [`WRITABLE_REGISTERS`]).
//! - Fan changes use the TPFanCtrl2 fan-switch handshake and are verified by
//!   reading the control register back.
//! - Temperature readings above [`TEMP_MAX_C`] are rejected.
//! - Manual fan levels are validated against [`FAN_LEVEL_MIN`]..=[`FAN_LEVEL_MAX`].
//! - Writes are rate-limited to at most one per [`DEFAULT_MIN_WRITE_INTERVAL`],
//!   except for the safety-critical [`Ec::set_fan_auto`] revert.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The size of the EC register window in bytes.
pub const EC_WINDOW_SIZE: usize = 256;

/// The default path of the EC register window exposed by `ec_sys`.
pub const EC_DEVICE_PATH: &str = "/sys/kernel/debug/ec/ec0/io";

/// The sysfs directory of the `ec_sys` module, used for availability detection.
pub const EC_SYS_MODULE_PATH: &str = "/sys/module/ec_sys";

/// The EC register that controls the fan.
///
/// Encoding (verified against TPFanCtrl2 2.3.3 and a live E14 Gen 4):
///
/// - bit 7 (`0x80`) set → BIOS controlled (the safe state).
/// - bits 0-5 (`0x3F`) → manual fan level (`0x00` = off, `0x01`–`0x07` = levels).
/// - `0x40` → "disengaged"/advanced level (see [`FAN_DISENGAGED`]).
pub const REG_FAN_CONTROL: u8 = 0x2F;

/// The EC register that selects which fan the control/RPM registers refer to.
///
/// The EC requires this register to be written before [`REG_FAN_CONTROL`] to
/// apply a fan change. TPFanCtrl2 performs this handshake on every change.
pub const REG_FAN_SWITCH: u8 = 0x31;

/// Fan-switch value selecting fan 1.
pub const FAN_SWITCH_SELFAN1: u8 = 0x00;

/// Fan-switch value selecting fan 2.
pub const FAN_SWITCH_SELFAN2: u8 = 0x01;

/// Low byte of the fan tachometer (RPM) register.
pub const REG_FAN_RPM_LOW: u8 = 0x84;

/// High byte of the fan tachometer (RPM) register.
pub const REG_FAN_RPM_HIGH: u8 = 0x85;

/// The only EC registers this layer is permitted to write.
///
/// Restricting writes to the verified fan control and fan switch registers is a
/// core safety invariant (see `plans/risks.md`, R1).
pub const WRITABLE_REGISTERS: &[u8] = &[REG_FAN_CONTROL, REG_FAN_SWITCH];

/// Fan control value that hands the fan back to the firmware (the safe state).
///
/// Verified: TPFanCtrl2 writes `0x80` for BIOS mode, and a live E14 Gen 4 reads
/// `0x80` from `0x2F` while under BIOS control. Writing `0x00` here would turn
/// the fan **off**, so this value is safety-critical.
pub const FAN_BIOS_AUTO: u8 = 0x80;

/// Fan control value for the "disengaged"/advanced level.
///
/// **Unverified:** TPFanCtrl2 treats `64` (`0x40`) as a special level gated by
/// its `Lev64Norm` option. Confirm on the E14 Gen 4 before relying on it.
pub const FAN_DISENGAGED: u8 = 0x40;

/// Lowest valid manual fan level.
pub const FAN_LEVEL_MIN: u8 = 0x01;

/// Highest valid manual fan level.
///
/// TPFanCtrl2 accepts `0x00`–`0x07`. On the E14 Gen 4 the observed curve is
/// highly non-linear (see [`crate::fan_curve`] and `config/E14G4-quirks`):
///
/// | Level | RPM |
/// | --- | --- |
/// | 1 | 1800 |
/// | 2 | 2200 |
/// | 3–7 | 3900 |
///
/// The fan reaches its maximum (3900 RPM) at level 3; levels 4–7 are redundant
/// and only waste power, so prefer 1–3.
pub const FAN_LEVEL_MAX: u8 = 0x07;

/// Highest temperature (°C) accepted from a sensor before it is rejected.
pub const TEMP_MAX_C: u8 = 127;

/// Default minimum interval between two EC writes.
pub const DEFAULT_MIN_WRITE_INTERVAL: Duration = Duration::from_millis(100);

/// Number of times the fan control register is re-read after a write.
pub const FAN_CONTROL_VERIFY_ATTEMPTS: u32 = 3;

/// Delay between fan control verification attempts.
pub const FAN_CONTROL_VERIFY_INTERVAL: Duration = Duration::from_millis(10);

/// Errors produced by the EC access layer.
#[derive(Debug, thiserror::Error)]
pub enum EcError {
    /// The `ec_sys` kernel module is not loaded.
    #[error("the `ec_sys` kernel module is not loaded; run `modprobe ec_sys write_support=1`")]
    ModuleNotLoaded,

    /// The EC register window does not exist.
    #[error(
        "EC device `{path}` was not found; ensure debugfs is mounted and `ec_sys` is loaded with `write_support=1`"
    )]
    DeviceNotFound {
        /// The path that was probed.
        path: PathBuf,
    },

    /// The EC register window exists but cannot be opened for writing.
    #[error(
        "EC device `{path}` is not writable; load `ec_sys` with `write_support=1` and run as root"
    )]
    DeviceNotWritable {
        /// The path that was probed.
        path: PathBuf,
    },

    /// An I/O error occurred while accessing the EC.
    #[error("I/O error on EC device `{path}`: {source}")]
    Io {
        /// The path that was accessed.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A register offset fell outside the 256-byte window.
    #[error("EC register offset 0x{offset:02X} is out of bounds (window is 256 bytes)")]
    OutOfBounds {
        /// The rejected offset.
        offset: usize,
    },

    /// A write targeted a register outside the verified writable set.
    #[error(
        "writing EC register 0x{offset:02X} is not allowed; only the verified fan control and fan switch registers may be written"
    )]
    WriteNotAllowed {
        /// The rejected register offset.
        offset: u8,
    },

    /// A manual fan level was outside the valid range.
    #[error("fan level {level} is invalid; expected 1..=7")]
    InvalidFanLevel {
        /// The rejected level.
        level: u8,
    },

    /// The EC did not accept the requested fan control value.
    #[error("EC did not apply fan control 0x{expected:02X}; read back 0x{actual:02X}")]
    FanControlNotApplied {
        /// The value that was written.
        expected: u8,
        /// The value read back from the EC.
        actual: u8,
    },

    /// A temperature sensor returned an implausible value.
    #[error(
        "temperature sensor 0x{offset:02X} returned {value} °C, outside the sane range 0..=127 °C"
    )]
    TemperatureOutOfRange {
        /// The sensor register offset.
        offset: u8,
        /// The implausible value.
        value: u8,
    },

    /// A write was rejected because the rate limit has not elapsed.
    #[error("EC write rate limit active; retry in {retry_after:?}")]
    RateLimited {
        /// How long the caller should wait before retrying.
        retry_after: Duration,
    },
}

/// A single temperature sensor reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemperatureReading {
    /// The EC register offset the reading came from.
    pub offset: u8,
    /// The temperature in degrees Celsius.
    pub celsius: u8,
}

/// Whether the EC window is opened read-only or read-write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// Open the window for reading only.
    ReadOnly,
    /// Open the window for reading and writing.
    ReadWrite,
}

/// Low-level, raw EC access.
///
/// Implementations perform the actual I/O; the [`Ec`] wrapper adds the safety
/// invariants. Offsets are [`usize`] here so implementations can reject
/// out-of-window access defensively.
pub trait EcBackend: Send {
    /// Read a single byte at `offset`.
    fn read_byte(&mut self, offset: usize) -> Result<u8, EcError>;

    /// Write `value` to `offset`.
    fn write_byte(&mut self, offset: usize, value: u8) -> Result<(), EcError>;

    /// Read the entire 256-byte register window.
    fn read_window(&mut self) -> Result<[u8; EC_WINDOW_SIZE], EcError>;
}

/// Reject offsets outside the EC window.
fn check_offset(offset: usize) -> Result<(), EcError> {
    if offset >= EC_WINDOW_SIZE {
        return Err(EcError::OutOfBounds { offset });
    }
    Ok(())
}

/// An [`EcBackend`] backed by the `ec_sys` register window file.
#[derive(Debug)]
pub struct FileBackend {
    file: File,
    path: PathBuf,
}

impl FileBackend {
    /// Open the default EC window, checking that `ec_sys` is loaded.
    pub fn open(mode: AccessMode) -> Result<Self, EcError> {
        if !Path::new(EC_SYS_MODULE_PATH).exists() {
            return Err(EcError::ModuleNotLoaded);
        }
        Self::open_path(Path::new(EC_DEVICE_PATH), mode)
    }

    /// Open a specific EC window path.
    pub fn open_path(path: &Path, mode: AccessMode) -> Result<Self, EcError> {
        if !path.exists() {
            return Err(EcError::DeviceNotFound {
                path: path.to_path_buf(),
            });
        }
        let file = OpenOptions::new()
            .read(true)
            .write(mode == AccessMode::ReadWrite)
            .open(path)
            .map_err(|source| match source.kind() {
                std::io::ErrorKind::PermissionDenied if mode == AccessMode::ReadWrite => {
                    EcError::DeviceNotWritable {
                        path: path.to_path_buf(),
                    }
                }
                _ => EcError::Io {
                    path: path.to_path_buf(),
                    source,
                },
            })?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }
}

impl EcBackend for FileBackend {
    fn read_byte(&mut self, offset: usize) -> Result<u8, EcError> {
        check_offset(offset)?;
        self.file
            .seek(SeekFrom::Start(offset as u64))
            .map_err(|source| EcError::Io {
                path: self.path.clone(),
                source,
            })?;
        let mut buf = [0u8; 1];
        self.file
            .read_exact(&mut buf)
            .map_err(|source| EcError::Io {
                path: self.path.clone(),
                source,
            })?;
        Ok(buf[0])
    }

    fn write_byte(&mut self, offset: usize, value: u8) -> Result<(), EcError> {
        check_offset(offset)?;
        self.file
            .seek(SeekFrom::Start(offset as u64))
            .map_err(|source| EcError::Io {
                path: self.path.clone(),
                source,
            })?;
        self.file
            .write_all(&[value])
            .map_err(|source| EcError::Io {
                path: self.path.clone(),
                source,
            })?;
        self.file.flush().map_err(|source| EcError::Io {
            path: self.path.clone(),
            source,
        })?;
        Ok(())
    }

    fn read_window(&mut self) -> Result<[u8; EC_WINDOW_SIZE], EcError> {
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|source| EcError::Io {
                path: self.path.clone(),
                source,
            })?;
        let mut buf = [0u8; EC_WINDOW_SIZE];
        self.file
            .read_exact(&mut buf)
            .map_err(|source| EcError::Io {
                path: self.path.clone(),
                source,
            })?;
        Ok(buf)
    }
}

/// A safety-checked view over an [`EcBackend`].
#[derive(Debug)]
pub struct Ec<B: EcBackend> {
    backend: B,
    min_write_interval: Duration,
    last_write: Option<Instant>,
}

impl<B: EcBackend> Ec<B> {
    /// Wrap a backend with the default safety policy.
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            min_write_interval: DEFAULT_MIN_WRITE_INTERVAL,
            last_write: None,
        }
    }

    /// Override the minimum interval between EC writes.
    pub fn with_min_write_interval(mut self, interval: Duration) -> Self {
        self.min_write_interval = interval;
        self
    }

    /// Read a single register.
    pub fn read_byte(&mut self, offset: u8) -> Result<u8, EcError> {
        self.backend.read_byte(usize::from(offset))
    }

    /// Read the entire 256-byte register window.
    pub fn read_window(&mut self) -> Result<[u8; EC_WINDOW_SIZE], EcError> {
        self.backend.read_window()
    }

    /// Read the fan tachometer in RPM.
    ///
    /// The value is `(high << 8) | low` from [`REG_FAN_RPM_HIGH`] and
    /// [`REG_FAN_RPM_LOW`]. On dual-fan models this returns the currently
    /// selected fan (see [`REG_FAN_SWITCH`]); the E14 Gen 4 has a single fan.
    pub fn read_fan_rpm(&mut self) -> Result<u16, EcError> {
        let low = self.read_byte(REG_FAN_RPM_LOW)?;
        let high = self.read_byte(REG_FAN_RPM_HIGH)?;
        Ok((u16::from(high) << 8) | u16::from(low))
    }

    /// Read a single temperature sensor, rejecting implausible values.
    pub fn read_temperature(&mut self, offset: u8) -> Result<u8, EcError> {
        let value = self.read_byte(offset)?;
        if value > TEMP_MAX_C {
            return Err(EcError::TemperatureOutOfRange { offset, value });
        }
        Ok(value)
    }

    /// Read several temperature sensors, skipping implausible values.
    ///
    /// Unlike [`Ec::read_temperature`], a single bad sensor does not fail the
    /// whole batch; it is simply omitted from the result.
    pub fn read_temperatures(
        &mut self,
        offsets: &[u8],
    ) -> Result<Vec<TemperatureReading>, EcError> {
        let mut readings = Vec::with_capacity(offsets.len());
        for &offset in offsets {
            let value = self.read_byte(offset)?;
            if value <= TEMP_MAX_C {
                readings.push(TemperatureReading {
                    offset,
                    celsius: value,
                });
            }
        }
        Ok(readings)
    }

    /// Write a raw register, honouring the rate limit.
    ///
    /// This is a low-level primitive: it does **not** perform the fan-switch
    /// handshake. Use [`Ec::set_fan_auto`], [`Ec::set_fan_level`], or
    /// [`Ec::set_fan_disengaged`] to change the fan. Only registers in
    /// [`WRITABLE_REGISTERS`] are accepted.
    pub fn write_byte(&mut self, offset: u8, value: u8) -> Result<(), EcError> {
        self.enforce_rate_limit()?;
        self.raw_write(offset, value)?;
        self.last_write = Some(Instant::now());
        Ok(())
    }

    /// Write a raw register without the rate limit, checking the writable set.
    fn raw_write(&mut self, offset: u8, value: u8) -> Result<(), EcError> {
        if !WRITABLE_REGISTERS.contains(&offset) {
            return Err(EcError::WriteNotAllowed { offset });
        }
        self.backend.write_byte(usize::from(offset), value)
    }

    /// Apply a fan control value using the TPFanCtrl2 fan-switch handshake.
    ///
    /// The EC only applies a change to [`REG_FAN_CONTROL`] after the fan switch
    /// ([`REG_FAN_SWITCH`]) has been toggled. The written value is read back and
    /// verified.
    fn apply_fan_control(&mut self, value: u8) -> Result<(), EcError> {
        self.raw_write(REG_FAN_SWITCH, FAN_SWITCH_SELFAN1)?;
        self.raw_write(REG_FAN_CONTROL, value)?;
        self.raw_write(REG_FAN_SWITCH, FAN_SWITCH_SELFAN2)?;
        self.raw_write(REG_FAN_CONTROL, value)?;
        self.raw_write(REG_FAN_SWITCH, FAN_SWITCH_SELFAN1)?;

        // Verify the EC accepted the value, retrying briefly in case the
        // register update is not yet visible.
        let mut actual = value;
        for attempt in 0..FAN_CONTROL_VERIFY_ATTEMPTS {
            actual = self.backend.read_byte(usize::from(REG_FAN_CONTROL))?;
            if actual == value {
                self.last_write = Some(Instant::now());
                return Ok(());
            }
            if attempt + 1 < FAN_CONTROL_VERIFY_ATTEMPTS {
                std::thread::sleep(FAN_CONTROL_VERIFY_INTERVAL);
            }
        }
        Err(EcError::FanControlNotApplied {
            expected: value,
            actual,
        })
    }

    fn enforce_rate_limit(&self) -> Result<(), EcError> {
        if let Some(last) = self.last_write {
            let elapsed = last.elapsed();
            if elapsed < self.min_write_interval {
                return Err(EcError::RateLimited {
                    retry_after: self.min_write_interval - elapsed,
                });
            }
        }
        Ok(())
    }

    /// Hand the fan back to the firmware (the canonical safe state).
    ///
    /// This bypasses the write rate limit: reverting to BIOS auto must never be
    /// delayed by throttling.
    pub fn set_fan_auto(&mut self) -> Result<(), EcError> {
        self.apply_fan_control(FAN_BIOS_AUTO)
    }

    /// Apply a fixed manual fan level.
    ///
    /// `level` must be in [`FAN_LEVEL_MIN`]..=[`FAN_LEVEL_MAX`]. Level `0x00`
    /// (fan off) is deliberately rejected.
    pub fn set_fan_level(&mut self, level: u8) -> Result<(), EcError> {
        if !(FAN_LEVEL_MIN..=FAN_LEVEL_MAX).contains(&level) {
            return Err(EcError::InvalidFanLevel { level });
        }
        self.enforce_rate_limit()?;
        self.apply_fan_control(level)
    }

    /// Disengage the fan (advanced level).
    ///
    /// **Unverified:** see [`FAN_DISENGAGED`].
    pub fn set_fan_disengaged(&mut self) -> Result<(), EcError> {
        self.enforce_rate_limit()?;
        self.apply_fan_control(FAN_DISENGAGED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct MockBackend {
        bytes: [u8; EC_WINDOW_SIZE],
        writes: Vec<(usize, u8)>,
    }

    impl Default for MockBackend {
        fn default() -> Self {
            Self {
                bytes: [0u8; EC_WINDOW_SIZE],
                writes: Vec::new(),
            }
        }
    }

    impl EcBackend for MockBackend {
        fn read_byte(&mut self, offset: usize) -> Result<u8, EcError> {
            check_offset(offset)?;
            Ok(self.bytes[offset])
        }

        fn write_byte(&mut self, offset: usize, value: u8) -> Result<(), EcError> {
            check_offset(offset)?;
            self.bytes[offset] = value;
            self.writes.push((offset, value));
            Ok(())
        }

        fn read_window(&mut self) -> Result<[u8; EC_WINDOW_SIZE], EcError> {
            Ok(self.bytes)
        }
    }

    /// A backend that silently drops writes to the fan control register, used to
    /// exercise the read-back verification.
    #[derive(Debug)]
    struct StubbornBackend {
        bytes: [u8; EC_WINDOW_SIZE],
    }

    impl EcBackend for StubbornBackend {
        fn read_byte(&mut self, offset: usize) -> Result<u8, EcError> {
            check_offset(offset)?;
            Ok(self.bytes[offset])
        }

        fn write_byte(&mut self, offset: usize, value: u8) -> Result<(), EcError> {
            check_offset(offset)?;
            if offset != usize::from(REG_FAN_CONTROL) {
                self.bytes[offset] = value;
            }
            Ok(())
        }

        fn read_window(&mut self) -> Result<[u8; EC_WINDOW_SIZE], EcError> {
            Ok(self.bytes)
        }
    }

    fn ec() -> Ec<MockBackend> {
        Ec::new(MockBackend::default())
    }

    #[test]
    fn reads_a_register() {
        let mut ec = ec();
        ec.backend.bytes[0x10] = 0xAB;
        assert_eq!(ec.read_byte(0x10).unwrap(), 0xAB);
    }

    #[test]
    fn reads_fan_rpm_from_low_and_high_bytes() {
        let mut ec = ec();
        ec.backend.bytes[usize::from(REG_FAN_RPM_LOW)] = 0x34;
        ec.backend.bytes[usize::from(REG_FAN_RPM_HIGH)] = 0x12;
        assert_eq!(ec.read_fan_rpm().unwrap(), 0x1234);
    }

    #[test]
    fn rejects_implausible_temperature() {
        let mut ec = ec();
        ec.backend.bytes[0x78] = 200;
        let error = ec.read_temperature(0x78).unwrap_err();
        assert!(matches!(
            error,
            EcError::TemperatureOutOfRange {
                offset: 0x78,
                value: 200
            }
        ));
    }

    #[test]
    fn skips_implausible_temperatures_in_batch() {
        let mut ec = ec();
        ec.backend.bytes[0x78] = 45;
        ec.backend.bytes[0x79] = 200;
        ec.backend.bytes[0x7A] = 60;
        let readings = ec.read_temperatures(&[0x78, 0x79, 0x7A]).unwrap();
        assert_eq!(
            readings,
            vec![
                TemperatureReading {
                    offset: 0x78,
                    celsius: 45
                },
                TemperatureReading {
                    offset: 0x7A,
                    celsius: 60
                },
            ]
        );
    }

    #[test]
    fn rejects_invalid_fan_level() {
        let mut ec = ec();
        assert!(matches!(
            ec.set_fan_level(0).unwrap_err(),
            EcError::InvalidFanLevel { level: 0 }
        ));
        assert!(matches!(
            ec.set_fan_level(8).unwrap_err(),
            EcError::InvalidFanLevel { level: 8 }
        ));
    }

    #[test]
    fn writes_manual_fan_level_with_handshake() {
        let mut ec = ec();
        ec.set_fan_level(3).unwrap();
        assert_eq!(ec.backend.bytes[usize::from(REG_FAN_CONTROL)], 3);
        assert_eq!(
            ec.backend.writes,
            vec![
                (usize::from(REG_FAN_SWITCH), FAN_SWITCH_SELFAN1),
                (usize::from(REG_FAN_CONTROL), 3),
                (usize::from(REG_FAN_SWITCH), FAN_SWITCH_SELFAN2),
                (usize::from(REG_FAN_CONTROL), 3),
                (usize::from(REG_FAN_SWITCH), FAN_SWITCH_SELFAN1),
            ]
        );
    }

    #[test]
    fn writes_bios_auto_as_0x80() {
        let mut ec = ec();
        ec.set_fan_auto().unwrap();
        assert_eq!(ec.backend.bytes[usize::from(REG_FAN_CONTROL)], 0x80);
        assert_eq!(FAN_BIOS_AUTO, 0x80);
    }

    #[test]
    fn writes_disengaged_as_0x40() {
        let mut ec = ec();
        ec.set_fan_disengaged().unwrap();
        assert_eq!(ec.backend.bytes[usize::from(REG_FAN_CONTROL)], 0x40);
        assert_eq!(FAN_DISENGAGED, 0x40);
    }

    #[test]
    fn refuses_to_write_unverified_registers() {
        let mut ec = ec();
        assert!(matches!(
            ec.write_byte(0x10, 0xFF).unwrap_err(),
            EcError::WriteNotAllowed { offset: 0x10 }
        ));
        assert!(ec.backend.writes.is_empty());
    }

    #[test]
    fn rate_limits_writes() {
        let mut ec = ec().with_min_write_interval(Duration::from_secs(3600));
        ec.set_fan_level(1).unwrap();
        assert!(matches!(
            ec.set_fan_level(2).unwrap_err(),
            EcError::RateLimited { .. }
        ));
        // Only the first handshake (5 writes) should have happened.
        assert_eq!(ec.backend.writes.len(), 5);
    }

    #[test]
    fn fan_auto_bypasses_rate_limit() {
        let mut ec = ec().with_min_write_interval(Duration::from_secs(3600));
        ec.set_fan_level(1).unwrap();
        ec.set_fan_auto().unwrap();
        assert_eq!(
            ec.backend.bytes[usize::from(REG_FAN_CONTROL)],
            FAN_BIOS_AUTO
        );
    }

    #[test]
    fn detects_failed_fan_control_apply() {
        let mut ec = Ec::new(StubbornBackend {
            bytes: [0u8; EC_WINDOW_SIZE],
        });
        assert!(matches!(
            ec.set_fan_level(3).unwrap_err(),
            EcError::FanControlNotApplied {
                expected: 3,
                actual: 0
            }
        ));
    }

    #[test]
    fn backend_rejects_out_of_bounds_offsets() {
        let mut backend = MockBackend::default();
        assert!(matches!(
            backend.read_byte(EC_WINDOW_SIZE).unwrap_err(),
            EcError::OutOfBounds { offset } if offset == EC_WINDOW_SIZE
        ));
    }
}
