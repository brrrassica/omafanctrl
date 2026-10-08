//! Read-only EC probe.
//!
//! Dumps the full 256-byte register window and decodes the known fan and
//! temperature registers. This is the safe inspection tool referenced by
//! `plans/risks.md` (R1): it never writes unless explicitly asked.

use crate::ec::{
    EC_WINDOW_SIZE, Ec, EcBackend, EcError, FAN_BIOS_AUTO, FAN_DISENGAGED, FAN_LEVEL_MAX,
    REG_FAN_CONTROL, REG_FAN_RPM_HIGH, REG_FAN_RPM_LOW, TEMP_MAX_C, TemperatureReading,
};

/// Temperature sensor offsets documented for the E14 Gen 4.
///
/// **Unverified:** confirm against a live probe before relying on these. The
/// offsets mirror the TPFanCtrl2 sensor table (primary bank `0x78`–`0x7F`,
/// secondary bank `0xC0`–`0xC3`).
pub const KNOWN_SENSOR_OFFSETS: &[u8] = &[
    0x78, 0x79, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E, 0x7F, 0xC0, 0xC1, 0xC2, 0xC3,
];

/// A decoded snapshot of the EC.
#[derive(Debug, Clone)]
pub struct ProbeReport {
    /// The raw 256-byte register window.
    pub window: [u8; EC_WINDOW_SIZE],
    /// The raw fan control register value.
    pub fan_control: u8,
    /// The decoded fan speed in RPM.
    pub fan_rpm: u16,
    /// The decoded temperature sensors (implausible values omitted).
    pub temperatures: Vec<TemperatureReading>,
}

impl ProbeReport {
    /// A human-readable description of the fan control register.
    ///
    /// Bit 7 (`0x80`) means BIOS controlled; otherwise the low bits are the
    /// manual level. See [`crate::ec::REG_FAN_CONTROL`].
    pub fn fan_control_description(&self) -> String {
        if self.fan_control & FAN_BIOS_AUTO != 0 {
            "BIOS controlled".to_string()
        } else if self.fan_control == FAN_DISENGAGED {
            "disengaged (unverified)".to_string()
        } else if self.fan_control <= FAN_LEVEL_MAX {
            format!("manual level {}", self.fan_control)
        } else {
            format!("unknown (0x{:02X})", self.fan_control)
        }
    }
}

/// Take a read-only snapshot of the EC.
pub fn probe<B: EcBackend>(ec: &mut Ec<B>) -> Result<ProbeReport, EcError> {
    let window = ec.read_window()?;
    let fan_control = window[usize::from(REG_FAN_CONTROL)];
    let fan_rpm = (u16::from(window[usize::from(REG_FAN_RPM_HIGH)]) << 8)
        | u16::from(window[usize::from(REG_FAN_RPM_LOW)]);
    let temperatures = KNOWN_SENSOR_OFFSETS
        .iter()
        .filter_map(|&offset| {
            let celsius = window[usize::from(offset)];
            (celsius <= TEMP_MAX_C).then_some(TemperatureReading { offset, celsius })
        })
        .collect();
    Ok(ProbeReport {
        window,
        fan_control,
        fan_rpm,
        temperatures,
    })
}

/// Render the report as a human-readable hex dump plus decoded registers.
pub fn format_report(report: &ProbeReport) -> String {
    let mut out = String::new();
    out.push_str("EC register window (256 bytes):\n");
    for row in 0..(EC_WINDOW_SIZE / 16) {
        let base = row * 16;
        out.push_str(&format!("  {base:02X}:"));
        for col in 0..16 {
            out.push_str(&format!(" {:02X}", report.window[base + col]));
        }
        out.push_str("  |");
        for col in 0..16 {
            let byte = report.window[base + col];
            out.push(if (0x20..=0x7E).contains(&byte) {
                byte as char
            } else {
                '.'
            });
        }
        out.push_str("|\n");
    }
    out.push('\n');
    out.push_str(&format!(
        "Fan control (0x{REG_FAN_CONTROL:02X}): 0x{:02X} — {}\n",
        report.fan_control,
        report.fan_control_description()
    ));
    out.push_str(&format!(
        "Fan RPM (0x{REG_FAN_RPM_HIGH:02X}:0x{REG_FAN_RPM_LOW:02X}): {}\n",
        report.fan_rpm
    ));
    out.push_str("Temperature sensors:\n");
    if report.temperatures.is_empty() {
        out.push_str("  (none in range)\n");
    } else {
        for reading in &report.temperatures {
            out.push_str(&format!(
                "  0x{:02X}: {} °C\n",
                reading.offset, reading.celsius
            ));
        }
    }
    out
}

/// Render the report as JSON.
pub fn format_json(report: &ProbeReport) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"window\": [");
    for (index, byte) in report.window.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&byte.to_string());
    }
    out.push_str("],\n");
    out.push_str(&format!("  \"fan_control\": {},\n", report.fan_control));
    out.push_str(&format!(
        "  \"fan_control_description\": \"{}\",\n",
        report.fan_control_description()
    ));
    out.push_str(&format!("  \"fan_rpm\": {},\n", report.fan_rpm));
    out.push_str("  \"temperatures\": [");
    for (index, reading) in report.temperatures.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!(
            "{{\"offset\": {}, \"celsius\": {}}}",
            reading.offset, reading.celsius
        ));
    }
    out.push_str("]\n}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct MockBackend {
        bytes: [u8; EC_WINDOW_SIZE],
    }

    impl Default for MockBackend {
        fn default() -> Self {
            Self {
                bytes: [0u8; EC_WINDOW_SIZE],
            }
        }
    }

    impl EcBackend for MockBackend {
        fn read_byte(&mut self, offset: usize) -> Result<u8, EcError> {
            Ok(self.bytes[offset])
        }

        fn write_byte(&mut self, _offset: usize, _value: u8) -> Result<(), EcError> {
            Ok(())
        }

        fn read_window(&mut self) -> Result<[u8; EC_WINDOW_SIZE], EcError> {
            Ok(self.bytes)
        }
    }

    #[test]
    fn decodes_fan_and_temperatures() {
        let mut backend = MockBackend::default();
        backend.bytes[usize::from(REG_FAN_CONTROL)] = 3;
        backend.bytes[usize::from(REG_FAN_RPM_LOW)] = 0x34;
        backend.bytes[usize::from(REG_FAN_RPM_HIGH)] = 0x12;
        backend.bytes[0x78] = 45;
        backend.bytes[0x79] = 200; // implausible, skipped
        let mut ec = Ec::new(backend);
        let report = probe(&mut ec).unwrap();
        assert_eq!(report.fan_control, 3);
        assert_eq!(report.fan_rpm, 0x1234);
        assert!(report.temperatures.contains(&TemperatureReading {
            offset: 0x78,
            celsius: 45
        }));
        // The implausible 200 °C reading at 0x79 must be omitted.
        assert!(
            !report
                .temperatures
                .iter()
                .any(|reading| reading.offset == 0x79)
        );
        assert_eq!(report.fan_control_description(), "manual level 3");
    }

    #[test]
    fn formats_report_and_json() {
        let mut backend = MockBackend::default();
        backend.bytes[usize::from(REG_FAN_CONTROL)] = FAN_BIOS_AUTO;
        let mut ec = Ec::new(backend);
        let report = probe(&mut ec).unwrap();
        let text = format_report(&report);
        assert!(text.contains("BIOS controlled"));
        let json = format_json(&report);
        assert!(json.contains(&format!("\"fan_control\": {}", FAN_BIOS_AUTO)));
    }
}
