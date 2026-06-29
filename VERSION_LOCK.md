# Version lock: Valhalla audit

Built from the `valhalla-audit` tag.

## Toolchain and build

- Rust 1.86.0 (`rust-toolchain.toml`).
- Reproducible build image `sourcescan/cargo-near:0.19.0-rust-1.86.0`,
  digest `sha256:772638e343baeeea24e49062c7d424274f3441452cc06ce97fc4e5695b19fecc`.
- Per contract crate: `cargo near build reproducible-wasm`, which runs the build
  `--locked` inside the pinned container. `Cargo.lock` is committed.

## Contract wasm sha256

The per-crate sha256 are recorded in the annotated `valhalla-audit` tag message
(`git show valhalla-audit`), built at the exact tag commit. They are kept in the tag
rather than in this file on purpose: cargo-near stamps each wasm with NEP-330
`contract_source_metadata` that embeds the source commit hash, so the bytes are
commit-specific and any commit that recorded the hashes in-tree would invalidate them.

`dev-contracts/test-ft` and the `integration/` workspace are out of audit scope.

To verify: check out the `valhalla-audit` tag, run `cargo near build reproducible-wasm`
in each contract crate, and compare each `target/near/<crate>/<crate>.wasm` sha256
against the table in `git show valhalla-audit`.
