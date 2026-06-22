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
| `hos-extension` | Marketplace authority. Registry-gated `force_transfer` and `sweep_ft`; sub-account reclaim is registry-side via `park_wallet`. |
| `mpc-recovery` | Opt-in account recovery. Timelocked and watcher-quorum-verified, with two target modes: wallet (ends in an `active-signer` owner swap guarded by a compare-and-swap) and native (MPC `AddKey` on a raw NEAR account via `v1.signer`). See [THREAT_MODEL.md](THREAT_MODEL.md). |
| `hos-wallet` | Vendored fork of the Defuse `wallet-no-sign` contract. The signing path always rejects; all authority flows through the extensions. Carries its own README and LICENSE. |

`dev-contracts/test-ft` is a minimal fungible token used only by the integration
tests. `crates/hos-common` holds pure helpers shared across the contracts.
`integration/` holds the near-workspaces sandbox suite.

## Recovery

`mpc-recovery` is opt-in per account and supports two target modes, fixed at policy
install:

- **Wallet** (default for managed sub-accounts): recovery rotates the
  `active-signer` owner key via `swap_owner`, guarded by a compare-and-swap on the
  current owner and serialized against sales by a freeze flag.
- **Native**: recovery adds a FullAccess key to a raw NEAR account via an `AddKey`
  transaction signed by NEAR Chain Signatures (`v1.signer`). This grants
  protocol-level control and depends on the MPC signer's availability.

Both modes require a watcher-quorum verdict after a per-policy timelock. The
security model and the Native-specific trust assumptions are in
[THREAT_MODEL.md](THREAT_MODEL.md).

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

## Conventions

Two error idioms are used deliberately. `tla-registry` and `hos-extension` return a
typed `ContractError` through `#[handle_result]`; `active-signer`, `mpc-recovery`, and
`tla-manager` use `require!` / `panic_str` with `&str` error constants. The chain id is
pinned to `mainnet` in `active-signer` (and the vendored wallet); a testnet build must
change that constant.

## License

The House of Stake contracts (`active-signer`, `hos-extension`, `mpc-recovery`,
`tla-manager`, `tla-registry`, and `dev-contracts/test-ft`) are licensed under
either of MIT or Apache-2.0 at your option. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).

`contracts/hos-wallet` is a fork of the Defuse `wallet-no-sign` contract
(`near/intents`) and remains under its upstream MIT license, Copyright (c) 2025
NEAR Foundation. See [contracts/hos-wallet/LICENSE](contracts/hos-wallet/LICENSE).

## Deploy

[DEPLOY.md](DEPLOY.md) has the deploy sequence, the fixed-at-init contract wiring,
the per-TLA setup, and the governance handoff to the admin multisig.

## Status

Pre-audit. Not yet deployed to mainnet.
