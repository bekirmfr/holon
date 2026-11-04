# Free-tier LLM quality for instruction following, structured JSON, and chess reasoning (as of 2026-11-04)

Scope note: this is a search-snippet-driven pass with 15 tool calls. Several primary hosts were unreachable from the sandbox (see Gaps). Most provider-doc claims below come from search-result snippets, not from fetched pages, and are marked as such. Evaluation dates are given where the source shows them. Anything older than about 6 months is marked DATED.

Unreachable hosts (DNS failure, `getaddrinfo ENOTFOUND`): ai.google.dev (blocked by instruction), arxiv.org, console.groq.com, inference-docs.cerebras.ai, openrouter.ai. Not attempted for lack of budget: Mistral, Together, SambaNova, DeepSeek, Hugging Face, and Cloudflare primary pages (search snippets only).

## Which free-tier models are credible for instruction following and JSON output?

### Takeaway
I could not establish a ranked, evidence-backed list of free-tier models for this task. The one model with a concrete independent number (Gemini 3.5 Flash-Lite on Artificial Analysis) has three conflicting values in search snippets, and no source I reached ranks free-tier models on instruction following or JSON reliability. Structured-output support is documented for several providers, but I could not confirm free-tier availability for most of them.

### Cited Findings
- Gemini 3.5 Flash-Lite exists as an Artificial Analysis (AA) model page; the search snippets show three different Intelligence Index values: 22 (English release page, "median 13 for similarly priced models"), 12 (English model page, "median 21"), and 36 (Spanish model page and comparison pages, which also show Gemini 3.5 Flash (high) at 50). The snippets do not show index version or snapshot date. Sources: [AA release page](https://artificialanalysis.ai/models/releases/gemini-3-5-flash-lite), [AA model page](https://artificialanalysis.ai/models/gemini-3-5-flash-lite), [AA Spanish model page](https://artificialanalysis.ai/es/models/gemini-3-5-flash-lite), [AA comparison vs Gemini 3.5 Flash (high), DE](https://artificialanalysis.ai/de/models/comparisons/gemini-3-5-flash-lite-vs-gemini-3-5-flash). Conflict is unresolved; the live AA page must be checked.
- A third-party guide, verified "against the provider source on June 28, 2026" per its own text, lists Gemini 3.5 Flash and 3.1 Flash-Lite as free on Google AI Studio, and Xiaomi MiMo V2.5 as free on OpenCode Zen. This is a secondary claim, not verified on ai.google.dev. Source found via search: [Artificial Analysis free-model guide snippet](https://artificialanalysis.ai/data-api/docs) (snippet, the guide's page was not fetched).
- A German roundup reports that Google removed Pro models from the free tier in April 2026, leaving Flash and Flash-Lite. Secondary, unverified. Source: [Free LLM API quota comparison, aireiter.com](https://aireiter.com/blog/free-llm-api-quota-comparison) (snippet).
- Cloudflare Workers AI: every account gets 10,000 Neurons per day free, resetting at 00:00 UTC; some newer Kimi, GLM and DeepSeek builds need a paid billing method (snippet from the comparison aggregators; secondary). Sources: [Free LLM API comparison, OpenRouter blog](https://openrouter.ai/blog/tutorials/free-llm-apis-compared/) (search snippet; the OpenRouter host itself was unreachable when fetched), [benchlm free tier page](https://benchlm.ai/md/free-tier.md).
- Groq: one aggregator lists Llama 3.3 70B at about 30 RPM and about 1,000 requests/day; another says Groq moved to model-specific daily limits. Conflicting, dated uncertain. Source: [costbench](https://costbench.com/best/best-llm-api-with-free-tier/) and [free-LLM-API directory snippets].
- Cerebras: free-tier sources disagree (about 1M tokens/day vs about 1M tokens/month; a context cap around 8K tokens; a $5 credit after card entry). Not a reliable free-tier basis. Source: [free LLM API comparison snippets](https://blogs.novita.ai/free-llm-api-comparison-2026/).
- SambaNova: listed as free tier and rate-limited, no numbers in the snippets; higher limits require a payment method. Source: [toolfreebie SambaNova page](https://toolfreebie.com/sambanova-cloud-free-api/) (snippet).
- Mistral: its free/limited tier was not checked directly.

### Inferences
- The only free-tier model with a reasonably documented structured-output path in these results is Gemini Flash-Lite (via Google's schema-based response format). However, the schema parameters for the Flash-Lite model ID were not confirmed on a Google page, because ai.google.dev is unreachable.
- Given the three conflicting AA values, Gemini 3.5 Flash-Lite's relative standing cannot be ranked against other free models from this evidence alone.

### Gaps
- No independent evaluation ranks free-tier models on instruction following or JSON reliability. AA, LMArena and BFCL were searched, but none gave a free-tier-filtered ranking I could read on a page.
- DeepSeek, Together, Hugging Face and OpenRouter free variants were not researched (OpenRouter host blocked; no budget left for the others).
- Current free-tier limits differ across aggregators; none is confirmed on a provider's own page.

## What do independent evaluations say?

### Takeaway
Independent evidence is thin and mostly dated. The BFCL official leaderboard snapshot I could see is from 2025-12-16 (DATED), and third-party BFCL aggregators are unverified. LMArena figures came only from aggregator and SEO pages. No source I read reports a per-model score for these free models on instruction following.

### Cited Findings
- BFCL: the official Berkeley leaderboard describes V4 as the current version, and its cached page shows last update 2025-12-16 (DATED, about 11 months old at 2026-11-04). The Hugging Face dataset card lists a version release date of 09/22/2024 and points to the live leaderboard. The PyPI package bfcl-eval shows version 2026.3.23 while the site's reproduction instructions cite 2025.12.17, which suggests the leaderboard snapshot is stale. Sources: [Berkeley leaderboard](https://gorilla.cs.berkeley.edu/leaderboard) (search snippet), [HF dataset card](https://huggingface.co/datasets/gorilla-llm/Berkeley-Function-Calling-Leaderboard), [PyPI stats](https://pypistats.org/packages/bfcl-eval). I did not read a per-model BFCL score on an official page.
- BFCL aggregators: BenchLM shows Qwen3.7 Max at 75.0% on one snapshot, and a Sept-2026 page shows Ternary Bonsai 2 27B at 74.9% on BFCL v3. BenchLM partly relies on provider self-reports and the two pages disagree on model counts. UNVERIFIED; not used for ranking. Sources: [BenchLM BFCL v4](https://www.benchlm.ai/benchmarks/bfclV4), [BenchLM BFCL v3](https://benchlm.ai/benchmarks/bfclv3).
- LMArena (WebDev): one third-party table lists Gemini 3.5 Flash Lite at about 1447 (rank #58) and Gemini 3.1 Flash Lite Preview at about 1255 (rank #109), both marked proprietary, and says Kimi K3 is the top open model on WebDev at 1657.9. UNVERIFIED third-party table. Source: [lmarena48 table, via search snippet](https://www.swfte.com/de/lmarena). LMArena has moved to arena.ai (text board: [arena.ai/leaderboard/text](https://arena.ai/leaderboard/text), not fetched).
- LMArena (text): aggregator snapshots (August 2026) list Kimi K3 at about 1500 and DeepSeek V4 Flash at about 1441 on the text board; a July snapshot lists Mistral Large 3 at 1341. Mutually inconsistent across pages; UNVERIFIED. Sources: [swfte LMArena August 2026](https://www.swfte.com/fr/ai/lmarena-ai), [swfte July 2026](https://www.swfte.com/zh/ai/lmarena-ai).
- Artificial Analysis: the free-tier API endpoint exposes headline indices and pricing; per-benchmark scores are Pro-tier. Source: [AA data API docs](https://artificialanalysis.ai/data-api/docs) (snippet).

### Inferences
- Because the BFCL official snapshot is stale and the LMArena figures are inconsistent, these sources cannot rank free-tier models for schema-constrained output.
- BFCL is the most relevant general benchmark for function/JSON calling, but its free-tier models' scores are not confirmed here.

### Gaps
- No BFCL, IFEval or AA per-model number for any free-tier model was read on a page I could confirm.
- IFEval was not searched.
- Need: current BFCL leaderboard (gorilla.cs.berkeley.edu/leaderboard, live), AA model pages for each candidate (artificialanalysis.ai), LMArena text board (arena.ai/leaderboard/text, open filter).

## Which providers support native JSON schema or structured output on the free tier, and does it constrain output?

### Takeaway
Several providers document `json_schema` response formats, but "constrained decoding" versus "best-effort" differs by provider and model, and I could not confirm free-tier availability for most. Groq and Cloudflare documents describe modes where the output may fail to comply.

### Cited Findings
- Groq (search snippets from the structured-outputs docs and integration pages; the Groq docs host was unreachable): docs describe a strict mode that constrains token generation to valid JSON, and a best-effort mode that targets schema compliance but can fail, available on all models that support Structured Outputs. Streaming and tool use are not supported with Structured Outputs. A GitHub issue reports a 400 error "This model does not support response format `json_schema`" for a model the docs listed as supported. Sources: [Groq structured outputs docs](https://console.groq.com/docs/structured-outputs.md) (snippet), [Groq provider in AI SDK](https://elements.ai-sdk.dev/providers/ai-sdk-providers/groq), [GitHub issue Vane #980](https://github.com/ItzCrazyKns/Vane/issues/980).
- Mistral: official docs describe Custom Structured Outputs with `response_format: {type: "json_schema", ...}`. The Python `client.chat.parse` helper prepends a schema instruction to the system prompt, so it is prompt-level as well. One French blog says Mistral supports only `json_object` and has no server-side schema; it conflicts with the official docs and may be outdated. The strict flag and model list were not verified. Sources: [Mistral custom structured outputs](https://docs.mistral.ai/capabilities/structured-output/custom_structured_output/) (snippet), [Vercel AI SDK Mistral provider](https://ai-sdk.dev/v7/providers/ai-sdk-providers/mistral) (snippet).
- Cloudflare Workers AI JSON mode: OpenAI-compatible `response_format` with `json_schema`. The docs say Workers AI cannot guarantee the response follows the schema and returns "JSON Mode couldn't be met" on failure, so it is best-effort, not strict. Model lists conflict: the older page names only two Llama 3.1 models; a preview page dated August 2026 names more; the preview copies could not be checked against production. Sources: [Workers AI JSON mode](https://developers.cloudflare.com/workers-ai/json-mode/) (snippet), [preview docs](https://developers.cloudflare.com/workers-ai/features/json-mode/index.md) (snippet).
- Gemini: Google's structured-output docs describe `response_format` with `application/json` and a schema, with examples on `gemini-3.5-flash`. Flash-Lite is listed by third-party sites as supporting native JSON Schema, not confirmed on a Google page. Sources: [Gemini structured output (search snippet, ja/hi mirror)](https://googledevai-dot-devsite-v2-prod-3p.appspot.com/gemini-api/docs/interactions/structured-output?hl=ja), [cloudprice Gemini 3.5 Flash-Lite](https://cloudprice.net/models/gemini-3.5-flash-lite).
- OpenRouter, Cerebras, SambaNova, DeepSeek, Together, Hugging Face: not confirmed. OpenRouter and Cerebras docs were unreachable.

### Inferences
- Based on the snippets, Groq strict mode is the only one described as token-level constraint. Mistral and Cloudflare are described as schema-requested with possible failure. This is provider-doc wording from snippets, so it needs confirmation on each page before design decisions.
- For the target task (a JSON answer choosing a legal move), a client-side validation and retry loop is still needed for every provider here, because none of the documented guarantees covers the chosen move being legal.

### Gaps
- Free-tier availability of structured output was not checked for any provider on a free-tier page.
- Model-by-model support lists are not confirmed for Groq, Mistral, Cloudflare or Gemini Flash-Lite.

## Does chess strength correlate with general benchmarks? What could and could not be found?

### Takeaway
Chess-specific LLM evaluations exist, and they show that legal-move generation and strategic play are weak for LLMs. I could not read any per-model legal-rate or Elo table for free-tier models, and I found no published study that tests the correlation with general benchmarks.

### Cited Findings
- ChessArena (arXiv 2509.24239, September 2025, DATED): 13+ LLMs with different modes, over 800 games; reports Legal Rate (LR), Top Rate (top 3 Stockfish moves) and Move Advantage Rate (MAR); headline finding that no model beats Maia-1100. Source: [ChessArena arXiv](https://arxiv.org/pdf/2509.24239) (search snippet; arXiv host unreachable when fetched). Per-model legal-rate numbers were not read. A GitHub Pages mirror lists the leaderboard but the snippet showed only Maia-1100 at rating 2220 (RD 82), which is on the Glicko scale and not comparable to nominal Elo. Source: [ChessArena GitHub Pages](https://xiaofajiang.github.io/ChessArena_github_page/).
- SPIN-Bench (arXiv 2503.12349, March 2025, DATED): tracks illegal moves in chess, Connect Four and Tic Tac Toe; reports that illegal moves become more common as games get more complex. Source: [SPIN-Bench arXiv](https://arxiv.org/pdf/2503.12349) (search snippet).
- "Tracking World States with Language Models" (arXiv 2508.19851, ICML 2025 workshop): evaluates game-state tracking with legal-move distributions. Source: [ICML 2025 page](https://icml.cc/virtual/2025/49114) (snippet).
- "Complete Chess Games Enable LLM Become A Chess Master" (arXiv 2501.17186, January 2025, DATED): fine-tuned 3B model; relates legal-move accuracy to Elo. Source: [arXiv HTML v1](https://arxiv.org/html/2501.17186v1) (snippet).
- "LLM Chess: Benchmarking Reasoning and Instruction-Following in LLMs through Chess" (arXiv 2512.01992, December 2025): found in search results; not read. Source: [arXiv HTML](https://arxiv.org/html/2512.01992v1) (snippet, unverified contents).
- ChessBench methodology page defines Legal% as the share of moves legal on the first attempt, and reports a Lichess Classical-rating estimate with Glicko-2 and engine anchors for LLMs. Source: [chessbenchllm methodology](https://chessbenchllm.onrender.com/methodology) and [chessbenchllm ranking page](https://chessbenchllm.onrender.com) (search snippets; per-model values not read; independent status unknown).
- A blog on jso.eecs.yorku.ca cites an 8%-to-73% legal-move rate for Claude Opus. UNVERIFIED secondary claim, not used.

### Inferences
- The chess papers evaluate legal-move rate and strategic strength, not instruction following or JSON format. For this task, the failure mode that matters most (illegal moves) is documented as a known weakness, but magnitudes for free-tier models are unknown.
- No source I read tests whether general benchmarks (BFCL, IFEval, AA index) predict chess legal-move rate. Any claim of correlation would be unsupported by this research.

### Gaps
- Per-model legal-move rate and Elo for free-tier models: not read on any page.
- Correlation between general benchmark scores and chess performance: no study found.
- arXiv is unreachable, so the abstracts above come from search snippets.

## Recommendations for the next step (not verified, for the report writer)
- Treat any model choice as unproven until a local test is run: feed the actual prompt (PGN, FEN, legal move list) to 3 to 5 candidates and measure legal-move rate, schema-validity rate and latency over, say, 50 positions. Include Gemini 3.5 Flash-Lite as the baseline.
- Enforce the legal-move list in code (reject and retry with the list) regardless of provider.
- Re-check the Artificial Analysis Gemini 3.5 Flash-Lite page directly to resolve the 12/22/36 conflict.

## Sources cited (with the date each was read or dated)
- [Artificial Analysis: Gemini 3.5 Flash-Lite release page](https://artificialanalysis.ai/models/releases/gemini-3-5-flash-lite) (read via search, undated)
- [Artificial Analysis: Gemini 3.5 Flash-Lite model page](https://artificialanalysis.ai/models/gemini-3-5-flash-lite) (read via search, undated)
- [Berkeley Function-Calling Leaderboard](https://gorilla.cs.berkeley.edu/leaderboard) (snapshot 2025-12-16, DATED)
- [ChessArena arXiv 2509.24239](https://arxiv.org/pdf/2509.24239) (2025-09, DATED)
- [SPIN-Bench arXiv 2503.12349](https://arxiv.org/pdf/2503.12349) (2025-03, DATED)
- [Complete Chess Games arXiv 2501.17186](https://arxiv.org/html/2501.17186v1) (2025-01, DATED)
- [Groq structured outputs](https://console.groq.com/docs/structured-outputs.md) (undated, snippet only)
- [Mistral custom structured outputs](https://docs.mistral.ai/capabilities/structured-output/custom_structured_output/) (undated, snippet only)
- [Cloudflare Workers AI JSON mode](https://developers.cloudflare.com/workers-ai/json-mode/) (undated; preview list dated August 2026, snippet only)
- [LMArena via swfte](https://www.swfte.com/de/lmarena) (third-party aggregator, dated 2026, unverified)
