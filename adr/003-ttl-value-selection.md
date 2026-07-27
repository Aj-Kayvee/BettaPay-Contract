# ADR 003: TTL (Time-to-Live) Value Selection

**Date:** 2026-07-26

**Status:** Accepted

## Context

Soroban's persistent storage entries require explicit TTL (time-to-live) management. Each stored entry has a live-until ledger sequence, and once that sequence is reached, the entry becomes eligible for archival/garbage collection. We needed to choose appropriate TTL thresholds and bump values for different kinds of storage entries in the settlement contract.

## Decision

We use two different TTL configurations based on the **semantic criticality** of the stored data:

### Payment Records: 14 / 30 days
```rust
const PAYMENT_TTL_THRESHOLD: u32 = 17280 * 14;   // ~14 days
const PAYMENT_TTL_BUMP: u32 = 17280 * 30;          // ~30 days
```

### Rules and Merchant Markers: 14 / 30 days
```rust
const RULE_TTL_THRESHOLD: u32 = 17280 * 14;        // ~14 days
const RULE_TTL_BUMP: u32 = 17280 * 30;              // ~30 days
const MERCHANT_TTL_THRESHOLD: u32 = 17280 * 14;     // ~14 days
const MERCHANT_TTL_BUMP: u32 = 17280 * 30;           // ~30 days
```

### Admin & Governance: 50k / 100k ledgers
```rust
// Used in read_admin, read_governance, read_recovery_address
env.storage().instance().extend_ttl(50_000, 100_000);
```

The `17280` constant represents the approximate number of ledgers closed per day on Stellar (1 ledger every ~5 seconds).

## Consequences

- ✅ Payment records remain available for 30 days after the last read (or write), giving ample time for settlement processing.
- ✅ Admin and governance entries use a much higher TTL (50k/100k ledgers, approximately 3–6 days) since they are instance-storage entries that should never expire under normal operation.
- ✅ Read operations extend the TTL of the accessed entry (when below threshold), so actively queried entries stay alive.
- ❌ Payment records and rules that are written once and never re-read will eventually expire after 30 days.
- ⚠️ The TTL approach adds complexity to every read/write path; each public query function must include a TTL extension call.
