# Threat Model

This document covers the security model of the House of Stake TLA contract suite,
with emphasis on the opt-in account recovery system (`mpc-recovery`) and its two
target modes.

## Trust boundaries

- **Registry (`tla-registry`)** is the only caller permitted to drive marketplace
  authority on a wallet (`hos-extension.force_transfer` / `sweep_ft`). It is
  admin-governed.
- **Active signer (`active-signer`)** holds the per-wallet owner key. Owner swap is
  gated to the marketplace authority (sales) and the recovery authority
  (recovery), with a compare-and-swap on the current owner and a freeze flag that
  serializes the two.
- **Recovery (`mpc-recovery`)** is opt-in per account. A policy binds an
  attestation key, a watcher set, a quorum threshold, and a timelock at install.
- **Wallet (`hos-wallet`)** has no FullAccess key; all authority flows through the
  installed extensions.

## Recovery: common guarantees

- **Attestation binding.** A recovery request is verified against the policy's
  install-bound attestation key, never a caller-supplied key.
- **Replay and ordering.** Each policy carries a monotonic round; a request must
  match the current round, and the round increments on acceptance, burning
  replays.
- **Timelock.** A verdict cannot settle until `timelock_secs` after the request.
- **Watcher quorum.** Settlement requires `threshold` distinct, valid watcher
  signatures over a domain-separated verdict message; duplicate and non-watcher
  keys are ignored.
- **Owner-gated abort.** The policy owner can abort an in-flight recovery.
- **No-brick callbacks.** Cross-contract callbacks never panic on the failure
  branch; an in-flight recovery is always resolvable via retryable finalize or
  abort.

## Recovery modes

### Wallet mode (default)

Recovery ends in `active-signer.swap_owner(wallet, new_owner, expected_current)`:

- The swap is guarded by a compare-and-swap on the bound owner; a stale policy
  cannot rotate a wallet whose ownership changed (for example via a sale).
- The wallet is frozen at approval and unfrozen at finalize or abort, serializing
  recovery against marketplace sales.

Trust assumptions: the watcher set and the attestation-key holder.

### Native mode

Recovery ends in an `AddKey` on a raw NEAR account, signed by NEAR Chain
Signatures (`v1.signer`) over a manually constructed transaction:

- The unsigned `AddKey` transaction is built byte-for-byte to the NEAR transaction
  format and hashed; the hash is signed by the MPC signer (`payload_v2: { Eddsa }`,
  `domain_id` for ed25519). The byte layout is locked by a golden-vector test
  against `near-api-js`.
- `AddKey` grants the recovered key FullAccess to the target account.

Additional trust assumptions and risks specific to Native mode:

- **MPC signer dependency.** Recovery liveness depends on `v1.signer` availability
  and correctness. If the MPC network is paused, Native recovery is paused.
- **FullAccess grant.** Unlike wallet mode, which rotates an extension-mediated
  key, Native mode adds a protocol-level FullAccess key. The new key holder has
  unconditional control of the account; there is no extension gate to evict a
  prior key.
- **Transaction construction.** The contract builds the transaction bytes itself;
  any divergence from the NEAR format would produce an invalid or wrong-permission
  signature. This is constrained by the golden-vector test.
- **Derivation path.** The signed key is derived under a per-account path; the path
  must be unique per account to prevent cross-account signature reuse.

Native mode is appropriate only where a protocol-level FullAccess recovery key and
the `v1.signer` trust assumption are both acceptable. Wallet mode is the default
for House of Stake managed sub-accounts.

## Marketplace

- **Sale serialization.** Listing and offer settlement set a per-listing `settling`
  lock; concurrent settlement attempts are rejected.
- **Asset gate.** A sale or reclaim that would move a sub-account is blocked unless
  every allow-listed fungible-token balance is provably zero. The gate is
  fail-closed: a failed or unparseable balance query blocks.
- **Refunds.** All refunds are pull-based (`claim_refund`); the contract never
  pushes funds in a way that can wedge on a failed transfer.
- **Business namespaces.** Sub-accounts under a Business (licensee-gated) TLA are
  not resellable; the licensee boundary is preserved across the marketplace.

## Out of scope

- The vendored `hos-wallet` fork (Defuse `wallet-no-sign`) is audited upstream and
  pinned by commit; only the added `init` is in scope here.
- Off-chain components (relay, attestation issuance) are outside this on-chain
  threat model.
