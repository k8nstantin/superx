# SuperX Master Blueprint v41.1: The Atomic Reset (Hardened)

**Classification:** Definitive Immutable Architecture (Revision 41.1)  
**Paradigm:** Documentation-Driven / Tight Substrate / Functional Rigor  
**Distribution:** Portable Swarm Directory / Transparent Unification  
**Substrate:** 5-Table Minimalist Kernel (SurrealDB + RocksDB)

---

## 1. The Constitution: Corrective Mandates
1. **Physical Alignment:** The documentation is a character-perfect mirror of the code. 
2. **5-Table Substrate:** Metamodel, Entity, Relation, State Ledger, Telemetry Stream.
3. **NASA Safety:** Hard-fail on cycle detection limits. Iterative flows only. 
4. **Validation First:** REAL JSON Schema validation is enforced for all state writes.
5. **Least Privilege:** Dual handles (`root_db` / `exec_db`) with proper multi-tenancy (`tenant_id`).
6. **Durable Workflows:** Execution state is merged into `telemetry_stream` with a resume cursor.

---

## 2. The 5-Table Atomic Substrate

| Table | Purpose | Corrective Logic |
| :--- | :--- | :--- |
| **`type_definition`** | The Metamodel | Added `is_acyclic` flag and `validation_schema`. |
| **`entity`** | Identity Anchors | UUIDv7 + UNIQUE index on `uid`. |
| **`relation`** | Unified Graph | `in`, `out`, `type`. Cycle check runs only if `type.is_acyclic` is true. |
| **`state_ledger`** | SCD-2 Content | Discriminated columns. Composite index on `(target, type, is_current)`. |
| **`telemetry_stream`** | Event Firehose | Execution cursor + Hardware/Agent trace. |

---

## 3. Directory Layout (Mandatory)
```text
/SuperX
├── superx (The Rust Manager)
├── /ui (Static UI assets)
├── /db (RocksDB files)
├── /models (GGUF brains)
└── /logs (Telemetry exports)
```

---

## 4. Engineering Fixes (v41.1)
*   **Edge Direction:** `compile_context` creates: `root -> edge_compiled_from -> artifact`.
*   **Validation:** `attr_desc` seeded with a required "text" schema.
*   **Tenancy:** `tenant_id` propagated to `relation` and `state_ledger`.
*   **Performance:** MTREE index on `value_vector` and BM25 on `value_text`.
*   **Cycles:** `create_structural_edge` hard-fails if depth > 1000.

---
**End of Document**  
*SuperX v41.1: Reset Complete. Moving to Hardened Construction.*
