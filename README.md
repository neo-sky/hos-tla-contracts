# House of Stake TLA Contracts

NEAR smart contracts for the House of Stake top-level-account (TLA) marketplace
and opt-in account recovery. A TLA owner rents sub-accounts under their name; each
rented account is a plain NEAR account whose only FullAccess key is derived by the
NEAR MPC network (`v1.signer`, ed25519 domain 1) under the path `hos-tla/<account>`
with `active-signer` as the predecessor. The renter never holds the key: the MPC
network only signs what `active-signer` asks it to, so every use of the key runs
through House of Stake signing policy. Because the key is a real FullAccess key,
NEP-413 ownership proof (`verifyFullKeyBelongsToUser`) passes and any Wallet
Selector dApp accepts the account natively, while resale, reclaim, and recovery
remain a storage swap of the operating key in `active-signer`.

## Contracts

| Crate | Role |
|-------|------|
| `tla-registry` | Marketplace orchestrator: TLA records, sub-account rentals, resale listings, fee tiers, FT allowlist, and refund accounting. |
| `tla-manager` | Per-TLA mint primitive. Derives the account's MPC key, creates the sub-account, funds it, adds the derived FullAccess key, and installs the operating key in `active-signer` in one promise chain. |
| `active-signer` | Per-account signing authority (ed25519). Holds the operating key and nonces, builds NEAR transactions and NEP-413 payloads in-contract and has the MPC signer sign them, and gates owner swap and freeze. |
| `hos-extension` | Marketplace authority. Registry-gated `force_transfer` (an owner-key swap) and `sweep_ft` (an MPC-signed `ft_transfer` handed back for broadcast); sub-account reclaim is registry-side via `park_wallet`. |
| `mpc-recovery` | Opt-in account recovery. Timelocked and watcher-quorum-verified, with two target modes: wallet (ends in an `active-signer` owner swap guarded by a compare-and-swap) and native (MPC `AddKey` on a raw NEAR account via `v1.signer`). See [THREAT_MODEL.md](THREAT_MODEL.md). |

`dev-contracts/test-ft` is a minimal fungible token and `dev-contracts/test-mpc` a
sandbox stand-in for `v1.signer`; both are used only by the integration tests.
`crates/hos-common` holds pure helpers shared across the contracts, including the
hand-rolled borsh NEAR transaction builder (byte-exact against `near-api-js`
golden vectors). `integration/` holds the near-workspaces sandbox suite.

## Signing

A renter drives their account through `active-signer`, never with the key itself:

- `submit_signed_tx` takes a renter-signed `TxMessage` envelope (domain
  `NEAR_HOS_ACTIVE_SIGNER_TX/V1`), builds the exact NEAR transaction with the
  account's MPC key, and has `v1.signer` sign its sha256. It returns the payload
  hash, the unsigned transaction hex, and the MPC signature for a relay to
  assemble and broadcast. The action set (`TxAction`) contains only `FunctionCall`
  and `Transfer`, so a signed request can never add or delete keys, delete the
  account, or deploy code; the escape hatch is closed at the type level.
- `submit_signed_message` builds a standard NEP-413 tagged payload in-contract
  (domain `NEAR_HOS_ACTIVE_SIGNER_MSG/V1`) and MPC-signs it, which is what gives
  the account ordinary dApp sign-in.
- `authority_sign_tx` is marketplace-authority-gated and drives the reclaim FT
  sweep.

The relay supplies the transaction nonce and a recent block hash. Neither is
authenticated and neither needs to be: the actions come from the renter's signed
envelope, so a bad value can only invalidate the transaction, never change what
it does.

## Recovery

`mpc-recovery` is opt-in per account and supports two target modes, fixed at policy
install:

- **Wallet** (default for managed sub-accounts): recovery rotates the
  `active-signer` operating key via `swap_owner`, guarded by a compare-and-swap on
  the current owner and serialized against sales by a freeze flag.
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

Each crate's exact reproducible-build command is recorded in its
`[package.metadata.near.reproducible_build]`.

## Checks

Format and lint:

    cargo fmt --check -p hos-common -p active-signer -p hos-extension -p mpc-recovery -p tla-manager -p tla-registry -p test-ft
    cargo clippy --workspace --all-targets -- -D warnings

Unit tests run across the workspace:

    cargo test --workspace

Integration tests live in a separate crate (excluded from the workspace) and spin
up a NEAR sandbox via near-workspaces, exercising the mint, signing, marketplace,
and recovery flows end to end, including broadcasting MPC-signed transactions
against the sandbox:

    cargo test --manifest-path integration/Cargo.toml

## Conventions

Two error idioms are used deliberately. `tla-registry` and `hos-extension` return a
typed `ContractError` through `#[handle_result]`; `active-signer`, `mpc-recovery`, and
`tla-manager` use `require!` / `panic_str` with `&str` error constants. The chain id is
pinned to `mainnet` in `active-signer`; a testnet build must change that constant.

## License

Licensed under either of MIT or Apache-2.0 at your option. See
[LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).

## Deploy

[DEPLOY.md](DEPLOY.md) has the deploy sequence, the fixed-at-init contract wiring,
the per-TLA setup, and the governance handoff to the admin multisig.

## Status

Pre-audit. Not yet deployed to mainnet.
