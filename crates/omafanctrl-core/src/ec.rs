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
//! # Safety
//!
//! Writing the wrong EC register can harm the machine. All access must be
//! bounds-checked and restricted to the documented, verified register set. See
//! `plans/risks.md` (R1, R2) before implementing writes.
//!
//! Implementation lands in milestone **M1**.

/// The size of the EC register window in bytes.
pub const EC_WINDOW_SIZE: usize = 256;

/// The EC register that controls the fan.
///
/// `0x00` = BIOS auto, `0x01`–`0x07` = manual levels, `0x40`/`0x80` = disengaged.
pub const REG_FAN_CONTROL: u8 = 0x2F;

/// Low byte of the fan tachometer (RPM) register.
pub const REG_FAN_RPM_LOW: u8 = 0x84;

/// High byte of the fan tachometer (RPM) register.
pub const REG_FAN_RPM_HIGH: u8 = 0x85;
