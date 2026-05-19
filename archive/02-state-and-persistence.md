# SuperX — State Management & Persistence (SCD-2 + CRDTs)

SuperX solves the tension between the need for microsecond execution speeds and the absolute requirement for immutable, time-traveled data retention. It achieves this via an Epoch-Synced dual-state architecture.

## 1. The Hot Path: CRDTs and Concurrent Maps

Agents cannot wait for SQL queries. When an agent runs, it interacts with data entirely in RAM.

### The Structures
*   **ECS / Multi-Map:** Holds the topology of the system. We use concurrent maps rather than a locked monolithic graph to ensure highly concurrent reads and writes across the Wasm Extractor Swarm.
*   **`loro` (CRDTs):** The state payload of every node (e.g., the text of a document, the JSON configuration of an agent) is backed by a Conflict-free Replicated Data Type.

### Why CRDTs?
In an Agentic OS, multiple entities act concurrently. A developer might be editing `auth.rs` via an MCP-backed IDE extension, while an agent is simultaneously modifying the AST representation. CRDTs ensure that these concurrent edits are merged deterministically.

## 2. The Cold Path: SurrealDB, SCD-2, and Tenancy

Data volume is not a concern; the imperative is absolute traceability.

### The Universal Invariant
No record in SurrealDB is ever updated in place, and no record is ever deleted. 

```surql
DEFINE FIELD tenant_id     ON entity TYPE string ASSERT $value != NONE;
DEFINE FIELD valid_from    ON entity TYPE datetime DEFAULT time::now();
DEFINE FIELD valid_to      ON entity TYPE option<datetime>;
DEFINE FIELD created_by    ON entity TYPE string;
DEFINE FIELD change_reason ON entity TYPE option<string>;
```
*Note: `tenant_id` is mandatory at row 0 to ensure strict enterprise multi-tenancy.*

### Secrets Management
Secrets are never stored in the standard entity table. They exist in a dedicated `secrets` table, encrypted at rest, and integrated with an external KMS (e.g., AWS KMS, HashiCorp Vault).

### The Epoch-Based Sync
To prevent database bloat and catastrophic Write Amplification, we do **not** sync every CRDT operation (e.g., individual keystrokes) to the database synchronously. 

1.  **OpLog Accumulation:** Loro manages the high-frequency operational log in RAM.
2.  **Epoch Flush:** On a semantic boundary (e.g., file save, agent completes a turn, or a 5-second debounce), an Epoch is triggered.
3.  **The `supersede()` Transaction:** The kernel opens a SurrealDB transaction. It `UPDATE`s the prior row setting `valid_to = time::now()`, `INSERT`s the new aggregated state as a new row, and commits.
4.  **Pruning:** Once safely stored in SCD-2, the hot-path Loro CRDT history can be shallowly pruned to preserve RAM.

## 3. Crash Recovery: The Rehydration Loop

Because the execution graph lives in volatile RAM, SuperX must perfectly recover state upon crash or restart.

1.  **Boot Phase:** The Rust kernel process starts. RAM is empty.
2.  **The Rehydration Query:** The kernel queries SurrealDB for the "active" slice of reality:
    ```sql
    SELECT * FROM entity WHERE valid_to IS NONE AND tenant_id = $tenant;
    ```
3.  **Invariant:** If the system crashes mid-Epoch, un-flushed CRDT operations in RAM are lost. The system strictly recovers from the last known valid SCD-2 state in SurrealDB. This guarantees no data corruption.
4.  **Resume:** The Wasm host is initialized and agents resume.

## 4. Time Travel (`fn::current_at`)

Because of SCD-2, the database contains the entire history of the DAG.

```surql
DEFINE FUNCTION fn::current_at($table: string, $t: datetime, $tenant: string) {
    RETURN type::query(string::concat(
        'SELECT * FROM ', $table,
        ' WHERE tenant_id = $tenant AND valid_from <= $t AND (valid_to IS NONE OR valid_to > $t)'
    ), { t: $t, tenant: $tenant });
};
```

This enables perfect counterfactual diagnosis for the Meta-Harness.