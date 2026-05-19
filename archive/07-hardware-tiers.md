# SuperX — Hardware Tiers & Degradation Strategy

SuperX adapts its System Agent roster and LLM requirements based on available hardware, gracefully degrading capabilities from T0 down to T5.

| Tier | Hardware Profile | Roster Deployment | Primary LLM Strategy | Degradation Impact |
| :--- | :--- | :--- | :--- | :--- |
| **T0 (Cloud)** | Distributed A100/H100 Cluster | Full Roster (Async parallelized) | GPT-4o / Claude 3.5 Sonnet / Llama 3 70B | Maximum reasoning depth. All system agents active continuously. |
| **T1 (Server)** | Single Server / Mac Studio (M3 Ultra) | Full Roster (Local inference) | Qwen 2.5 32B / Llama 3 70B Quantized | Near-cloud reasoning; inference handled entirely locally via `candle`. |
| **T2 (Pro)** | High-End Laptop (M3 Max / RTX 4090) | Full Roster (Batched) | Gemma 2 27B / Llama 3 8B | Meta-Harness Proposer runs asynchronously off-path to save VRAM. |
| **T3 (Standard)** | Mid-Tier Laptop (M2 / 16GB RAM) | Core Agents Only | Phi-3 / Qwen 2.5 7B | Proposer disabled. Evaluator runs scheduled batches only. Relies heavily on Cloud endpoints for complex tasks. |
| **T4 (Edge)** | IoT / Constrained (8GB RAM) | Classifier & Extractor Swarm Only | N/A (Rules-based fallback) | No local LLMs. Uses strict WASI capabilities and delegates all reasoning to T0/T1 over network. |
| **T5 (Offline)** | Airgapped / No GPU | Wasm Extractor Swarm Only | None | Functions solely as a high-speed code indexer and CRDT state manager. No autonomous agents. |