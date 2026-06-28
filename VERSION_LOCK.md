# Version lock: Valhalla audit

Built from tag `valhalla-audit` (commit `acb74dc`).

## Toolchain and build

- Rust 1.86.0 (`rust-toolchain.toml`).
- Reproducible build image `sourcescan/cargo-near:0.19.0-rust-1.86.0`,
  digest `sha256:772638e343baeeea24e49062c7d424274f3441452cc06ce97fc4e5695b19fecc`.
- Per contract crate: `cargo near build reproducible-wasm`, which runs the build
  `--locked` inside the pinned container. `Cargo.lock` is committed.

## Contract wasm sha256

| Crate | wasm | sha256 |
|---|---|---|
| tla-registry | tla_registry.wasm | `c2fa36658334beff1084789f48a66f8ce35c36dc6f6aed838a418285106acf73` |
| tla-manager | tla_manager.wasm | `3f3b569348ade43c948157042e8bb97d39349f0173c249410718945f6b8bc303` |
| active-signer | active_signer.wasm | `190ef085a16ee36a5d094bb7b37b4b4cd25de95a5f2deb2de01a4ca67035c7db` |
| hos-extension | hos_extension.wasm | `d2bb98e85d318bf0f6053cb2777cf33cdeb07efea7c2491fccf836767cfcc032` |
| mpc-recovery | mpc_recovery.wasm | `eb6bf71777ce423bf7bc3a0dd8fd783a963d301321429f2c2fa30fe97b10463c` |
| hos-wallet | hos_wallet.wasm | `134353e99da7bd2e3266679d06f7792b0b702067c0e74df38444ec38254c96f9` |

`dev-contracts/test-ft` and the `integration/` workspace are out of audit scope and
are not part of this lock.

To verify: check out the `valhalla-audit` tag and run `cargo near build reproducible-wasm`
in each contract crate. The wasm sha256 must match the table above.
