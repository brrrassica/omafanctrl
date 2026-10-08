//! Configuration model.
//!
//! Configuration is TPFanCtrl2 `.ini` compatible. The parser accepts the flat
//! TPFanCtrl2 layout (keys with no section header) as well as explicit
//! `[General]`, `[Sensors]`, `[FanLevels]`, and `[SmartMode]` sections.
//!
//! # Format
//!
//! - Keys are **case-insensitive**; they are lower-cased on parse.
//! - Values are trimmed of surrounding whitespace.
//! - Comments start with `;`, `#`, or `//` and may appear on their own line or
//!   inline after a value.
//! - Section headers are written `[Section]`.
//! - `Level=` entries use the TPFanCtrl2 form `temp fan [hystUp hystDown]`.
//!
//! # Example
//!
//! ```ini
//! [General]
//! Active=2
//! Cycle=5
//!
//! [Sensors]
//! IgnoreSensors=pwr,no5
//!
//! [FanLevels]
//! Level1=1800
//! Level2=2200
//! Level3=3900
//!
//! [SmartMode]
//! Label=Smart Mode 1/
//! Level=45 1 0 0
//! Level=60 3 0 0
//! ```

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::ec::{FAN_LEVEL_MAX, FAN_LEVEL_MIN, TEMP_MAX_C};
use crate::fan_curve::E14_GEN4_FAN_CURVE;

/// The default configuration file name, matching TPFanCtrl2.
pub const DEFAULT_CONFIG_FILE: &str = "TPFanControl.ini";

/// The number of temperature thresholds expected in `IconLevels`.
pub const ICON_LEVEL_COUNT: usize = 3;

/// The highest value accepted for `Active` (0 = BIOS, 1 = Smart 1, 2 = Smart 2,
/// 3 = Manual).
pub const ACTIVE_MAX: u8 = 3;

/// The maximum number of smart modes TPFanCtrl2 supports.
pub const MAX_SMART_MODES: usize = 2;

/// Errors produced while parsing or loading a configuration file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// An I/O error occurred while reading the configuration file.
    #[error("I/O error on config `{path}`: {source}")]
    Io {
        /// The path that was read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A line was not a comment, a section header, or a `key=value` pair.
    #[error("line {line}: expected `key=value`, found `{content}`")]
    MalformedLine {
        /// The 1-based line number.
        line: usize,
        /// The offending line.
        content: String,
    },

    /// A section header was not terminated with `]`.
    #[error("line {line}: unterminated section header `{content}`")]
    MalformedSection {
        /// The 1-based line number.
        line: usize,
        /// The offending line.
        content: String,
    },

    /// A value could not be parsed into the expected type.
    #[error("line {line}: invalid value `{value}` for `{key}`: {reason}")]
    InvalidValue {
        /// The 1-based line number.
        line: usize,
        /// The key being parsed.
        key: String,
        /// The offending value.
        value: String,
        /// A human-readable explanation.
        reason: String,
    },

    /// The parsed configuration failed semantic validation.
    #[error(transparent)]
    Validation(#[from] ValidationError),
}

/// Semantic validation failures, each with a clear, actionable message.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValidationError {
    /// `Active` was outside `0..=3`.
    #[error("`Active` must be 0..=3 (0 = BIOS, 1 = Smart 1, 2 = Smart 2, 3 = Manual), got {0}")]
    ActiveOutOfRange(u8),

    /// `Cycle` was zero.
    #[error("`Cycle` must be at least 1 second, got {0}")]
    CycleTooSmall(u32),

    /// `ManFanSpeed` was outside `0..=7`.
    #[error("`ManFanSpeed` must be 0..=7, got {0}")]
    ManFanSpeedOutOfRange(u8),

    /// `IconLevels` did not contain exactly three temperatures.
    #[error("`IconLevels` must contain exactly {ICON_LEVEL_COUNT} temperatures, got {0}")]
    IconLevelCount(usize),

    /// `IconLevels` contained a value above the sane temperature range.
    #[error("`IconLevels` values must be 0..=127 °C, got {0:?}")]
    IconLevelsOutOfRange(Vec<u8>),

    /// `IconLevels` was not strictly ascending.
    #[error("`IconLevels` must be strictly ascending, got {0:?}")]
    IconLevelsNotAscending(Vec<u8>),

    /// The fan-level table was empty.
    #[error("`FanLevels` must define at least one level")]
    FanLevelsEmpty,

    /// A fan level was outside `1..=7`.
    #[error("fan level {level} is out of range 1..=7")]
    FanLevelOutOfRange {
        /// The offending level.
        level: u8,
    },

    /// A fan level mapped to zero RPM.
    #[error("fan level {level} maps to 0 RPM")]
    FanLevelZeroRpm {
        /// The offending level.
        level: u8,
    },

    /// More smart modes were defined than TPFanCtrl2 supports.
    #[error("at most {MAX_SMART_MODES} smart modes are supported, got {0}")]
    TooManySmartModes(usize),

    /// A smart mode defined no `Level` entries.
    #[error("smart mode {mode} must define at least one `Level`")]
    SmartModeEmpty {
        /// The 1-based smart mode number.
        mode: usize,
    },

    /// A smart-mode temperature was outside the sane range.
    #[error("smart mode {mode} temperature {temperature} is out of range 0..=127 °C")]
    SmartTemperatureOutOfRange {
        /// The 1-based smart mode number.
        mode: usize,
        /// The offending temperature.
        temperature: u8,
    },

    /// A smart-mode fan level was outside `1..=7`.
    #[error("smart mode {mode} fan level {level} is out of range 1..=7")]
    SmartFanLevelOutOfRange {
        /// The 1-based smart mode number.
        mode: usize,
        /// The offending fan level.
        level: u8,
    },

    /// A smart mode's temperatures were not strictly ascending.
    #[error("smart mode {mode} temperatures must be strictly ascending, got {temperatures:?}")]
    SmartTemperaturesNotAscending {
        /// The 1-based smart mode number.
        mode: usize,
        /// The temperatures in declaration order.
        temperatures: Vec<u8>,
    },

    /// The ignore list contained an empty sensor name.
    #[error("`IgnoreSensors` contains an empty sensor name")]
    EmptySensorName,
}

/// The `[General]` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct General {
    /// The active mode: 0 = BIOS, 1 = Smart 1, 2 = Smart 2, 3 = Manual.
    pub active: u8,
    /// The polling interval in seconds.
    pub cycle: u32,
    /// The manual fan level applied in Manual mode.
    pub man_fan_speed: u8,
    /// The temperature at which Manual mode exits back to the active mode.
    pub man_mode_exit: u8,
    /// The number of consecutive read errors tolerated before reverting.
    pub max_read_errors: u32,
    /// The process priority hint.
    pub process_priority: u8,
    /// The three temperature thresholds used to colour the tray icon.
    pub icon_levels: Vec<u8>,
    /// The `(on, off)` beep durations.
    pub fan_beep: (u8, u8),
    /// Whether global hotkeys are enabled.
    pub hotkeys: bool,
    /// Whether the window stays on top.
    pub stay_on_top: bool,
    /// Whether the dialog uses the slim layout.
    pub slim_dialog: bool,
    /// Whether Bluetooth EDR is enabled.
    pub bluetooth_edr: bool,
    /// Whether balloon notifications are suppressed.
    pub no_balloons: bool,
    /// Whether the external sensor is disabled.
    pub no_ext_sensor: bool,
    /// Whether the window starts minimised.
    pub start_minimized: bool,
    /// Whether the tray icon cycles through temperatures.
    pub icon_cycle: bool,
    /// Whether the temperature is shown in the tray icon.
    pub show_temp_icon: bool,
    /// Whether biased temperatures are shown.
    pub show_biased_temps: bool,
    /// Whether all sensors are shown.
    pub show_all: bool,
    /// Whether the fan icon is coloured.
    pub icon_color_fan: bool,
    /// Whether level 64 is treated as a normal level.
    pub lev64_norm: bool,
    /// Whether readings are logged to a file.
    pub log_to_file: bool,
    /// Whether readings are logged to CSV.
    pub log_to_csv: bool,
    /// Unrecognised keys, preserved for round-tripping.
    pub extra: BTreeMap<String, String>,
}

impl Default for General {
    fn default() -> Self {
        Self {
            active: 2,
            cycle: 5,
            man_fan_speed: 0,
            man_mode_exit: 78,
            max_read_errors: 10,
            process_priority: 2,
            icon_levels: vec![65, 75, 80],
            fan_beep: (0, 0),
            hotkeys: true,
            stay_on_top: true,
            slim_dialog: false,
            bluetooth_edr: false,
            no_balloons: true,
            no_ext_sensor: false,
            start_minimized: true,
            icon_cycle: true,
            show_temp_icon: true,
            show_biased_temps: true,
            show_all: false,
            icon_color_fan: true,
            lev64_norm: true,
            log_to_file: false,
            log_to_csv: false,
            extra: BTreeMap::new(),
        }
    }
}

/// The `[Sensors]` section.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sensors {
    /// Sensor names to ignore when computing the maximum temperature.
    pub ignore: Vec<String>,
    /// Optional display names keyed by TPFanCtrl2 sensor sequence number.
    pub names: BTreeMap<u8, String>,
    /// Unrecognised keys, preserved for round-tripping.
    pub extra: BTreeMap<String, String>,
}

/// The `[FanLevels]` section: the fan-control level to RPM curve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanLevels {
    /// The observed RPM for each fan-control level.
    pub levels: BTreeMap<u8, u16>,
    /// Unrecognised keys, preserved for round-tripping.
    pub extra: BTreeMap<String, String>,
}

impl Default for FanLevels {
    fn default() -> Self {
        let levels = E14_GEN4_FAN_CURVE
            .iter()
            .map(|point| (point.level, point.rpm))
            .collect();
        Self {
            levels,
            extra: BTreeMap::new(),
        }
    }
}

/// A single smart-mode threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmartLevel {
    /// The temperature threshold in degrees Celsius.
    pub temperature: u8,
    /// The fan level applied at or above this temperature.
    pub fan_level: u8,
    /// The upward hysteresis in degrees Celsius.
    pub hyst_up: u8,
    /// The downward hysteresis in degrees Celsius.
    pub hyst_down: u8,
}

/// The `[SmartMode]` section.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SmartMode {
    /// The optional menu label (TPFanCtrl2 `MenuLabelSM1`).
    pub label: Option<String>,
    /// The temperature thresholds, in declaration order.
    pub levels: Vec<SmartLevel>,
    /// Unrecognised keys, preserved for round-tripping.
    pub extra: BTreeMap<String, String>,
}

/// A fully parsed and validated configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    /// The `[General]` section.
    pub general: General,
    /// The `[Sensors]` section.
    pub sensors: Sensors,
    /// The `[FanLevels]` section.
    pub fan_levels: FanLevels,
    /// The smart modes, in order (`[SmartMode]`, `[SmartMode2]`).
    pub smart_modes: Vec<SmartMode>,
    /// Any unrecognised sections, preserved for round-tripping.
    pub extra_sections: BTreeMap<String, BTreeMap<String, String>>,
}

impl Config {
    /// Load and validate a configuration from a file.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        contents.parse()
    }

    /// Validate the configuration, returning the first failure.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let general = &self.general;
        if general.active > ACTIVE_MAX {
            return Err(ValidationError::ActiveOutOfRange(general.active));
        }
        if general.cycle == 0 {
            return Err(ValidationError::CycleTooSmall(general.cycle));
        }
        if general.man_fan_speed > FAN_LEVEL_MAX {
            return Err(ValidationError::ManFanSpeedOutOfRange(
                general.man_fan_speed,
            ));
        }
        if general.icon_levels.len() != ICON_LEVEL_COUNT {
            return Err(ValidationError::IconLevelCount(general.icon_levels.len()));
        }
        if general.icon_levels.iter().any(|&value| value > TEMP_MAX_C) {
            return Err(ValidationError::IconLevelsOutOfRange(
                general.icon_levels.clone(),
            ));
        }
        if !general.icon_levels.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err(ValidationError::IconLevelsNotAscending(
                general.icon_levels.clone(),
            ));
        }

        if self.fan_levels.levels.is_empty() {
            return Err(ValidationError::FanLevelsEmpty);
        }
        for (&level, &rpm) in &self.fan_levels.levels {
            if !(FAN_LEVEL_MIN..=FAN_LEVEL_MAX).contains(&level) {
                return Err(ValidationError::FanLevelOutOfRange { level });
            }
            if rpm == 0 {
                return Err(ValidationError::FanLevelZeroRpm { level });
            }
        }

        if self.smart_modes.len() > MAX_SMART_MODES {
            return Err(ValidationError::TooManySmartModes(self.smart_modes.len()));
        }
        for (index, mode) in self.smart_modes.iter().enumerate() {
            let mode_number = index + 1;
            if mode.levels.is_empty() {
                return Err(ValidationError::SmartModeEmpty { mode: mode_number });
            }
            let mut temperatures = Vec::with_capacity(mode.levels.len());
            for level in &mode.levels {
                if level.temperature > TEMP_MAX_C {
                    return Err(ValidationError::SmartTemperatureOutOfRange {
                        mode: mode_number,
                        temperature: level.temperature,
                    });
                }
                if !(FAN_LEVEL_MIN..=FAN_LEVEL_MAX).contains(&level.fan_level) {
                    return Err(ValidationError::SmartFanLevelOutOfRange {
                        mode: mode_number,
                        level: level.fan_level,
                    });
                }
                temperatures.push(level.temperature);
            }
            if !temperatures.windows(2).all(|pair| pair[0] < pair[1]) {
                return Err(ValidationError::SmartTemperaturesNotAscending {
                    mode: mode_number,
                    temperatures,
                });
            }
        }

        if self
            .sensors
            .ignore
            .iter()
            .any(|name| name.trim().is_empty())
        {
            return Err(ValidationError::EmptySensorName);
        }

        Ok(())
    }

    /// Serialise the configuration back to the canonical `.ini` form.
    ///
    /// The output is stable: parsing it again yields an equal [`Config`].
    pub fn to_ini(&self) -> String {
        let mut out = String::new();

        out.push_str("[General]\n");
        let general = &self.general;
        out.push_str(&format!("Active={}\n", general.active));
        out.push_str(&format!("Cycle={}\n", general.cycle));
        out.push_str(&format!("ManFanSpeed={}\n", general.man_fan_speed));
        out.push_str(&format!("ManModeExit={}\n", general.man_mode_exit));
        out.push_str(&format!("MaxReadErrors={}\n", general.max_read_errors));
        out.push_str(&format!("ProcessPriority={}\n", general.process_priority));
        out.push_str(&format!(
            "IconLevels={}\n",
            join_numbers(&general.icon_levels)
        ));
        out.push_str(&format!(
            "FanBeep={} {}\n",
            general.fan_beep.0, general.fan_beep.1
        ));
        out.push_str(&format!("Hotkeys={}\n", bool_ini(general.hotkeys)));
        out.push_str(&format!("StayOnTop={}\n", bool_ini(general.stay_on_top)));
        out.push_str(&format!("SlimDialog={}\n", bool_ini(general.slim_dialog)));
        out.push_str(&format!(
            "BluetoothEDR={}\n",
            bool_ini(general.bluetooth_edr)
        ));
        out.push_str(&format!("NoBallons={}\n", bool_ini(general.no_balloons)));
        out.push_str(&format!(
            "NoExtSensor={}\n",
            bool_ini(general.no_ext_sensor)
        ));
        out.push_str(&format!(
            "StartMinimized={}\n",
            bool_ini(general.start_minimized)
        ));
        out.push_str(&format!("IconCycle={}\n", bool_ini(general.icon_cycle)));
        out.push_str(&format!(
            "ShowTempIcon={}\n",
            bool_ini(general.show_temp_icon)
        ));
        out.push_str(&format!(
            "ShowBiasedTemps={}\n",
            bool_ini(general.show_biased_temps)
        ));
        out.push_str(&format!("ShowAll={}\n", bool_ini(general.show_all)));
        out.push_str(&format!(
            "IconColorFan={}\n",
            bool_ini(general.icon_color_fan)
        ));
        out.push_str(&format!("Lev64Norm={}\n", bool_ini(general.lev64_norm)));
        out.push_str(&format!("Log2File={}\n", bool_ini(general.log_to_file)));
        out.push_str(&format!("Log2csv={}\n", bool_ini(general.log_to_csv)));
        for (key, value) in &general.extra {
            out.push_str(&format!("{key}={value}\n"));
        }

        out.push_str("\n[Sensors]\n");
        out.push_str(&format!(
            "IgnoreSensors={}\n",
            self.sensors.ignore.join(",")
        ));
        for (sequence, name) in &self.sensors.names {
            out.push_str(&format!("SensorName{sequence}={name}\n"));
        }
        for (key, value) in &self.sensors.extra {
            out.push_str(&format!("{key}={value}\n"));
        }

        out.push_str("\n[FanLevels]\n");
        for (level, rpm) in &self.fan_levels.levels {
            out.push_str(&format!("Level{level}={rpm}\n"));
        }
        for (key, value) in &self.fan_levels.extra {
            out.push_str(&format!("{key}={value}\n"));
        }

        for (index, mode) in self.smart_modes.iter().enumerate() {
            let header = if index == 0 {
                "[SmartMode]"
            } else {
                "[SmartMode2]"
            };
            out.push_str(&format!("\n{header}\n"));
            if let Some(label) = &mode.label {
                out.push_str(&format!("Label={label}\n"));
            }
            for level in &mode.levels {
                out.push_str(&format!(
                    "Level={} {} {} {}\n",
                    level.temperature, level.fan_level, level.hyst_up, level.hyst_down
                ));
            }
            for (key, value) in &mode.extra {
                out.push_str(&format!("{key}={value}\n"));
            }
        }

        for (name, entries) in &self.extra_sections {
            out.push_str(&format!("\n[{name}]\n"));
            for (key, value) in entries {
                out.push_str(&format!("{key}={value}\n"));
            }
        }

        out
    }
}

impl FromStr for Config {
    type Err = ConfigError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let raw = parse_raw(input)?;
        let config = Config {
            general: build_general(raw.general)?,
            sensors: build_sensors(raw.sensors)?,
            fan_levels: build_fan_levels(raw.fan_levels)?,
            smart_modes: build_smart_modes(raw.smart_modes)?,
            extra_sections: raw
                .extra_sections
                .into_iter()
                .map(|(name, entries)| {
                    let entries = entries
                        .into_iter()
                        .map(|(key, value, _line)| (key, value))
                        .collect();
                    (name, entries)
                })
                .collect(),
        };
        config.validate()?;
        Ok(config)
    }
}

/// The destination of a parsed key.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    General,
    Sensors,
    FanLevels,
    SmartMode(u8),
    Extra(String),
}

/// The raw, untyped contents of an `.ini` file.
///
/// Entries are kept as ordered lists so that repeated keys (notably the
/// TPFanCtrl2 `Level=` lines) are preserved rather than collapsed.
#[derive(Debug, Default)]
struct RawIni {
    general: Vec<(String, String, usize)>,
    sensors: Vec<(String, String, usize)>,
    fan_levels: Vec<(String, String, usize)>,
    smart_modes: BTreeMap<u8, Vec<(String, String, usize)>>,
    extra_sections: BTreeMap<String, Vec<(String, String, usize)>>,
}

/// Strip an inline `//`, `;`, or `#` comment from a line.
fn strip_inline_comment(line: &str) -> &str {
    let mut end = line.len();
    if let Some(index) = line.find("//") {
        end = end.min(index);
    }
    if let Some(index) = line.find(';') {
        end = end.min(index);
    }
    if let Some(index) = line.find('#') {
        end = end.min(index);
    }
    &line[..end]
}

/// Decide which section a key belongs to, normalising TPFanCtrl2 aliases.
fn route(section: Option<&str>, key: &str) -> (Target, String) {
    match section {
        Some("general") => (Target::General, key.to_string()),
        Some("sensors") => (Target::Sensors, key.to_string()),
        Some("fanlevels") => (Target::FanLevels, key.to_string()),
        Some("smartmode") => (Target::SmartMode(1), normalize_smart_key(key)),
        Some("smartmode2") => (Target::SmartMode(2), normalize_smart_key(key)),
        Some(other) => (Target::Extra(other.to_string()), key.to_string()),
        None => match key {
            "level" => (Target::SmartMode(1), "level".to_string()),
            "level2" => (Target::SmartMode(2), "level".to_string()),
            "menulabelsm1" => (Target::SmartMode(1), "label".to_string()),
            "menulabelsm2" => (Target::SmartMode(2), "label".to_string()),
            _ if key == "ignoresensors" || key.starts_with("sensorname") => {
                (Target::Sensors, key.to_string())
            }
            _ => (Target::General, key.to_string()),
        },
    }
}

/// Normalise smart-mode key aliases within an explicit section.
fn normalize_smart_key(key: &str) -> String {
    match key {
        "menulabelsm1" | "menulabelsm2" => "label".to_string(),
        other => other.to_string(),
    }
}

/// Parse the raw `.ini` text into untyped maps.
fn parse_raw(input: &str) -> Result<RawIni, ConfigError> {
    let mut raw = RawIni::default();
    let mut section: Option<String> = None;

    for (index, raw_line) in input.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with(';') || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        if trimmed.starts_with('[') {
            let Some(end) = trimmed.find(']') else {
                return Err(ConfigError::MalformedSection {
                    line: line_number,
                    content: trimmed.to_string(),
                });
            };
            let name = trimmed[1..end].trim().to_ascii_lowercase();
            if name.is_empty() {
                return Err(ConfigError::MalformedSection {
                    line: line_number,
                    content: trimmed.to_string(),
                });
            }
            section = Some(name);
            continue;
        }

        let content = strip_inline_comment(trimmed).trim();
        if content.is_empty() {
            continue;
        }
        let Some((key, value)) = content.split_once('=') else {
            return Err(ConfigError::MalformedLine {
                line: line_number,
                content: trimmed.to_string(),
            });
        };
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            return Err(ConfigError::MalformedLine {
                line: line_number,
                content: trimmed.to_string(),
            });
        }
        let value = value.trim().to_string();

        let (target, normalized_key) = route(section.as_deref(), &key);
        match target {
            Target::General => {
                raw.general.push((normalized_key, value, line_number));
            }
            Target::Sensors => {
                raw.sensors.push((normalized_key, value, line_number));
            }
            Target::FanLevels => {
                raw.fan_levels.push((normalized_key, value, line_number));
            }
            Target::SmartMode(mode) => {
                raw.smart_modes
                    .entry(mode)
                    .or_default()
                    .push((normalized_key, value, line_number));
            }
            Target::Extra(name) => {
                raw.extra_sections.entry(name).or_default().push((
                    normalized_key,
                    value,
                    line_number,
                ));
            }
        }
    }

    Ok(raw)
}

/// Build the typed [`General`] section from its raw entries.
fn build_general(entries: Vec<(String, String, usize)>) -> Result<General, ConfigError> {
    let mut general = General::default();
    let mut extra = BTreeMap::new();
    for (key, value, line) in entries {
        match key.as_str() {
            "active" => general.active = parse_u8(&key, &value, line)?,
            "cycle" => general.cycle = parse_u32(&key, &value, line)?,
            "manfanspeed" => general.man_fan_speed = parse_u8(&key, &value, line)?,
            "manmodeexit" => general.man_mode_exit = parse_u8(&key, &value, line)?,
            "maxreaderrors" => general.max_read_errors = parse_u32(&key, &value, line)?,
            "processpriority" => general.process_priority = parse_u8(&key, &value, line)?,
            "iconlevels" => general.icon_levels = parse_list_u8(&key, &value, line)?,
            "fanbeep" => general.fan_beep = parse_pair_u8(&key, &value, line)?,
            "hotkeys" => general.hotkeys = parse_bool(&key, &value, line)?,
            "stayontop" => general.stay_on_top = parse_bool(&key, &value, line)?,
            "slimdialog" => general.slim_dialog = parse_bool(&key, &value, line)?,
            "bluetoothedr" => general.bluetooth_edr = parse_bool(&key, &value, line)?,
            "noballons" => general.no_balloons = parse_bool(&key, &value, line)?,
            "noextsensor" => general.no_ext_sensor = parse_bool(&key, &value, line)?,
            "startminimized" => general.start_minimized = parse_bool(&key, &value, line)?,
            "iconcycle" => general.icon_cycle = parse_bool(&key, &value, line)?,
            "showtempicon" => general.show_temp_icon = parse_bool(&key, &value, line)?,
            "showbiasedtemps" => general.show_biased_temps = parse_bool(&key, &value, line)?,
            "showall" => general.show_all = parse_bool(&key, &value, line)?,
            "iconcolorfan" => general.icon_color_fan = parse_bool(&key, &value, line)?,
            "lev64norm" => general.lev64_norm = parse_bool(&key, &value, line)?,
            "log2file" => general.log_to_file = parse_bool(&key, &value, line)?,
            "log2csv" => general.log_to_csv = parse_bool(&key, &value, line)?,
            _ => {
                extra.insert(key, value);
            }
        }
    }
    general.extra = extra;
    Ok(general)
}

/// Build the typed [`Sensors`] section from its raw entries.
fn build_sensors(entries: Vec<(String, String, usize)>) -> Result<Sensors, ConfigError> {
    let mut sensors = Sensors::default();
    let mut extra = BTreeMap::new();
    for (key, value, line) in entries {
        if key == "ignoresensors" {
            sensors.ignore = split_values(&value)
                .into_iter()
                .map(str::to_string)
                .collect();
        } else if let Some(rest) = key.strip_prefix("sensorname") {
            let sequence = rest.parse::<u8>().map_err(|_| ConfigError::InvalidValue {
                line,
                key: key.clone(),
                value: value.clone(),
                reason: "expected a sensor sequence number 0..=255".to_string(),
            })?;
            sensors.names.insert(sequence, value);
        } else {
            extra.insert(key, value);
        }
    }
    sensors.extra = extra;
    Ok(sensors)
}

/// Build the typed [`FanLevels`] section from its raw entries.
fn build_fan_levels(entries: Vec<(String, String, usize)>) -> Result<FanLevels, ConfigError> {
    let mut fan_levels = FanLevels::default();
    let mut extra = BTreeMap::new();
    for (key, value, line) in entries {
        if let Some(rest) = key.strip_prefix("level") {
            let level = rest.parse::<u8>().map_err(|_| ConfigError::InvalidValue {
                line,
                key: key.clone(),
                value: value.clone(),
                reason: "expected a fan level 1..=7".to_string(),
            })?;
            let rpm = value
                .parse::<u16>()
                .map_err(|_| ConfigError::InvalidValue {
                    line,
                    key: key.clone(),
                    value: value.clone(),
                    reason: "expected an RPM value 0..=65535".to_string(),
                })?;
            fan_levels.levels.insert(level, rpm);
        } else {
            extra.insert(key, value);
        }
    }
    fan_levels.extra = extra;
    Ok(fan_levels)
}

/// Build the typed smart modes from their raw entries.
fn build_smart_modes(
    map: BTreeMap<u8, Vec<(String, String, usize)>>,
) -> Result<Vec<SmartMode>, ConfigError> {
    let mut modes = Vec::new();
    for entries in map.into_values() {
        let mut mode = SmartMode::default();
        let mut extra = BTreeMap::new();
        for (key, value, line) in entries {
            match key.as_str() {
                "label" => mode.label = Some(value),
                "level" => mode.levels.push(parse_smart_level(&value, line)?),
                _ => {
                    extra.insert(key, value);
                }
            }
        }
        mode.extra = extra;
        modes.push(mode);
    }
    Ok(modes)
}

/// Parse a TPFanCtrl2 `Level=temp fan [hystUp hystDown]` entry.
fn parse_smart_level(value: &str, line: usize) -> Result<SmartLevel, ConfigError> {
    let parts = split_values(value);
    if parts.len() < 2 || parts.len() > 4 {
        return Err(ConfigError::InvalidValue {
            line,
            key: "Level".to_string(),
            value: value.to_string(),
            reason: "expected `temp fan [hystUp hystDown]`".to_string(),
        });
    }
    let parse = |part: &str| -> Result<u8, ConfigError> {
        part.parse::<u8>().map_err(|_| ConfigError::InvalidValue {
            line,
            key: "Level".to_string(),
            value: value.to_string(),
            reason: format!("`{part}` is not an integer 0..=255"),
        })
    };
    let temperature = parse(parts[0])?;
    let fan_level = parse(parts[1])?;
    let hyst_up = parts
        .get(2)
        .map(|part| parse(part))
        .transpose()?
        .unwrap_or(0);
    let hyst_down = parts
        .get(3)
        .map(|part| parse(part))
        .transpose()?
        .unwrap_or(0);
    Ok(SmartLevel {
        temperature,
        fan_level,
        hyst_up,
        hyst_down,
    })
}

/// Split a value on whitespace and commas, dropping empty fields.
fn split_values(value: &str) -> Vec<&str> {
    value
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|part| !part.is_empty())
        .collect()
}

/// Parse a `u8` value.
fn parse_u8(key: &str, value: &str, line: usize) -> Result<u8, ConfigError> {
    value
        .trim()
        .parse::<u8>()
        .map_err(|_| ConfigError::InvalidValue {
            line,
            key: key.to_string(),
            value: value.to_string(),
            reason: "expected an integer 0..=255".to_string(),
        })
}

/// Parse a `u32` value.
fn parse_u32(key: &str, value: &str, line: usize) -> Result<u32, ConfigError> {
    value
        .trim()
        .parse::<u32>()
        .map_err(|_| ConfigError::InvalidValue {
            line,
            key: key.to_string(),
            value: value.to_string(),
            reason: "expected an integer 0..=4294967295".to_string(),
        })
}

/// Parse a boolean value (`1`/`0`, `true`/`false`, `yes`/`no`, `on`/`off`).
fn parse_bool(key: &str, value: &str, line: usize) -> Result<bool, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::InvalidValue {
            line,
            key: key.to_string(),
            value: value.to_string(),
            reason: "expected a boolean (`1`/`0`, `true`/`false`, `yes`/`no`, `on`/`off`)"
                .to_string(),
        }),
    }
}

/// Parse a whitespace/comma separated list of `u8` values.
fn parse_list_u8(key: &str, value: &str, line: usize) -> Result<Vec<u8>, ConfigError> {
    split_values(value)
        .into_iter()
        .map(|part| {
            part.parse::<u8>().map_err(|_| ConfigError::InvalidValue {
                line,
                key: key.to_string(),
                value: value.to_string(),
                reason: format!("`{part}` is not an integer 0..=255"),
            })
        })
        .collect()
}

/// Parse a whitespace/comma separated pair of `u8` values.
fn parse_pair_u8(key: &str, value: &str, line: usize) -> Result<(u8, u8), ConfigError> {
    let parts = split_values(value);
    if parts.len() != 2 {
        return Err(ConfigError::InvalidValue {
            line,
            key: key.to_string(),
            value: value.to_string(),
            reason: "expected exactly two integers".to_string(),
        });
    }
    let first = parse_u8(key, parts[0], line)?;
    let second = parse_u8(key, parts[1], line)?;
    Ok((first, second))
}

/// Render a boolean as `1` or `0`.
fn bool_ini(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

/// Join numbers with single spaces.
fn join_numbers(values: &[u8]) -> String {
    values
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A content fingerprint used to detect configuration changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    hash: u64,
    len: usize,
}

impl FileStamp {
    /// Fingerprint the given file contents.
    fn from_contents(contents: &str) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        contents.hash(&mut hasher);
        Self {
            hash: hasher.finish(),
            len: contents.len(),
        }
    }
}

/// Watches a configuration file and reloads it when its contents change.
///
/// The watcher is poll-based: call [`ConfigWatcher::poll`] on the daemon's
/// control-loop tick. It compares a content fingerprint, so it is immune to
/// filesystem timestamp granularity and works on any filesystem.
#[derive(Debug)]
pub struct ConfigWatcher {
    path: PathBuf,
    last: Option<FileStamp>,
}

impl ConfigWatcher {
    /// Create a watcher for the given path. The first [`ConfigWatcher::poll`]
    /// always reports a change.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            last: None,
        }
    }

    /// The watched path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reload the configuration if its contents changed since the last poll.
    ///
    /// Returns `Ok(None)` when the file is unchanged, and `Ok(Some(config))`
    /// when a new, validated configuration was loaded.
    pub fn poll(&mut self) -> Result<Option<Config>, ConfigError> {
        let contents = std::fs::read_to_string(&self.path).map_err(|source| ConfigError::Io {
            path: self.path.clone(),
            source,
        })?;
        let stamp = FileStamp::from_contents(&contents);
        if self.last == Some(stamp) {
            return Ok(None);
        }
        let config: Config = contents.parse()?;
        self.last = Some(stamp);
        Ok(Some(config))
    }

    /// Force a reload regardless of whether the contents changed.
    pub fn reload(&mut self) -> Result<Config, ConfigError> {
        let contents = std::fs::read_to_string(&self.path).map_err(|source| ConfigError::Io {
            path: self.path.clone(),
            source,
        })?;
        let config: Config = contents.parse()?;
        self.last = Some(FileStamp::from_contents(&contents));
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// The real TPFanCtrl2 sample shipped in `config/TPFanControl.ini`.
    const TPFANCTRL2_SAMPLE: &str = r#"// ================================================
// TPFanControl 2.3.3 - Custom Smart Mode config
// Fan: Level 2 below 60°C, Level 3 at 60°C and above
// "pwr" sensor (sequence 11) is now ignored
// ================================================

Hotkeys=1
Active=2
ManFanSpeed=0
ManModeExit=78
StayOnTop=1
SlimDialog=0
BluetoothEDR=0
ProcessPriority=2
NoBallons=1
Cycle=5
NoExtSensor=0
StartMinimized=1
IconCycle=1
ShowTempIcon=1
IconLevels=65 75 80
FanBeep=0 0
MaxReadErrors=10
Log2File=0
Log2csv=0

// Disable "pwr" sensor (sequence 11)
IgnoreSensors=pwr,no5,x7d,aps,bus,pci,xc3

// Sensor names (optional, for clarity)
SensorName5=no5
//SensorName11=pwr   // commented out - sensor is ignored

ShowBiasedTemps=1
ShowAll=0
IconColorFan=1
Lev64Norm=1

// ================================================
// Smart Mode 1 - Main Profile
// ================================================
MenuLabelSM1=Smart Mode 1/

// Level=temp  fan  hystUp  hystDown
Level=50  2  0  0     // Below ~60°C → Fan level 2
Level=60  3  0  0     // 60°C and above → Fan level 3

// ================================================
// Optional: Smart Mode 2 (disabled by default)
// ================================================
// MenuLabelSM2=Smart Mode 2/
// Level2=50 2 0 0
// Level2=60 3 0 0
"#;

    /// The shipped device profile in `data/profiles/e14-gen4.ini`.
    const E14_GEN4_PROFILE: &str = r#"// ================================================
// omafanctrl device profile — ThinkPad E14 Gen 4
// ================================================

Hotkeys=1
Active=2
ManFanSpeed=0
ManModeExit=78
StayOnTop=1
SlimDialog=0
BluetoothEDR=0
ProcessPriority=2
NoBallons=1
Cycle=5
NoExtSensor=0
StartMinimized=1
IconCycle=1
ShowTempIcon=1
IconLevels=65 75 80
FanBeep=0 0
MaxReadErrors=10
Log2File=0
Log2csv=0

IgnoreSensors=pwr,no5,x7d,aps,bus,pci,xc3

SensorName5=no5

ShowBiasedTemps=1
ShowAll=0
IconColorFan=1
Lev64Norm=1

MenuLabelSM1=Smart Mode 1/

Level=45  1  0  0
Level=50  2  0  0
Level=60  3  0  0
"#;

    fn temp_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "omafanctrl-config-{}-{tag}-{unique}.ini",
            std::process::id()
        ))
    }

    #[test]
    fn parses_the_tpfanctrl2_sample() {
        let config: Config = TPFANCTRL2_SAMPLE.parse().unwrap();
        assert_eq!(config.general.active, 2);
        assert_eq!(config.general.cycle, 5);
        assert_eq!(config.general.icon_levels, vec![65, 75, 80]);
        assert_eq!(config.general.fan_beep, (0, 0));
        assert!(config.general.hotkeys);
        assert!(!config.general.show_all);
        assert_eq!(
            config.sensors.ignore,
            vec!["pwr", "no5", "x7d", "aps", "bus", "pci", "xc3"]
        );
        assert_eq!(
            config.sensors.names.get(&5).map(String::as_str),
            Some("no5")
        );
        // The commented-out SensorName11 must not be parsed.
        assert!(!config.sensors.names.contains_key(&11));
        assert_eq!(config.smart_modes.len(), 1);
        let mode = &config.smart_modes[0];
        assert_eq!(mode.label.as_deref(), Some("Smart Mode 1/"));
        assert_eq!(
            mode.levels,
            vec![
                SmartLevel {
                    temperature: 50,
                    fan_level: 2,
                    hyst_up: 0,
                    hyst_down: 0
                },
                SmartLevel {
                    temperature: 60,
                    fan_level: 3,
                    hyst_up: 0,
                    hyst_down: 0
                },
            ]
        );
    }

    #[test]
    fn parses_the_e14_gen4_profile() {
        let config: Config = E14_GEN4_PROFILE.parse().unwrap();
        assert_eq!(config.smart_modes[0].levels.len(), 3);
        assert_eq!(config.smart_modes[0].levels[0].temperature, 45);
        // No explicit [FanLevels] section, so the verified default curve applies.
        assert_eq!(config.fan_levels.levels.get(&1), Some(&1800));
        assert_eq!(config.fan_levels.levels.get(&3), Some(&3900));
    }

    #[test]
    fn shipped_profile_on_disk_parses() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/profiles/e14-gen4.ini");
        let config = Config::from_file(&path).unwrap();
        // The shipped profile carries the verified fan curve explicitly.
        assert_eq!(config.fan_levels.levels.get(&1), Some(&1800));
        assert_eq!(config.fan_levels.levels.get(&2), Some(&2200));
        assert_eq!(config.fan_levels.levels.get(&3), Some(&3900));
        assert_eq!(config.smart_modes[0].levels.len(), 3);
        assert_eq!(config.general.active, 2);
    }

    #[test]
    fn round_trips_the_tpfanctrl2_sample() {
        let config: Config = TPFANCTRL2_SAMPLE.parse().unwrap();
        let rendered = config.to_ini();
        let reparsed: Config = rendered.parse().unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn round_trips_the_e14_gen4_profile() {
        let config: Config = E14_GEN4_PROFILE.parse().unwrap();
        let rendered = config.to_ini();
        let reparsed: Config = rendered.parse().unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn keys_are_case_insensitive() {
        let config: Config = "[GENERAL]\nACTIVE=1\nCYCLE=9\n".parse().unwrap();
        assert_eq!(config.general.active, 1);
        assert_eq!(config.general.cycle, 9);
    }

    #[test]
    fn supports_semicolon_and_hash_comments() {
        let input = "; a comment\n# another\nActive=1 ; inline\nCycle=7 # inline\n";
        let config: Config = input.parse().unwrap();
        assert_eq!(config.general.active, 1);
        assert_eq!(config.general.cycle, 7);
    }

    #[test]
    fn tolerates_whitespace_around_keys_and_values() {
        let config: Config = "  Active = 3  \n\tCycle\t=\t12\n".parse().unwrap();
        assert_eq!(config.general.active, 3);
        assert_eq!(config.general.cycle, 12);
    }

    #[test]
    fn parses_explicit_sections() {
        let input = "\
[General]
Active=1
Cycle=4

[Sensors]
IgnoreSensors=pwr,no5

[FanLevels]
Level1=1800
Level2=2200
Level3=3900

[SmartMode]
Label=Custom/
Level=40 1 0 0
Level=70 3 0 0
";
        let config: Config = input.parse().unwrap();
        assert_eq!(config.general.active, 1);
        assert_eq!(config.sensors.ignore, vec!["pwr", "no5"]);
        assert_eq!(config.fan_levels.levels.get(&2), Some(&2200));
        assert_eq!(config.smart_modes[0].label.as_deref(), Some("Custom/"));
        assert_eq!(config.smart_modes[0].levels[1].fan_level, 3);
    }

    #[test]
    fn parses_two_smart_modes() {
        let input = "\
MenuLabelSM1=One/
Level=50 2 0 0
MenuLabelSM2=Two/
Level2=55 3 0 0
";
        let config: Config = input.parse().unwrap();
        assert_eq!(config.smart_modes.len(), 2);
        assert_eq!(config.smart_modes[0].label.as_deref(), Some("One/"));
        assert_eq!(config.smart_modes[1].label.as_deref(), Some("Two/"));
        assert_eq!(config.smart_modes[1].levels[0].fan_level, 3);
    }

    #[test]
    fn preserves_unknown_keys_and_sections() {
        let input = "\
[General]
Active=1
CustomThing=hello

[Weird]
Foo=bar
";
        let config: Config = input.parse().unwrap();
        assert_eq!(
            config.general.extra.get("customthing").map(String::as_str),
            Some("hello")
        );
        assert_eq!(
            config
                .extra_sections
                .get("weird")
                .and_then(|section| section.get("foo"))
                .map(String::as_str),
            Some("bar")
        );
        let reparsed: Config = config.to_ini().parse().unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn rejects_malformed_lines() {
        let error = "Active=1\nthis is not a pair\n"
            .parse::<Config>()
            .unwrap_err();
        assert!(matches!(error, ConfigError::MalformedLine { line: 2, .. }));
    }

    #[test]
    fn rejects_unterminated_sections() {
        let error = "[General\nActive=1\n".parse::<Config>().unwrap_err();
        assert!(matches!(
            error,
            ConfigError::MalformedSection { line: 1, .. }
        ));
    }

    #[test]
    fn rejects_invalid_values_with_line_numbers() {
        let error = "Active=1\nCycle=soon\n".parse::<Config>().unwrap_err();
        match error {
            ConfigError::InvalidValue { line, key, .. } => {
                assert_eq!(line, 2);
                assert_eq!(key, "cycle");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn rejects_out_of_range_active() {
        let error = "Active=9\n".parse::<Config>().unwrap_err();
        assert!(matches!(
            error,
            ConfigError::Validation(ValidationError::ActiveOutOfRange(9))
        ));
    }

    #[test]
    fn rejects_zero_cycle() {
        let error = "Cycle=0\n".parse::<Config>().unwrap_err();
        assert!(matches!(
            error,
            ConfigError::Validation(ValidationError::CycleTooSmall(0))
        ));
    }

    #[test]
    fn rejects_non_ascending_smart_temperatures() {
        let input = "Level=60 3 0 0\nLevel=50 2 0 0\n";
        let error = input.parse::<Config>().unwrap_err();
        assert!(matches!(
            error,
            ConfigError::Validation(ValidationError::SmartTemperaturesNotAscending { mode: 1, .. })
        ));
    }

    #[test]
    fn rejects_out_of_range_smart_fan_level() {
        let input = "Level=50 9 0 0\n";
        let error = input.parse::<Config>().unwrap_err();
        assert!(matches!(
            error,
            ConfigError::Validation(ValidationError::SmartFanLevelOutOfRange { mode: 1, level: 9 })
        ));
    }

    #[test]
    fn rejects_bad_icon_levels() {
        let error = "IconLevels=80 70 60\n".parse::<Config>().unwrap_err();
        assert!(matches!(
            error,
            ConfigError::Validation(ValidationError::IconLevelsNotAscending(_))
        ));
        let error = "IconLevels=65 75\n".parse::<Config>().unwrap_err();
        assert!(matches!(
            error,
            ConfigError::Validation(ValidationError::IconLevelCount(2))
        ));
    }

    #[test]
    fn accepts_two_value_smart_levels() {
        let config: Config = "Level=50 2\n".parse().unwrap();
        assert_eq!(
            config.smart_modes[0].levels[0],
            SmartLevel {
                temperature: 50,
                fan_level: 2,
                hyst_up: 0,
                hyst_down: 0
            }
        );
    }

    #[test]
    fn loads_from_a_file() {
        let path = temp_path("load");
        std::fs::write(&path, "Active=1\nCycle=3\n").unwrap();
        let config = Config::from_file(&path).unwrap();
        assert_eq!(config.general.active, 1);
        assert_eq!(config.general.cycle, 3);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn reports_missing_file() {
        let path = temp_path("missing");
        let error = Config::from_file(&path).unwrap_err();
        assert!(matches!(error, ConfigError::Io { .. }));
    }

    #[test]
    fn watcher_reports_first_load_then_only_changes() {
        let path = temp_path("watch");
        std::fs::write(&path, "Active=1\nCycle=3\n").unwrap();
        let mut watcher = ConfigWatcher::new(&path);

        let first = watcher.poll().unwrap();
        assert_eq!(first.unwrap().general.active, 1);
        // Unchanged contents: no reload.
        assert!(watcher.poll().unwrap().is_none());

        std::fs::write(&path, "Active=2\nCycle=3\n").unwrap();
        let changed = watcher.poll().unwrap();
        assert_eq!(changed.unwrap().general.active, 2);
        assert!(watcher.poll().unwrap().is_none());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn watcher_force_reload_ignores_fingerprint() {
        let path = temp_path("reload");
        std::fs::write(&path, "Active=1\n").unwrap();
        let mut watcher = ConfigWatcher::new(&path);
        watcher.poll().unwrap();
        let reloaded = watcher.reload().unwrap();
        assert_eq!(reloaded.general.active, 1);
        std::fs::remove_file(&path).ok();
    }
}
