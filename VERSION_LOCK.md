# Version lock: Valhalla audit

Audit builds are cut from an annotated tag; the hand-off tree is whatever that tag
points at.

## Toolchain and build

- Rust 1.86.0 (`rust-toolchain.toml`).
- Reproducible build image `sourcescan/cargo-near:0.19.0-rust-1.86.0`,
  digest `sha256:772638e343baeeea24e49062c7d424274f3441452cc06ce97fc4e5695b19fecc`.
- Per contract crate: `cargo near build reproducible-wasm`, which runs the build
  `--locked` inside the pinned container. `Cargo.lock` is committed.

## Contract wasm sha256

The existing `valhalla-audit` tag predates the MPC FullAccess key rework (the
hos-wallet contract is gone and every remaining contract changed), so the hashes
recorded in that tag no longer describe this tree. Fresh hashes are generated when
the next audit tag is cut. They will live in the annotated tag message rather than
in this file, on purpose: cargo-near stamps each wasm with NEP-330
`contract_source_metadata` that embeds the source commit hash, so the bytes are
commit-specific and any commit that recorded the hashes in-tree would invalidate
them.

`dev-contracts/` (`test-ft`, `test-mpc`) and the `integration/` workspace are out
of audit scope.

To verify a tagged build: check out the tag, run `cargo near build reproducible-wasm`
in each contract crate, and compare each `target/near/<crate>/<crate>.wasm` sha256
against the table in the tag message (`git show <tag>`).
