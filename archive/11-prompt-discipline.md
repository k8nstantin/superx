# AuraOS — Strict XML Prompt Discipline

Free-form text prompts introduce non-determinism, rendering the Meta-Harness Evaluator useless. AuraOS enforces a strict XML/tagged prompt discipline for all system and external agents.

## 1. The Tag Vocabulary
All LLM outputs must conform to a defined schema. If an agent outputs raw text outside these tags, the OS considers it a fault and truncates the turn.

*   `<thought>`: The agent's scratchpad. Not logged to the final execution trace unless in debug mode.
*   `<action tool="name">`: The structured tool invocation request.
*   `<observation>`: The system's response injected back into the context window.
*   `<error>`: Structured error reporting for the Proposer to digest.
*   `<yield>`: Explicit signal that the agent has finished its task.

## 2. Write-Time Validation
*   Before the string is parsed by the kernel, it is streamed through a **Wasm Validation Filter**.
*   This filter uses a fast regex/state-machine to verify XML structure *during* generation, cutting off hallucinating models early to save tokens and latency.

## 3. Harness Templates
The Meta-Harness stores prompts not as strings, but as AST templates:
```xml
<system_instructions tenant="T-123">
  <objective>{objective_data}</objective>
  <capabilities>
    {wasi_manifest_projection}
  </capabilities>
</system_instructions>
```