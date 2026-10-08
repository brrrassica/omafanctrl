# Fuzzing the `.ini` parser

This directory contains a [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz)
target for `omafanctrl-core`'s TPFanCtrl2 `.ini` parser.

The parser must never panic on arbitrary input, and any configuration that
parses successfully must round-trip losslessly through `Config::to_ini`.

## Running

`cargo-fuzz` requires a nightly toolchain and the `cargo-fuzz` subcommand:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz

cd fuzz
cargo +nightly fuzz run parse_config
```

To run for a fixed number of iterations (useful in a release check):

```sh
cargo +nightly fuzz run parse_config -- -runs=1000000
```

To reproduce a crash, pass the saved artifact:

```sh
cargo +nightly fuzz run parse_config fuzz/artifacts/parse_config/<artifact>
```

## Continuous coverage on stable

CI runs on stable Rust and cannot execute `cargo-fuzz`. The equivalent
property is covered by the deterministic
`fuzz_parser_never_panics_and_round_trips` test in
[`crates/omafanctrl-core/src/config.rs`](../crates/omafanctrl-core/src/config.rs),
which drives the parser with a fixed-seed PRNG and a corpus of structurally
interesting seeds. It runs as part of `cargo test --all`.
