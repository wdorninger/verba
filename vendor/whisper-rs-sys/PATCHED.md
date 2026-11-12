# Patched whisper-rs-sys 0.15.0

Copy of the crates.io release with one change: `src/bindings.rs` (generated on Linux, where `long` is 64-bit)
has `c_long`/`c_ulong` replaced by `i64`/`u64`. Only glibc typedefs such as `__int64_t` used them, so this gives
the correct layout on Windows too (`int64_t` stays 64-bit). Together with `WHISPER_DONT_GENERATE_BINDINGS=1`
(see `.cargo/config.toml`) the build no longer runs bindgen and needs no libclang.

To update: copy the new crate from `~/.cargo/registry/src/*/whisper-rs-sys-<version>` and redo the replacement:
`sed -i 's/::std::os::raw::c_long\b/i64/g; s/::std::os::raw::c_ulong\b/u64/g' src/bindings.rs`

Second change: C enum typedefs (types that have `pub const NAME_X: NAME = ...` values) are `c_int` instead of
`c_uint`, as MSVC (and whisper-rs on Windows) expects:
`perl -0pi -e 'my %e; while (/pub const \w+: (\w+) = /g) { $e{$1}=1 } for my $t (keys %e) { s/pub type \Q$t\E = ::std::os::raw::c_uint;/pub type $t = ::std::os::raw::c_int;/ }' src/bindings.rs`
