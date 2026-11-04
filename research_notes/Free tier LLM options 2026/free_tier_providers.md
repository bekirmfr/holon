# Free-tier LLM API providers and per-model limits (as of 2026-11-04)

## Key question 1: Which providers and models have a free tier now, and what are their RPM, TPM, RPD and monthly caps?

### Takeaway
No official provider page could be reached from this sandbox (most official hosts fail DNS resolution; see Gaps). Every figure below is therefore either the user's own dashboard reading (Gemini) or comes from third-party aggregator sites, and most providers conflict across sources. Treat every number as "unverified (aggregator only)" until checked on the provider's console. The strongest-supported picture: Cloudflare Workers AI gives a 10,000-Neuron daily pool; OpenRouter `:free` models are capped at 20 RPM and 50 or 1,000 RPD depending on lifetime credit purchases; Cerebras appears to have dropped its standing free tier in favour of a trial credit; Hugging Face gives under $0.10 of Inference Providers credit per month.

### Summary table (all figures unverified unless marked)

Read date for every row: 2026-11-04 (sandbox date). Dates quoted inside sources are given where the source stated one. Nothing below was read from an official page in this session.

| Provider | Free models (per sources) | RPM | TPM | RPD | Monthly / daily cap | Credit card | Evidence status |
|---|---|---|---|---|---|---|---|
| Google Gemini | gemini-3.5-flash-lite | 15 (user dashboard screenshot); 10 (one aggregator) | 250K (user dashboard); generic "~250K TPM Flash-class" (cost-guide aggregator) | 500 (user dashboard; one aggregator) | not found | not found | Official page (ai.google.dev) blocked. Dashboard reading is the user's own; aggregator disagrees on RPM. Model names gemini-3.6-flash and gemini-3.8-flash: not found in any source. |
| Gemini | gemini-3.6-flash, gemini-3.8-flash | 5 (user only) | not found | 20 (user only) | not found | not found | Unverified (user statement only). No source found. |
| Groq | llama-3.3-70b-versatile, llama-3.1-8b-instant | 30 (aggregator) | not found | 14,400 (aggregator) | not found | not found | Unverified (aggregator only). Conflicting. |
| Groq | openai/gpt-oss-120b | 30 (aggregator, cites console) | 8K (aggregator) | 1,000 (aggregator) | TPD 200K (aggregator) | not found | Unverified (aggregator only). |
| Groq | llama-4-maverick | 15 (aggregator) | not found | 500 (aggregator) | not found | not found | Unverified (aggregator only). |
| Groq | whisper models | 20 (aggregator) | not found | 2,000 (aggregator) | not found | not found | Unverified (aggregator only). |
| Mistral (La Plateforme "Experiment" plan) | Per-model; list not found | ~60 (1 request/second, aggregator) | 500K (aggregator; free-llm.com notes the exact TPM is not published) | not found | 1B tokens/month per model (aggregator) | No card; phone verification required, plus opt-in to data training (BenchLM aggregator) | Unverified (aggregator only). Mistral's help article reportedly publishes no figures (BenchLM, read Sept 2026). A "~300 RPM after billing card" claim (RapidDev) has no official source: unverified. |
| OpenRouter | `:free` model variants (16 to 25+ IDs; sources disagree on count) | 20 (provider page as summarised by BenchLM; aggregator) | not found | 50 if account has never bought $10 of credits; 1,000 if it has (aggregator summarising provider page) | none stated | Card not stated as required; a $10+ purchase (5.5% fee, $0.80 minimum per aggregator) unlocks 1,000/day | Unverified (aggregator only). openrouter.ai unreachable. |
| Cerebras | Trial: gpt-oss-120b, qwen-3.8-27b (aggregator) | 5 (trial, aggregator citing official limits docs) | 30K uncached / 90K total (trial, aggregator) | not found | 1M tokens/hour and 1M tokens/day (trial, aggregator) | Verified payment method required for the trial (aggregator, Oct 2026 page) | Unverified (aggregator only). Conflicting: older pages list a standing free tier (30 RPM, 1M tokens/day for Llama 3.3 70B and Qwen3). A developer page checked Oct 2026 says there is no standing free tier. Developer tier for gpt-oss-120b: 1K RPM, 1M/3M TPM (aggregator). |
| Cloudflare Workers AI | All models share one pool | not found | not found | not found | 10,000 Neurons/day free; resets 00:00 UTC (third-party); no overage on Free plan; Workers Paid ($5/month) includes the same 10,000 and bills $0.011 per 1,000 Neurons above it (aggregator; a July 2026 Cloudflare changelog entry is cited for the $5 Paid plan and for some models requiring Workers Paid) | not stated | Partly supported. Pool size appears on the official Cloudflare docs URL in search results but that page was not fetched. Per-model RPM/RPD: not found. |
| SambaNova Cloud | Free Tier (no payment method linked) and Developer Tier (payment method attached) | Free tier: 20 (aggregator; per model in one source, across all models in another) | Free tier: not clearly stated | Free tier: 20 (same conflict); 200K tokens/day free (aggregator) | Developer tier: 20M tokens/day across all models (search summary of official docs, not fetched) | Free tier applies when no payment method is linked (search summary of official docs) | Mixed. Tier structure and Developer 20M/day come from search-result summaries of official docs (docs.sambanova.ai, not fetched). Free-tier RPM/RPD are unverified. Starter credit: $5, expiry 30 days vs 3 months (conflict). |
| Together AI | Serverless chat models | ~60 (1 RPS for new accounts, aggregator) | 60,000 for chat models (aggregator) | not found | Conflicting: one source says no free tier and a $5 minimum purchase; others list $1 or $5 signup credits | Card/purchase required per the billing-page claim (aggregator) | Unverified (aggregator only). Conflicting on whether a free tier exists. Tier 1 ($25 paid): 600 RPM, 180K TPM (aggregator). |
| Hugging Face Inference Providers | Providers integrated with HF billing | not found | not found | not found | Free accounts: under $0.10 of credit per month; PRO: $2.00/month; cut off when exhausted (official HF docs URL in search result, not fetched; forum post agrees) | not found | Partly supported. The search result pointed to huggingface.co/docs/inference-providers/en/pricing but the fetch failed, so the $0.10 / $2.00 figures are from search summaries only. A third-party 2026 guide claims the free credit may have ended on 15 Oct 2026: unconfirmed. |
| DeepSeek | not found | not found | not found | not found | not found | not found | Not found. api-docs.deepseek.com unreachable and not searched further. |
| Other providers | Not researched beyond the list above (time budget). | | | | | | Not searched. |

### Cited Findings

Gemini
- gemini-3.5-flash-lite free tier: 500 RPD and 10 RPM; daily reset at midnight Pacific; model ID gemini-3.5-flash-lite — [ScriptByAI-style aggregator, yingtu/apisrouter/scriptbyai results from WebSearch](https://yingtu.ai/en/blog/google-gemini-api-free-tier-limits-2026); aggregator only.
- Free limits apply per Google Cloud project; several API keys in one project do not multiply quota — [Gemini API rate-limits aggregator results](https://www.scriptbyai.com/gemini-api-rate-limits/); aggregator only.
- Generic Flash-class approximations: ~10 RPM, ~250K TPM, a few hundred RPD — [Google Gemini pricing cost guide](https://apisrouter.com/gemini-api-cost-guide); aggregator only, not model-specific.
- Google's rate-limits page (ai.google.dev) returned only localized copies in search results; a table could not be matched to free-tier numbers for gemini-3.5-flash-lite — [Google rate limits page, localized copy](https://ai.google.dev/gemini-api/docs/rate-limits?authuser=54&hl=de); not confirmed.

Groq
- Free-tier limits are set per model across RPM, RPD, TPM and TPD; 429 on breach; RPM uses a rolling 60-second window and RPD a 24-hour UTC window — [Groq rate-limit aggregator, fast.io](https://fast.io/resources/groq-rate-limit.md); aggregator only.
- Aggregator figures conflict: llama-3.3-70b-versatile and llama-3.1-8b-instant at 30 RPM / 14,400 RPD; gpt-oss-120b at 30 RPM / 1K RPD / 8K TPM / 200K TPD (cites console); llama-4-maverick 15 RPM / 500 RPD; whisper 20 RPM / 2,000 RPD — [Groq free tier, fremodel](https://fremodel.pages.dev/providers/groq); [costbench](https://costbench.com/software/llm-api-providers/groq/); [developersdigest](https://www.developersdigest.tech/models/free-api/groq). Aggregator only.
- Groq community says exact per-account limits appear under the Limits section of GroqConsole — [Groq community thread](https://community.groq.com/t/is-there-a-free-tier-and-what-are-its-limits/790); forum post, not official docs.

Mistral
- Experiment (free) plan exists; per-model limits about 1 request/second, 500K TPM, 1B tokens/month per model — [free-llm.com listing](https://free-llm.com/provider/mistral-ai); [mintlify free-llm-api-resources](https://mintlify.com/cheahjs/free-llm-api-resources/providers/free/mistral-plateforme); aggregator only.
- Phone verification required; free tier requires opting in to data training; adding credit alone does not raise limits — [BenchLM Mistral free tier](https://benchlm.ai/md/free-tier/mistral.md); aggregator, states it read Mistral's help article in Sept 2026.
- Mistral's help article reportedly publishes no RPS or token figures — same BenchLM source.
- A "~300 RPM after billing card" claim appears on RapidDev — [RapidDev Mistral Large](https://www.rapidevelopers.com/md/ai-api-limits-performance-matrix/mistral-large); no official source, so unverified.

OpenRouter
- `:free` models: 20 requests/minute; 50 requests/day for accounts with under $10 lifetime purchases; 1,000/day once $10 has been bought (permanent); limits are per account across all keys; negative balance returns 402 even on free models — [BenchLM OpenRouter free models](https://benchlm.ai/md/free-tier/openrouter.md); [TrueFoundry blog](https://www.truefoundry.com/ar/blog/openrouter-rate-limits-your-free-tier-depends-on-what-youve-already-paid); aggregator summarising provider page.
- Purchase fee 5.5% with $0.80 minimum — [developersdigest OpenRouter](https://www.developersdigest.tech/models/free-api/openrouter); aggregator only.
- Free models are rate-limited hard at peak times and rotate out without notice — [Pinggy blog](https://pinggy.io/blog/free_ai_model_apis_unlimited_tokens_openrouter/); blog, not used for any figure.

Cerebras
- Developer page checked Oct 2026: no standing free tier; one-time trial credit after adding a verified payment method — [developersdigest Cerebras](https://www.developersdigest.tech/models/free-api/cerebras); aggregator.
- Trial $5 expiring in 30 days — [Cline provider docs](https://docs.cline.bot/provider-config/cerebras); secondary.
- Trial limits per model (gpt-oss-120b, qwen-3.8-27b): 5 RPM; 30K uncached / 90K total TPM; 1M tokens/hour and /day — [BenchLM Cerebras free tier](https://benchlm.ai/md/free-tier/cerebras.md); aggregator claiming to cite official limits docs.
- Older pages (30 RPM, 1M tokens/day for Llama 3.3 70B and Qwen3) conflict with the above — [Morph Cerebras pricing](https://www.morphllm.com/cerebras-pricing); outdated per the newer sources.

Cloudflare Workers AI
- 10,000 Neurons/day free allocation, shared across models, resets 00:00 UTC — [Cloudflare pricing docs URL in search results](https://developers.cloudflare.com/workers-ai/platform/pricing/) (not fetched; figure as reported by search summary); [freeapihub tutorial](https://freeapihub.com/blog/cloudflare-workers-ai-tutorial-free-neurons) (third-party reset claim).
- Workers Paid starts at $5/month and includes the 10,000 free Neurons; overage $0.011 per 1,000 Neurons; some models require Workers Paid — [Cloudflare changelog 2026-07-28](https://developers.cloudflare.com/changelog/post/2026-07-28-models-require-workers-paid/) (URL seen in search results, not fetched); [costbench Cloudflare free plan](https://costbench.com/software/llm-api-providers/cloudflare-workers-ai/free-plan/).
- Per-model RPM/RPD: not found.

SambaNova
- Free Tier applies when no payment method is linked; Developer Tier applies once one is attached; Developer cap 20M tokens/day across all models — [SambaNova rate-limits docs URL](https://docs-prod.sambanova.ai/docs/en/models/rate-limits) (search-result summary only; page not fetched).
- Free tier: 20 RPM and 20 RPD (per model in one source, across all models in another); 200K tokens/day — [costbench SambaNova free plan](https://costbench.com/software/llm-api-providers/sambanova-cloud/free-plan/); [toolfreebie](https://toolfreebie.com/sambanova-cloud-free-api/); aggregator, conflicting.
- Starter credit: $5, expiring in 3 months vs 30 days (conflict) — aggregators.
- Early-2026 billing migration reportedly left some accounts stuck on free limits after adding a card — aggregator, unverified.

Together AI
- Conflicting: no free tier and a $5 minimum purchase (billing-page claim) vs $1 or $5 signup credits on older sites — [BenchLM Together free tier](https://benchlm.ai/md/free-tier/together.md); [fast.io Together rate limits](https://fast.io/resources/together-ai-rate-limit.md); aggregators.
- New accounts 1 RPS (60 RPM) rising with funding; one site lists 60K TPM for chat models; Tier 1 ($25 paid) 600 RPM, 180K TPM — aggregators, conflicting.

Hugging Face Inference Providers
- Free accounts: under $0.10 of Inference Providers credit per month; PRO $2.00/month credit; free users cut off when it runs out — [HF Inference Providers pricing, search-result URL](https://huggingface.co/docs/inference-providers/en/pricing) (fetch failed; summary from search); [HF forum post](https://discuss.huggingface.co/t/about-membership-and-subscription/173746) (community, same figures).
- Third-party 2026 guide says the free credit may have ended 15 Oct 2026 — [activepieces HF guide](https://www.activepieces.com/blog/hugging-face-inference-api-pricing-a-2026-guide.md); unconfirmed.

### Inferences
- No provider's free-tier figures can be treated as confirmed. The Gemini baseline in the user's dashboard is the only first-party number in this file, and the aggregator disagrees on RPM (10 vs 15).
- The 5 RPM / 20 RPD figures the user gives for gemini-3.6-flash and gemini-3.8-flash appear nowhere in the sources reached; if the dashboard shows them, they are the best available data.
- Free tiers have been tightening across the board (Cerebras, Together, Mistral phone/data-training requirements, OpenRouter purchase unlock), so aggregator pages dated 2024 or early 2026 should be discarded.

### Gaps
- Official rate-limit pages for Groq, Mistral, OpenRouter, Cerebras, Cloudflare, SambaNova, DeepSeek, Together and Hugging Face were not fetched: DNS failed (see the host list below).
- Gemini: ai.google.dev is blocked; the 15 RPM / 250K TPM / 500 RPD figures and the 5 RPM / 20 RPD figures for 3.6/3.8 flash are not confirmed from any source here.
- DeepSeek: no free-tier data found.
- Cloudflare per-model RPM/RPD: not found.
- Credit-card requirements: confirmed only for Mistral (phone verification, aggregator) and Cerebras (trial, aggregator). Not found for the others.
- Other providers with free tiers (beyond the named list): not searched.

## Key question 2: Do limits apply per project, per key, or per account?

### Takeaway
Only two sources speak to scope, both aggregators: Gemini limits are per Google Cloud project, and OpenRouter limits are per account across all keys. Other providers' scope is not confirmed.

### Cited Findings
- Gemini: free limits apply per Google Cloud project; multiple keys within one project do not add quota — [ScriptByAI Gemini rate limits](https://www.scriptbyai.com/gemini-api-rate-limits/); aggregator only.
- OpenRouter: limits enforced per account across every key; extra keys or accounts add no capacity — [BenchLM OpenRouter](https://benchlm.ai/md/free-tier/openrouter.md); aggregator.
- Mistral: limits applied per model, not shared across models — [free-llm.com](https://free-llm.com/provider/mistral-ai); aggregator.
- Groq: limits set per model — [fremodel Groq](https://fremodel.pages.dev/providers/groq); aggregator. Whether they also apply per organization: not confirmed.
- Cloudflare: a single daily Neuron pool shared across models — [freeapihub](https://freeapihub.com/blog/cloudflare-workers-ai-tutorial-free-neurons); aggregator. The pool is per account in practice, but no official confirmation was read.
- SambaNova: conflict on whether 20 RPM / 20 RPD is per model or across all models — aggregators.

### Inferences
- Plan on the most conservative reading: assume limits are per account and per model until your own console says otherwise. Creating extra keys is unlikely to add capacity for any provider.

### Gaps
- Scope not found for DeepSeek, Together, Hugging Face, Cerebras (trial), and Gemini's model-level split.

## Key question 3: When do daily quotas reset?

### Takeaway
Gemini (midnight Pacific) and Cloudflare (00:00 UTC) have aggregator-sourced reset times. Groq's RPD window is described as a rolling 24-hour UTC window. Others are not confirmed.

### Cited Findings
- Gemini daily RPD resets at midnight Pacific time — [yingtu.ai](https://yingtu.ai/en/blog/google-gemini-api-free-tier-limits-2026); aggregator.
- Cloudflare free Neuron pool resets daily at 00:00 UTC and does not accumulate — [freeapihub](https://freeapihub.com/blog/cloudflare-workers-ai-tutorial-free-neurons); aggregator.
- Groq RPD counted over a 24-hour UTC window; RPM over a rolling 60-second window — [fast.io Groq](https://fast.io/resources/groq-rate-limit.md); aggregator.
- OpenRouter, Mistral, Cerebras, SambaNova, Together, Hugging Face: reset behaviour not found in sources read.

### Inferences
- The three reset conventions (midnight Pacific, 00:00 UTC, rolling 24-hour window) differ, so quota planning should use the most restrictive one in use.

### Gaps
- Reset times for most providers not confirmed.

## Key question 4: Which free tiers fit the chess sample?

### Takeaway
The chess sample (about 1 request per move, roughly several thousand tokens of transcript, schema-constrained JSON out) and a possible 8 to 16 calls per move mode are limited mainly by RPD and TPM. Only providers with a large daily pool or a high RPD fit the 8 to 16 call mode for a whole game; at 20 RPD, the Gemini 3.6/3.8 flash figures (user-supplied) allow about 1 to 2 moves per day in that mode.

### Inferences (arithmetic from the figures above; figures are unverified)
- Assumed per call: ~4K input tokens (transcript) plus ~200 output tokens.
- 1 call/move, one game of ~80 plies: ~80 calls and ~340K tokens per game.
  - RPD 500 (Gemini 3.5 flash-lite, aggregator and user figure): ~6 games/day.
  - RPD 20 (Gemini 3.6/3.8 flash, user figure; SambaNova free aggregator): under 1 game/day.
  - RPD 50 (OpenRouter without a $10 purchase): under 1 game/day.
  - RPD 1,000 (OpenRouter after $10 purchase; Groq gpt-oss-120b aggregator): ~12 games/day.
- 8 to 16 calls/move, one move: 8 to 16 RPD. At 500 RPD this is ~30 moves/day; at 20 RPD it is 1 to 2 moves/day; at 50 RPD it is 3 to 6 moves/day.
- TPM for one move at 16 calls: ~64K tokens in a burst. Cerebras trial (90K total TPM, aggregator) and Together's 60K TPM (aggregator) can take that in a single minute but leave little headroom. Mistral's 500K TPM (aggregator) and Gemini's 250K TPM (user figure) have more headroom.
- RPM: at 5 RPM (Gemini 3.6/3.8 flash, user figure), a 16-call move needs ~3.2 minutes, which is slow for live play. At 15 RPM (Gemini 3.5 flash-lite, user figure; 10 per aggregator), ~1 minute. At 30 RPM (Groq aggregator), ~30 seconds. At ~60 RPM (Mistral and Together, aggregator), ~16 seconds.
- Conclusion: for a single-call-per-move sample, Gemini 3.5 flash-lite (if the user's dashboard figures hold) and Mistral's per-model limits fit best; for the 8 to 16 call mode, only a high-RPD option (1,000+ RPD) gives a usable game count, which points to OpenRouter after a $10 purchase or a paid tier elsewhere.

### Gaps
- Token counts per call are assumptions, not measurements.

## Method note and host reachability

Sources were read in this order: WebFetch on official pages, then WebSearch for leads, then one retry of official pages found in search results. All requests below failed at DNS resolution (`getaddrinfo ENOTFOUND`), so they are unreachable from this sandbox and should be allowed in the session's network settings before this research is redone:

- ai.google.dev (blocked by the sandbox, per the brief; not retried)
- console.groq.com
- docs.mistral.ai
- openrouter.ai
- inference-docs.cerebras.ai
- developers.cloudflare.com
- api-docs.deepseek.com
- docs.together.ai
- sambanova.ai
- docs.sambanova.ai
- huggingface.co (search results pointed to huggingface.co pages, but fetch failed)

Search-result URLs on these hosts were used only to note their existence, not as the basis for any figure.

Dates: no official page was read, so no official "as of" date could be recorded. Third-party pages were dated Sept and Oct 2026 by their own text.
