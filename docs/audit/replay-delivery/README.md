# Synthetic Agent Replay & Delivery Audit — interrupted delivery

**Classification:** synthetic demonstration, not a customer incident and not raw Telegram evidence. This fixture demonstrates the audit format. It does not claim compatibility with Herdr or any other external runtime.

## Incident

A two-chunk answer belongs to Run `22`. Delivery `6` sends chunk `0` as message `392`; chunk `1` is still `pending` when the process is killed. On restart, the Core must mark that chunk `unknown` without calling the model or sending again. Only the owner's explicit `/result 22` creates Delivery `7`, whose chunks arrive as messages `397` and `398`. Delivery `6` remains unresolved.

## State and authority map

| Component | Owns | Must not do |
|---|---|---|
| Core Run / Session | Run `22`, completed state, stored redacted answer | Treat Run completion as proof of channel delivery |
| Core Delivery | Delivery `6` / `7`, chunk state, attempts, revisions | Retry an ambiguous original send automatically |
| Telegram channel | Bot API outcome and positive message ids | Decide whether interrupted work should resume |
| Owner | Explicit `/result 22` and any decision to reconcile the original `unknown` | Mistake the new Delivery for proof of the old Delivery's arrival |
| Model | Original answer generation only | Trigger recovery, replay, or delivery reconciliation |

## Run the deterministic audit

Requires Node.js 18+ and no packages. From the repository root:

```bash
node docs/audit/replay-delivery/check.mjs
```

Expected summary: `state: PASS`, `passed: 8`, `failed: 0`, zero automatic model calls or original-delivery retries after restart, and an unresolved original `unknown`.

The checker reads only `fixture.json`. It performs no network, model, Telegram, filesystem-write, or customer-data operation.

## Run the matching Iskra regression proofs

These tests exercise the actual Rust implementation with local mocks and dummy credentials:

```bash
cargo test restart_maps_pending_to_unknown_without_sending_or_rerunning
cargo test --test session_cli cli_restart_does_not_reprint_or_resend_completed_work
cargo test --test session_cli cli_result_prints_a_stored_answer_without_a_model_call
```

The fixture checker is an audit-format demonstration; the Rust tests are the product-behavior proof. Do not describe the fixture alone as proof of live Telegram behavior.

## Audit verdict template

- **Observed:** original Delivery `6` has one `sent` chunk and one unresolved `unknown` chunk after restart.
- **Prevented:** no automatic model replay and no second attempt against original Delivery `6`.
- **Explicit recovery:** `/result 22` creates separate Delivery `7` and both chunks receive unique message ids.
- **Unresolved:** the original `unknown` is not marked as delivered merely because `/result` succeeded.
- **Recommendation:** preserve separate Run and Delivery identities; require an explicit owner action or channel-proven reconciliation before any repeated external effect.
- **Manual work:** incident selection, scope review, public-claim review, and final customer verdict.
- **Automated work:** event-sequence validation and deterministic assertions in `check.mjs`.

## Public-claim boundary

This sample supports only the narrow statement: “the supplied synthetic incident and the named Iskra regression tests can be rerun locally.” It does not prove that every agent runtime is safe, that a customer incident has been fixed, or that Herdr has a confirmed duplicate-execution defect.
