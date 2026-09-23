# Embedded LLM runtime (proposal)

Status: proposal, nothing built. Related: [`../src/nlp/CLAUDE.md`](../src/nlp/CLAUDE.md) (the parse
pipeline this plugs into), [`DEVELOPMENT.md`](./DEVELOPMENT.md) (latency baseline, KI-19/20/21),
[`roadmap.md`](./roadmap.md).

## Why

Ollama is step 3 of `NLPParser::parse` (cache, regex, Ollama, regex fallback, bare title). It runs
only when a deadline phrase stays unresolved (2 of 25 probe inputs), yet it costs a multi-GB install,
a server process, a 17-26s first model load, and a 0.3-1.5s warm parse. The README promises
sub-100ms; the regex path meets it, the LLM path cannot.

Goal: replace the HTTP call with an in-process runtime specialised to this one task, so an LLM parse
feels like a regex parse. Non-goals: chat, other model families, GPU, training, beating llama.cpp
on general throughput.

## Where the win comes from

Estimates from the model shape and the 2026-09-19 baseline (M3 Max). None is measured for a custom
runtime; Phase 0 replaces them with data.

| Lever | Mechanism | Expected effect |
|---|---|---|
| Cold start | Ollama spawns a runner and initialises the GPU (17s first load, 0.5-1s reload). An in-process `mmap` of a ~0.5 GB file costs page faults only. | The big win: seconds to well under 1s. |
| Prefix KV cache | `build_prompt` is ~670 tokens, ~600 identical per call. Prefill the fixed part once, keep its KV state (~8 MB in f16 for a 0.5B), prefill only the ~15 input tokens per parse. | ~40x fewer prefill tokens. Required: prefilling all 670 on CPU would likely cost hundreds of ms to seconds. |
| Forced scaffold | The output schema is fixed. Text like `{"type": "task", "title": "` is known, so run it as one batched forward pass instead of one decode step per token; sample only free fields. | Roughly halves decode steps. Ollama's `format: json` checks validity but still runs every step. |
| No HTTP/JSON hop | Direct call. | A few ms. |

Honest ceiling: llama.cpp on Metal already parses in 0.29-0.42s warm with the 0.5B. A CPU-only
runtime with both caches should land near 0.1-0.2s warm, a 2-3x win, not 10x. The cold-start win is
what changes the experience. Ollama may already reuse the prompt prefix within a day (the date lines
change only at midnight); Phase 0 checks.

## Design

**Seam.** `NLPParser::parse` step 3 calls `OllamaClient` directly. Add an `LlmBackend` trait
(`parse(input) -> Result<ParsedItem>`, `warm()`), implement it for `OllamaClient` (no behaviour
change) and the new runtime, pick by config. `build_prompt` and `parse_response` are reused. Ollama
stays as a fallback backend.

**Separate crate.** `triptych` sets `unsafe_code = "forbid"` (cannot be overridden inside the crate)
and denies `unwrap`/`expect`/`panic`; NEON kernels need `unsafe`. So the runtime is a workspace member
`crates/tinyllm`: sync, no triptych dependencies, called through `spawn_blocking` so the TUI loop
never waits (KI-21).

`tinyllm` modules: `gguf` (header, metadata, tensor table, mmap), `tokenizer` (byte-level BPE built
from GGUF metadata), `quant` (Q8_0 first, then Q4_K/Q6_K), `kernels` (scalar reference, then NEON),
`model` (RMSNorm, RoPE, grouped-query attention, SwiGLU; Qwen2 only), `kv` (preallocated cache, prefix
region immutable, length reset per parse), `decode` (greedy sampling plus the JSON-template state
machine).

**Processes.** The daemon (`src/cli/daemon.rs`) and the TUI each load the model; both `mmap` the same
file, so the page cache is shared. The TUI loads on a background thread and uses regex until ready
(the rule `warm_in_background` follows today). The prefix KV is built in `prewarm` and rebuilt when
the date in the prompt changes.

**Known traps.** Tokenizing scaffold text apart from generated text can differ from canonical
tokenization, so end scaffold segments on stable boundaries (a closing quote). A wrong RoPE variant or
KV-head mapping gives fluent but wrong output; only a layer-by-layer logit diff against a reference
catches it.

## Phases (each ends at a decision)

0. **Baseline, no runtime.** Read `prompt_eval_duration`, `eval_count` and `load_duration` from Ollama
   responses over repeated same-prefix calls. Build a labelled corpus of unresolved-deadline phrases
   and score 0.5B, 1.5B, 3B and 7B. Gate: if no small model is accurate enough, stop and extend rules.
1. **Seam.** Trait plus config, Ollama as the only impl. No behaviour change, tests green.
2. **`tinyllm` correctness.** GGUF, tokenizer, f16 forward pass; logits match llama.cpp on fixed
   prompts. Gate: matches.
3. **Speed.** Quantized kernels and NEON, then prefix cache, then forced scaffold. Benchmark each
   lever alone against Phase 0.
4. **Integrate.** Background load, fallback to Ollama then regex, new TUI-driver scenarios (cold start
   never blocks; a parse works with Ollama stopped). Update `src/nlp/CLAUDE.md`.

Rough effort: 5-8 weeks part-time, 3-5k lines.

## Success criteria (targets, not results)

- Warm parse p50 under 150ms on the 0.5B/1.5B (0.29-0.42s today via Ollama).
- Process start to first LLM parse under 1s with the file in page cache.
- Accuracy on the Phase 0 corpus at least equal to the model it replaces, same prompt.
- Ollama not needed for the LLM path. Weights are still a download (0.4 GB and up).

## Cheaper alternatives, do these first

- Extend `src/nlp/rules.rs` to cover the deadline phrases the LLM catches. The LLM hit rate drops
  toward 0, which may make this proposal moot.
- Embed `llama-cpp-2` or `candle` in-process: same seam, same cold-start win, days not weeks, no
  kernels. It is also the fair baseline the custom runtime must be measured against.
