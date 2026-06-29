# Version lock: Valhalla audit

Built from the `valhalla-audit` tag.

## Toolchain and build

- Rust 1.86.0 (`rust-toolchain.toml`).
- Reproducible build image `sourcescan/cargo-near:0.19.0-rust-1.86.0`,
  digest `sha256:772638e343baeeea24e49062c7d424274f3441452cc06ce97fc4e5695b19fecc`.
- Per contract crate: `cargo near build reproducible-wasm`, which runs the build
  `--locked` inside the pinned container. `Cargo.lock` is committed.

## Contract wasm sha256

| Crate | wasm | sha256 |
|---|---|---|
| tla-registry | tla_registry.wasm | `966d387422a2d7443fcb17803050ed2634674acf028df68f19e1b3526d8d2b2c` |
| tla-manager | tla_manager.wasm | `ce551cbe26ea31f9b10690894db447590840c5834534c14a31736456eb1041ee` |
| active-signer | active_signer.wasm | `827617b6adacf1332e34c80cf7579cdbe291823669be5baa54e9abe89ea1966f` |
| hos-extension | hos_extension.wasm | `48d74206912be00e15bef2a955171e0b0ccd1062de9295c55c5e24d5ea7994c6` |
| mpc-recovery | mpc_recovery.wasm | `b95d5d564fa52ade7d967dca70a61639fd0d95890991126e06cb8d20bdd16e0f` |
| hos-wallet | hos_wallet.wasm | `54af27cd741d02cd24cd598b3371e1de0955811a6fde9d6eeb53a14b578c3b1c` |

`dev-contracts/test-ft` and the `integration/` workspace are out of audit scope and
are not part of this lock.

To verify: check out the `valhalla-audit` tag and run `cargo near build reproducible-wasm`
in each contract crate. The wasm sha256 must match the table above.
