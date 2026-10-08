//! Observed fan curve for the ThinkPad E14 Gen 4.
//!
//! The E14 Gen 4 fan is **not** linear in the EC fan-control level. A live
//! observation (see `config/E14G4-quirks`) of each manual register value in
//! [`crate::ec::REG_FAN_CONTROL`] against `/proc/acpi/ibm/fan` produced:
//!
//! | Level | RPM |
//! | --- | --- |
//! | 1 | 1800 |
//! | 2 | 2200 |
//! | 3 | 3900 |
//! | 4 | 3900 |
//! | 5 | 3900 |
//! | 6 | 3900 |
//! | 7 | 3900 |
//!
//! In other words there are only **three distinct speeds** — 1800, 2200, and
//! 3900 RPM — and levels 3 through 7 all saturate at the maximum. This is why
//! the shipped profile ([`data/profiles/e14-gen4.ini`]) only uses levels 1–3:
//! higher levels add no cooling and only waste power and produce noise.
//!
//! The table is a *static observation*, not a live reading. Use
//! [`crate::probe`] (or the `--sweep` mode of `omafanctrl-probe`) to re-measure
//! it on real hardware.

/// A single point on the observed fan curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanCurvePoint {
    /// The EC fan-control level (`0x01`–`0x07`).
    pub level: u8,
    /// The observed fan speed in RPM at that level.
    pub rpm: u16,
}

/// The observed fan curve of the ThinkPad E14 Gen 4.
///
/// See the module documentation for provenance. Levels 3–7 all saturate at the
/// maximum speed.
pub const E14_GEN4_FAN_CURVE: &[FanCurvePoint] = &[
    FanCurvePoint {
        level: 1,
        rpm: 1800,
    },
    FanCurvePoint {
        level: 2,
        rpm: 2200,
    },
    FanCurvePoint {
        level: 3,
        rpm: 3900,
    },
    FanCurvePoint {
        level: 4,
        rpm: 3900,
    },
    FanCurvePoint {
        level: 5,
        rpm: 3900,
    },
    FanCurvePoint {
        level: 6,
        rpm: 3900,
    },
    FanCurvePoint {
        level: 7,
        rpm: 3900,
    },
];

/// The lowest speed the fan can be commanded to, in RPM.
pub const E14_GEN4_MIN_RPM: u16 = 1800;

/// The highest speed the fan can reach, in RPM.
pub const E14_GEN4_MAX_RPM: u16 = 3900;

/// Look up the observed RPM for a fan-control level.
///
/// Returns `None` for levels outside [`FAN_LEVEL_MIN`]..=[`FAN_LEVEL_MAX`].
pub fn rpm_for_level(level: u8) -> Option<u16> {
    E14_GEN4_FAN_CURVE
        .iter()
        .find(|point| point.level == level)
        .map(|point| point.rpm)
}

/// The levels that produce a *distinct* fan speed.
///
/// On the E14 Gen 4 this is `[1, 2, 3]`: levels 4–7 are redundant because they
/// saturate at the same maximum speed as level 3.
pub fn effective_levels() -> Vec<u8> {
    let mut levels = Vec::new();
    let mut last_rpm: Option<u16> = None;
    for point in E14_GEN4_FAN_CURVE {
        if last_rpm != Some(point.rpm) {
            levels.push(point.level);
            last_rpm = Some(point.rpm);
        }
    }
    levels
}

/// The distinct fan speeds the curve can produce, in ascending level order.
pub fn distinct_speeds() -> Vec<u16> {
    let mut speeds = Vec::new();
    for point in E14_GEN4_FAN_CURVE {
        if !speeds.contains(&point.rpm) {
            speeds.push(point.rpm);
        }
    }
    speeds
}

/// Render the observed curve as a human-readable table.
pub fn format_curve() -> String {
    let mut out = String::new();
    out.push_str("Observed E14 Gen 4 fan curve (level -> RPM):\n");
    for point in E14_GEN4_FAN_CURVE {
        out.push_str(&format!("  level {} -> {} RPM\n", point.level, point.rpm));
    }
    let effective = effective_levels();
    out.push_str(&format!(
        "  distinct speeds: {:?} RPM (effective levels: {:?})\n",
        distinct_speeds(),
        effective
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ec::{FAN_LEVEL_MAX, FAN_LEVEL_MIN};

    #[test]
    fn curve_covers_every_valid_level() {
        for level in FAN_LEVEL_MIN..=FAN_LEVEL_MAX {
            assert!(
                rpm_for_level(level).is_some(),
                "level {level} missing from the curve"
            );
        }
    }

    #[test]
    fn curve_rejects_out_of_range_levels() {
        assert_eq!(rpm_for_level(0), None);
        assert_eq!(rpm_for_level(FAN_LEVEL_MAX + 1), None);
    }

    #[test]
    fn matches_the_observed_quirks_file() {
        assert_eq!(rpm_for_level(1), Some(1800));
        assert_eq!(rpm_for_level(2), Some(2200));
        for level in 3..=7 {
            assert_eq!(rpm_for_level(level), Some(3900));
        }
    }

    #[test]
    fn only_three_levels_are_effective() {
        assert_eq!(effective_levels(), vec![1, 2, 3]);
        assert_eq!(distinct_speeds(), vec![1800, 2200, 3900]);
    }

    #[test]
    fn min_and_max_rpm_match_the_curve() {
        assert_eq!(E14_GEN4_MIN_RPM, 1800);
        assert_eq!(E14_GEN4_MAX_RPM, 3900);
        assert_eq!(rpm_for_level(FAN_LEVEL_MIN), Some(E14_GEN4_MIN_RPM));
        assert_eq!(rpm_for_level(FAN_LEVEL_MAX), Some(E14_GEN4_MAX_RPM));
    }
}
