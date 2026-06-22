# Threat Model

This is the security model for the House of Stake TLA contracts. Most of the
attention goes to the opt-in recovery system, because that is where the trust
assumptions are least obvious and the blast radius is largest.

## Trust boundaries

The contracts split authority along these lines:

The registry (`tla-registry`) is the only account allowed to drive marketplace
authority on a wallet, meaning `hos-extension.force_transfer` and `sweep_ft`. It is
governed by the admin multisig described below.

The active signer (`active-signer`) holds the per-wallet owner key. An owner swap
can only come from the marketplace authority (a sale) or the recovery authority (a
recovery), and both run through a compare-and-swap on the current owner plus a
freeze flag that keeps the two from racing.

Recovery (`mpc-recovery`) is opt-in, one policy per account. The policy binds an
attestation key, a watcher set, a quorum threshold, and a timelock, all fixed at
install.

The wallet (`hos-wallet`) holds no FullAccess key. Every action flows through the
extensions installed at mint.

The owner cannot reconfigure that authority. The only owner-reachable path into the
wallet is `active-signer.submit_signed_request`, and it rejects any request that
carries wallet ops, so an owner-signed request can move funds and call contracts but
cannot add or remove an extension or re-enable the native signature path. This is
what keeps a sub-account sale clean: the seller cannot plant a back-door extension to
keep authority after the buyer pays, and cannot strip the signer to brick the buyer.
As a second line, a sale reads the wallet's extension set on chain at settlement and
refuses to transfer unless it is exactly the canonical pair (`active-signer` plus
`hos-extension`), so any wallet whose authority set has drifted from the mint shape
cannot be sold.

## Deployment and governance invariants

Two properties the contracts cannot enforce on their own. They have to hold in
deployment, and the auditor should confirm them on-chain.

**TLA accounts must be locked.** A TLA account runs `tla-manager` and is registered
as a minter on `active-signer`, so it can mint signers for sub-accounts under its
namespace. `install_signer` is install-once: it refuses to overwrite an existing
entry, so the only way to rotate a key is the authority-gated `swap_owner`. That
stops a minter from re-keying a live sub-account. It does not stop a TLA account
that still holds a FullAccess key from squatting or griefing signer slots on names
that have not been rented yet, so the account must have no FullAccess key. Only its
registry-gated contract methods should be callable.

**Admin is the multisig.** The admin role on `active-signer`, `hos-extension`, and
`tla-registry` is, in practice, unrestricted over wallets and funds: it can repoint
the registry, manage minters, change fees, and queue withdrawals. It has to be a
multisig with a timelock (the Security Council), not a single key. Contract upgrades
sit behind a separate, longer timelock.

## Recovery: shared guarantees

Both recovery modes pass the same gate before anything settles on-chain:

- A request is checked against the attestation key bound at install, never a key the
  caller supplies.
- Each policy carries a monotonic round. A request has to match the current round,
  and the round advances on acceptance, so a captured attestation cannot be
  replayed.
- A verdict cannot settle until the timelock has elapsed.
- Settlement needs `threshold` distinct, valid watcher signatures over a
  domain-separated message; duplicate signatures and non-watcher keys are dropped
  before counting.
- The policy owner can abort an in-flight recovery.
- A policy can only be reinstalled while the account is idle, and the reinstall
  preserves the monotonic round, so it cannot abandon a frozen in-flight recovery or
  reopen a spent round for replay.

Two more properties hold under failure. Callbacks never panic on their failure
branch, so a half-completed cross-contract call cannot brick a recovery; it stays
resolvable through a retryable finalize or an abort. And the policy reset that a
marketplace transfer fires is best-effort: if it fails, the stale policy is
harmless, because the bound-owner compare-and-swap voids any later finalize against
a wallet whose owner has since changed.

## Recovery modes

A policy is one mode or the other, chosen at install.

**Wallet mode** is the default for managed sub-accounts. Recovery finishes with
`active-signer.swap_owner`, guarded by the compare-and-swap on the bound owner, so a
stale policy cannot rotate a wallet whose ownership already moved. The wallet is
frozen at approval and unfrozen at finalize or abort, which is what serializes
recovery against a sale. The only parties trusted here are the watcher set and
whoever holds the attestation key.

**Native mode** finishes with an `AddKey` on a raw NEAR account, signed by NEAR
Chain Signatures (`v1.signer`). The contract builds the unsigned `AddKey`
transaction byte-for-byte to the NEAR format, hashes it, and has the MPC signer sign
the hash (`payload_v2: { Eddsa }`, `domain_id` for ed25519); the byte layout is
pinned by a golden-vector test against `near-api-js`. The result is that the
recovered key gets protocol-level FullAccess, which carries a few risks the wallet
path does not:

- Liveness depends on `v1.signer`. If the MPC network is paused, so is native
  recovery.
- The new key is FullAccess. There is no extension gate to evict a prior key the way
  wallet mode rotates an extension-mediated one; whoever holds it controls the
  account outright.
- The contract assembles the transaction itself, so a deviation from the NEAR format
  would yield an invalid or wrong-permission signature. The golden-vector test is
  what holds that line.
- The signing key is derived under a per-account path, which has to be unique per
  account or signatures could be reused across accounts.

Native finalize is owner-gated and does not treat a signature as a completed
recovery. `finalize_recovery` on a native policy can only be called by the policy
owner, who supplies the account nonce and a recent block hash; an MPC signature over
the wrong parameters is useless, so leaving the call open to anyone would only let a
stranger burn an approved round with stale inputs. When the signer returns, the round
does not close: it returns to Approved and the signed transaction is handed back for
broadcast. Only `claim_native_finalized`, again owner-gated, retires the round, and
the operator calls it after confirming the `AddKey` actually landed on chain. So a
produced signature never stands in for an accepted transaction, and a failed
broadcast leaves the recovery retryable rather than falsely finalized.

Use native mode only where a FullAccess recovery key and a dependency on `v1.signer`
are both acceptable. Everything House of Stake manages defaults to wallet mode.

## Marketplace

Sale settlement takes a per-listing `settling` lock, so two settlements cannot run
against the same listing at once.

A sale is anchored to the owner key the seller listed against. `list_sub_account` and
`accept_offer` record the seller's current owner key, and settlement passes it to
`swap_owner` as a compare-and-swap. If the wallet's signing key has changed since the
listing (for example a recovery rotated it), the swap voids, the sale does not
complete, and the buyer is refunded. The settlement callback treats a voided swap as a
failed sale, not a silent success.

The asset gate blocks any sale, reclaim, or re-rent that would move a sub-account
unless every allow-listed fungible-token balance reads as provably zero. It is
fail-closed: a balance query that fails or does not parse blocks the move rather than
waving it through. Re-renting a parked name runs the same gate, so a new renter cannot
inherit fungible tokens that landed on the wallet while it was parked. The allowlist is
capped at 16 so the per-token fan-out stays inside the gas limit; raising the cap means
re-checking the gas budget first.

Refunds are pull-based through `claim_refund`. The contract never pushes funds in a
way that could wedge on a failed transfer.

Sub-accounts under a Business (licensee-gated) TLA are not resellable, so the
licensee boundary the rent path enforces is not lost on the marketplace.

## Out of scope

NFTs are not gated. The asset gate is fungible-token only, so an NFT held by a
wallet transfers with the sub-account on a sale or reclaim; a seller has to move
NFTs out before listing. NFT-aware gating is a V2 item.

The vendored `hos-wallet` fork is audited upstream and pinned by commit. Only the
added `init` is in scope here. The wallet still lets an enabled extension flip its
signature mode through `SetSignatureMode`, but this is not reachable as an escalation:
the owner signing path rejects all wallet ops (so an owner cannot send it), neither
core extension ever emits it, and the contract is built no-sign, so the signature
verifier is dead code that cannot validate anything even if the flag were flipped. A
wallet-side guard against `SetSignatureMode` would only matter to a compromised core
extension, which already controls the wallet outright, so the fork is intentionally
left at its audited commit rather than patched for this.

Off-chain pieces (the relay, attestation issuance) sit outside this on-chain model.
