# SuperX — State Sync Protocol (CRDT to SCD-2)

The dual-state Hot (CRDT) / Cold (SCD-2) architecture requires explicit mechanisms to prevent data corruption, handle partial Epochs, and ensure op-log persistence.

## 1. Single-Node Commitment
SuperX v1 commits to a **Single-Node CRDT** architecture for the primary kernel loop. Cross-node clustered CRDT sync (multi-master) is explicitly deferred to v2 to guarantee state consistency during initial adoption.

## 2. Loro Op-Log Persistence
The SCD-2 `supersede()` transaction stores the materialized *snapshot* of the entity. However, cross-process collaboration requires the CRDT history to survive a crash.
*   Alongside the standard SCD-2 record, the raw `Loro` op-log delta is persisted to a dedicated `crdt_blob` table in SurrealDB during the Epoch flush.
*   On crash recovery, the kernel rehydrates the SCD-2 query surface *and* re-imports the Loro binary blobs to restore the exact vector clock state.

## 3. Partial-Epoch Failure Semantics
*   Epoch sync executes as a **2-Phase Commit (2PC)** against SurrealDB.
*   If a crash occurs mid-supersede, or the transaction fails, the in-memory CRDT is **rolled back** to the state of the last successfully flushed `crdt_blob`.
*   Un-flushed concurrent edits in RAM are intentionally dropped ("Cold-Wins" invariant) to prevent DB divergence.

## 4. Time-Travel Within an Epoch
Querying `fn::current_at(now)` must reflect reality, not just the last Epoch flush.
*   **Overlay Logic:** When queried for `now`, the kernel fetches the baseline from SurrealDB and dynamically applies the un-flushed in-memory Loro Op-Log delta before returning the result.

## 5. Epoch Trigger Taxonomy
Epoch triggers are strictly defined data events:
*   `Trigger.SemanticWrite`: Explicit save from IDE/MCP.
*   `Trigger.AgentYield`: Agent completes a defined tool call sequence.
*   `Trigger.DebounceMax`: 5000ms maximum time since last flush.
*   `Trigger.MemoryPressure`: Threshold limit reached in the Loro RAM buffer.

## 6. Safe Loro Pruning
Loro's history pruning capability is gated. The in-memory Op-Log is only pruned for operations that have been mathematically proven to exist in the persisted `crdt_blob` table, ensuring merge capability for stale concurrent ops is never broken.