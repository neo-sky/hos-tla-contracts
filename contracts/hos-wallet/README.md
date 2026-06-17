# hos-wallet

House of Stake wallet contract. A pinned fork of the Defuse `wallet-no-sign`
contract (`near/intents` at `980c19e8`) with a single addition: an `init` method
that seeds wallet state at deploy time, so the contract can be installed on a
named account instead of the derived deterministic account that `state_init`
would otherwise force.

Everything else is the upstream Defuse wallet, unchanged. The signing path stays
`NoSign` (it always rejects); all authority flows through the extensions the
`init` installs.
