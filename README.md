# House of Stake TLA Contracts

NEAR smart contracts for the House of Stake top-level-account (TLA) marketplace
and opt-in account recovery. A TLA owner rents sub-accounts under their name; each
rented account runs a wallet contract whose authority is mediated entirely by a set
of House of Stake singleton extensions, never by a raw FullAccess key.

## Contracts

| Crate | Role |
|-------|------|
| `tla-registry` | Marketplace orchestrator: TLA records, sub-account rentals, resale listings, fee tiers, FT allowlist, and refund accounting. |
| `tla-manager` | Per-TLA mint primitive. Creates the sub-account, deploys the wallet contract, and installs the extensions in one promise chain. |
| `active-signer` | Per-wallet signing authority (ed25519). Holds the owner key and nonce, executes signed requests against the wallet, and gates owner swap and freeze. |
| `hos-extension` | Marketplace authority. Registry-gated `force_transfer`, `sweep_ft`, and `reclaim_and_delete`. |
| `mpc-recovery` | Opt-in account recovery. Timelocked and attestation-verified, ending in an `active-signer` owner swap guarded by a compare-and-swap. |
| `hos-wallet` | Vendored fork of the Defuse `wallet-no-sign` contract. The signing path always rejects; all authority flows through the extensions. Carries its own README and LICENSE. |

`dev-contracts/test-ft` is a minimal fungible token used only by the integration
tests. `crates/hos-common` holds pure helpers shared across the contracts.
`integration/` holds the near-workspaces sandbox suite.

## Build

The toolchain is pinned: Rust 1.86 (the nearcore VM rejects wasm produced by 1.87+)
and near-sdk 5.26.1. Build a contract to wasm from its crate directory under
`contracts/`:

    cargo near build non-reproducible-wasm --no-abi

The vendored `hos-wallet` fork is feature-gated and must be built with its contract
feature:

    cargo near build non-reproducible-wasm --locked --no-default-features --features=contract --no-abi

Each crate's exact reproducible-build command is recorded in its
`[package.metadata.near.reproducible_build]`.

## Checks

Format and lint the House of Stake crates. The vendored `hos-wallet` fork keeps
upstream formatting and is excluded from the format gate by package selection
(`cargo fmt` does not honor rustfmt `ignore` directives):

    cargo fmt --check -p hos-common -p active-signer -p hos-extension -p mpc-recovery -p tla-manager -p tla-registry -p test-ft
    cargo clippy --workspace --all-targets -- -D warnings

Unit tests run across the workspace:

    cargo test --workspace

Integration tests live in a separate crate (excluded from the workspace) and spin
up a NEAR sandbox via near-workspaces, exercising the mint, signing, marketplace,
and recovery flows end to end:

    cargo test --manifest-path integration/Cargo.toml

## License

The House of Stake contracts (`active-signer`, `hos-extension`, `mpc-recovery`,
`tla-manager`, `tla-registry`, and `dev-contracts/test-ft`) are licensed under
either of MIT or Apache-2.0 at your option. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).

`contracts/hos-wallet` is a fork of the Defuse `wallet-no-sign` contract
(`near/intents`) and remains under its upstream MIT license, Copyright (c) 2025
NEAR Foundation. See [contracts/hos-wallet/LICENSE](contracts/hos-wallet/LICENSE).

## Status

Pre-audit. Not yet deployed to mainnet.
