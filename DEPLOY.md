# Deploy and Governance Runbook

How to deploy the House of Stake TLA contracts and hand control to the Security
Council multisig. This describes mainnet; testnet is the same with testnet account
names and `v1.signer-prod.testnet` in place of `v1.signer`.

One thing to understand before you start: most of the wiring between contracts is
fixed at init and has no setter. `active-signer` bakes in its marketplace and
recovery authorities, `mpc-recovery` bakes in its owner, signer, and transfer
authority, and `tla-manager` bakes in all of its references. Because every account
name is chosen up front this is fine, but it means a wrong address cannot be patched
later without redeploying. Get the names right first.

## 1. Account layout

Choose the singleton account names before anything else; they reference each other
at init, so all of them need to be known.

| Account | Contract | Role |
|---|---|---|
| `<registry>` | tla-registry | marketplace orchestrator |
| `<active-signer>` | active-signer | per-wallet signing authority |
| `<hos-extension>` | hos-extension | marketplace authority |
| `<mpc-recovery>` | mpc-recovery | recovery (opt-in per wallet) |
| `<council>` | SputnikDAO | the admin multisig |

Each TLA is its own account (for example `acme.near`) running `tla-manager`.
Sub-accounts are minted underneath it (`alice.acme.near`).

## 2. Prerequisites

Build every contract reproducibly and record the wasm hash. The reproducible build
runs in a pinned container; the command and image for each crate are in its
`[package.metadata.near.reproducible_build]` (Rust 1.86, near-sdk 5.26.1):

    cargo near build   # reproducible, run from each crate directory

Record the `sha256` of each output wasm. Those hashes are what the auditor and
anyone verifying the deploy will compare against the on-chain code.

Deploy `hos-wallet` as a global contract and record its code hash. `tla-manager`
takes this hash at init and uses `use_global_contract` to put the wallet on each new
sub-account, so the hash has to exist before `tla-manager` is deployed.

Deploy the `<council>` SputnikDAO with its four signers and the chosen threshold.
Deploy it first, so the contracts below can be initialized with `<council>` as their
admin and there is never a single-key admin on-chain.

## 3. Deploy and initialize the singletons

Initialize each with `<council>` as the admin or owner directly. This is the secure
default; it removes the bootstrap-key window entirely. If `<council>` is not ready
in time, use the bootstrap handoff in section 5 instead.

active-signer:

    new(admin: <council>,
        marketplace_authority: <hos-extension>,
        recovery_authority: <mpc-recovery>,
        timeout_secs: <secs>)

mpc-recovery (the owner is immutable, there is no setter, so set it correctly here):

    new(owner: <council-or-recovery-ops>,
        signer: v1.signer,
        transfer_authority: <hos-extension>,
        watchers: [<watcher-pubkeys>],
        threshold: <n>)

hos-extension:

    new(admin: <council>,
        registry: <registry>,
        active_signer: <active-signer>,
        recovery: <mpc-recovery>)

tla-registry:

    new(admin: <council>,
        hos_extension: <hos-extension>,
        parked_signer_pubkey: <ed25519-pubkey>,
        grace_period_ns: <ns>)

A note on `mpc-recovery`'s owner: it gates `install_policy` and `abort_recovery`, and
an abort can be time-sensitive (cancelling a recovery aimed at a user). A full
council vote may be too slow for that, so consider a separate, faster recovery-ops
multisig here rather than the main council. Whatever you pick is permanent for this
deployment.

## 4. Per-TLA setup

For each TLA account (for example `acme.near`):

1. Deploy `tla-manager` on the TLA account and init it:

       new(registry: <registry>,
           active_signer: <active-signer>,
           hos_extension: <hos-extension>,
           wallet_code_hash: <hos-wallet-hash>,
           min_balance: <yocto>)

2. Lock the TLA account. Remove every FullAccess key so only the `tla-manager`
   methods are callable. This is a hard requirement: a TLA account that keeps a
   FullAccess key can squat or grief signer slots under its namespace. See
   THREAT_MODEL.md.

3. Register the TLA and add it as a minter, both through `<council>`:
   - `active-signer.add_minter(<tla-account>)`
   - `tla-registry.register_tla(...)`

## 5. Bootstrap handoff (only if you did not init with `<council>`)

If a bootstrap key was used as the initial admin for setup convenience, hand off and
remove it on each admin-bearing contract: `active-signer`, `hos-extension`, and
`tla-registry`.

    add_admin(<council>)        # from the bootstrap key
    remove_admin(<bootstrap>)   # leaving <council> as the only admin

Do this on all three. A bootstrap key left in any admin set is a single-key backdoor
sitting next to the multisig. The remove-last-admin guard stops you from locking a
contract out, but it will not stop you from leaving a second admin in by accident, so
read back each contract's admin set when you are done. `mpc-recovery` has no admin
handoff; its owner is whatever was set at init.

## 6. Verify (launch gate)

The contracts cannot enforce the deployment invariants on themselves: a TLA account
that keeps a FullAccess key, or an admin set with a stray key in it, is invisible to
the on-chain logic but fatal to the security model. So this is a gate, not a
checklist. Read every value back from chain after the handoff, and treat any miss as
a launch blocker, not a follow-up.

Admin and minter sets, read from chain (not from your deploy notes):

    near view <active-signer> admins         # expect exactly [<council>]
    near view <active-signer> minters        # expect exactly the TLA accounts you added
    near view <hos-extension> get_admins     # expect exactly [<council>]
    near view <tla-registry>  get_admins     # expect exactly [<council>]

A bootstrap or deployer key left in any admin set is a single-key backdoor next to the
multisig. If any set has more than `<council>` in it, stop and remove the extra before
launch.

Every TLA account is locked. For each TLA account, list its keys and confirm none has
FullAccess permission:

    near account list-keys <tla-account>

The expected result is no FullAccess key at all; only the `tla-manager` contract
methods should be reachable. A single FullAccess key on a TLA account lets it squat or
grief signer slots under its namespace, so a hit here blocks launch until the key is
removed.

`mpc-recovery` owner, watcher set, and threshold match what you intended:

    near view <mpc-recovery> # confirm owner, watchers, threshold via the contract's views

Code hashes match the reproducible build:

- The on-chain code hash of each contract equals the reproducible-build hash you
  recorded in step 1. Anyone verifying the deploy compares against on-chain code.

Smoke test, last:

- Rent a sub-account, submit a signed request through `active-signer`, and confirm the
  wallet executes it. Then confirm a wallet-ops request (AddExtension) submitted the
  same way is rejected, and that a sale settles only when the wallet's extension set is
  the canonical pair.
