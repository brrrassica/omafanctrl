//! Fuzz target for the TPFanCtrl2 `.ini` parser.
//!
//! The parser must never panic on arbitrary input: malformed input is reported
//! as a [`ConfigError`], never as a crash. When a configuration parses
//! successfully it must also round-trip losslessly through [`Config::to_ini`].
//!
//! Run with:
//!
//! ```sh
//! cargo +nightly fuzz run parse_config
//! ```
//!
//! [`ConfigError`]: omafanctrl_core::config::ConfigError
//! [`Config::to_ini`]: omafanctrl_core::config::Config::to_ini

#![no_main]

use libfuzzer_sys::fuzz_target;
use omafanctrl_core::config::Config;

fuzz_target!(|data: &[u8]| {
    // The parser operates on `&str`; skip inputs that are not valid UTF-8.
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };

    if let Ok(config) = input.parse::<Config>() {
        // A successfully parsed configuration must serialise and re-parse to an
        // equal value. A failure here is a real round-trip bug.
        let rendered = config.to_ini();
        let reparsed: Config = rendered
            .parse()
            .expect("a serialised configuration must re-parse");
        assert_eq!(config, reparsed, "round-trip mismatch");
    }
});
