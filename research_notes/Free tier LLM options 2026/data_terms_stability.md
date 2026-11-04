# Free-tier LLM APIs: data-use terms and stability (as of 2026-11-04)

**Access note (read first).** Searches were run on 2026-11-04. Almost every official provider page was unreachable from this sandbox: WebFetch returned `getaddrinfo ENOTFOUND` for help.mistral.ai, openrouter.ai, groq.com, developers.cloudflare.com, www.cerebras.ai, cdn.deepseek.com, www.together.ai and huggingface.co. Those hosts need to be added to the session's network allowlist before anything can be verified from primary sources. ai.google.dev was not retried, as instructed. Everything below marked **[snippet]** comes from WebSearch result text, not from a fetched page. Everything marked **[secondary]** comes from an aggregator, blog or GitHub tracker. Quotes are marked as either verbatim-from-search-summary or paraphrase. Treat all of it as unverified until the primary pages are read. SambaNova, DeepSeek, Hugging Face and Cerebras are the weakest on sources.

## Key Question 1: Is free-tier traffic used for training, is there an opt-out, and how long is data retained?

### Takeaway
Only Google (Gemini API free tier) has a clearly stated "free tier data is used to improve products" rule, and that rule is the reason free keys should not get customer or player data. Groq, Cloudflare Workers AI and Together state no-training commitments (Together's is opt-in and off by default). DeepSeek and SambaNova are unclear or contested. For OpenRouter free models, the data policy depends on the upstream host of each model, so it cannot be answered for the "openrouter/free" router as a whole. Retention periods are published for almost none of these providers in a form I could confirm.

### Cited Findings

**Google Gemini API (free tier)**
- Billing page, paraphrased in snippet: paid-tier users get higher rate limits, and "your prompts and responses aren't used to improve Google products" — [Google billing docs](https://ai.google.dev/gemini-api/docs/billing?authuser=6) [snippet]
- Gemini API additional terms (unpaid services), snippet text: "To help with quality and improve our products, human reviewers may read" (truncated in the snippet; the snippet also says Google may use submitted prompts and generated responses to "provide, improve, and develop its products") — [Gemini API terms preview](https://ai.google.dev/gemini-api/terms_preview?hl=tr) [snippet]. Not fetched, so the full wording is unconfirmed.
- Wikipedia notes Google advises against submitting sensitive information on the free tier — [Google AI Studio (Wikipedia)](https://en.wikipedia.org/wiki/Google_AI_Studio) [secondary]
- One blog (Spur) says prompts are not used to improve products. It appears to describe the paid tier and contradicts the official terms, so I rely on the Google pages — [Spur](https://www.spurnow.com/en/blogs/is-gemini-api-free) [secondary]
- Retention period for free-tier data: not found.

**Groq (free tier)**
- Archived legal page, quoted in search summary: "Groq will not use such Customer Training Data other than to provide the Cloud Services to Customer." The archived page is about 623 days old by the summary's own count. Groq's current policy was not read — [GroqDocs legal archive](https://console.groq.com/docs/legal/archive/) [snippet; secondary date note]
- Third-party guide: inference requests are not kept by default, but logs may persist for troubleshooting and abuse review, up to about 30 days; Zero Data Retention can be enabled in Console Data Controls — [Meetily](https://meetily.ai/llm-privacy/groq) [secondary]; [Humla](https://humla.team/blog/groq-data-retention-policy) [secondary]
- One review notes a page meta description still says "standard 30-day retention", which conflicts with the body text — [Humla](https://humla.team/blog/groq-data-retention-policy) [secondary]
- Unconfirmed whether the free tier can enable ZDR. One author says they could not verify this first-hand — [Neural Base](https://theneuralbase.com/groq/learn/advanced/data-privacy-and-residency/) [secondary]

**Mistral (free "Experiment" plan)**
- See Key Question 2.

**OpenRouter (free models)**
- Data policy: the pricing page says data-policy-based routing is paid-plan only. Per the search summary, free endpoints can log prompts, and the host's policy depends on the model. OpenRouter describes its provider data-policy table as its best knowledge — [OpenRouter pricing](https://openrouter.ai/pricing) [snippet]
- TrueFoundry analysis: about nine of 19 free models run on hosts whose policies allow training on prompts. Count not checked against OpenRouter — [TrueFoundry](https://www.truefoundry.com/blog/openrouter-free-models-whats-actually-free-and-what-it-costs-you) [secondary]

**Cerebras (free tier)**
- Aggregator quoting the ToS (paraphrase): Cerebras's terms give it no right to use API inputs and outputs to train or fine-tune models. A separate aggregator says the privacy policy states it "does not retain inference inputs and outputs" — [DeveloperDigest roundup](https://www.developersdigest.tech/models/free-api) [secondary]; the same roundup lists Cerebras among providers that say they do not use API data for training. Cerebras's own terms were not read.
- Retention: the privacy-policy claim above is the only retention statement found, and it is secondhand.

**Cloudflare Workers AI**
- Official data-usage page, paraphrased in the search summary: Cloudflare "does not use Customer Content processed by Workers AI to train AI models." The same is said on its Responsible AI page — [Workers AI data usage](https://developers.cloudflare.com/workers-ai/platform/data-usage/) [snippet]
- Retention (paraphrase from the same snippet): for standard inference, inputs and outputs are not retained after they are no longer needed to process the request. Exceptions: storage bindings (R2, KV, Durable Objects, Vectorize), AI Gateway logging or caching, and application logs. Metadata-only AI Gateway logging is available by per-request header `cf-aig-collect-log-payload: false` — [Cloudflare architecture guide](https://architectingoncloudflare.com/chapter-16/) [secondary]
- The no-training policy covers Workers AI only. Cloudflare's own guide warns it does not establish the retention behaviour of external providers reached through AI Gateway — [Cloudflare architecture guide](https://architectingoncloudflare.com/chapter-16/) [secondary]
- Security note (date unknown): Knostic reported a cross-tenant exposure in Workers AI on the glm-5.3 model, where a large request could return content from another inference session. This is a data-isolation issue, not training — [Knostic](https://www.knostic.ai/blog/hallucination-or-cross-tenant-when-an-llm-answers-with-another-users-context) [secondary]

**SambaNova (free tier)**
- Could not find an official answer on training or retention. An open issue in a free-API tracker says the only answer found was a staff reply in a community forum, which is not official — [free-llm-api-hub issue #262](https://github.com/pacocartones/free-llm-api-hub/issues/262) [secondary]
- An aggregator's trust rubric lists retention and training opt-out as "Unclear" — [free-llm-api-hub issue #262](https://github.com/pacocartones/free-llm-api-hub/issues/262) [secondary]

**DeepSeek (first-party API)**
- Sources conflict. One group says training on API inputs is on by default with an email-only opt-out. Another reads the API terms as silent on training while the privacy policy lists training as a purpose, and it notes that the policy gives a right to opt out. An independent analysis says the terms allow training and that there was no clear opt-out mechanism for the developer API as of April 2026 — [Meetily](https://meetily.ai/llm-privacy/deepseek) [secondary]; [DeveloperDigest](https://www.developersdigest.tech/models/data-retention/deepseek) [secondary]; [zdr.dev](https://zdr.dev/e/deepseek-api-firstparty) [secondary]
- Retention: no fixed period published. One source quotes "for as long as you have an account", and another quotes "as long as necessary to provide services" — [Isoqar India](https://isoqarindia.com/deepseek-privacy-policy-a-critical-analysis-of-risks-and-implications/) [secondary]. No zero-data-retention option is documented for the first-party API.
- Storage location: one source says data is stored in the "People's Republic of China"; a later check says the policy no longer names China — [Unseen Security](https://www.unseensecurity.ai/ai-services/deepseek) [secondary]. Not resolved.
- DeepSeek's terms PDF (cdn.deepseek.com) could not be fetched.

**Together AI**
- Training: "Allow organization's data for training" is off by default, under Organization Settings > Privacy. The privacy docs say data-sharing for training is opt-in — [Together privacy and security docs](https://docs.together.ai/docs/privacy-and-security) [snippet]
- ZDR: available at the organization level. With it on, prompts and outputs are not stored and not used for secondary purposes. Uploaded fine-tuning or batch files are stored until deleted. Some "passthrough" models forward prompts to upstream providers under their own policies, controlled by a separate toggle — [Together ZDR docs](https://docs.together.ai/docs/zero-data-retention) [snippet]
- Privacy policy, paraphrased in snippet: data will not be used for training without explicit opt-in and consent — [Together privacy policy](https://www.together.ai/privacy) [snippet]

**Hugging Face Inference Providers**
- No Hugging Face retention or training statement was found. Requests are routed to partner providers, so their policies likely apply. This was not confirmed — [HF Inference Providers pricing](https://huggingface.co/docs/inference-providers/en/pricing) [snippet]

### Inferences
- For a public web game, the user's transcripts include player chat, which is personal data from third parties. Only a provider with a clear no-training and short-retention commitment should receive it. On current evidence that is Cloudflare Workers AI (no training; retention depends on configuration) and Together with ZDR enabled, if those terms hold on paid plans too. Google's free tier should be excluded for this data, because its terms say free-tier prompts may be read by human reviewers and used to improve products.
- OpenRouter's free models should be treated as unsafe for player data unless each chosen model's host is verified, because the host policy varies by model.

### Gaps
- None of the Google, Groq, Mistral, OpenRouter, Cerebras, Cloudflare, DeepSeek, Together or HF primary pages could be read in this session (network). All findings above rest on snippets or secondary sources.
- SambaNova: no primary source found.
- Google free-tier retention period: not found.
- Cerebras and DeepSeek retention periods: not confirmed from primary sources.

## Key Question 2: Mistral's free "Experiment" tier: does it require opting into training?

### Takeaway
The sources do not support the premise that Experiment requires opting in. They say the free Experiment plan trains on data by default, with an opt-out that is a toggle in the console. Exact terms could not be read, so this is the most important item to verify on the primary help page.

### Cited Findings
- Search summary, paraphrase: Experiment plan users "may opt out of data training for Mistral AI Studio and related API services" by disabling a toggle under the "Anonymous improvement data" section — [Mistral help article 455207](https://help.mistral.ai/en/articles/455207-can-i-opt-out-of-my-input-or-output-data-being-used-for-training) [snippet; page not fetched]
- A secondary source says the free Experiment tier lets Mistral use prompts to improve its models unless you opt out — [Shieldra](https://www.shieldra.ai/ai-vendors/mistral-la-plateforme) [secondary]
- Another secondary report says that from March 12, 2026, Mistral's Commercial Terms allow customer data and outputs to train its models for free subscribers. One commentary reports that Mistral removed a no-training sentence from its privacy policy, while the commercial terms and help pages still describe no-training protections for defined paid plans — [Platform Governance Archive](https://www.platformgovernancearchive.org/2026/mistral-ai-broadens-scope-user-data-and-search-results-now-used-for-training/) [secondary]; [OpenVeil](https://openveil.app/blog/mistral-ai-privacy-policy-training-paid-api) [secondary]
- Paid tiers: one help-center summary says Scale plan customers are opted out of training by default. Another analysis reads the help page as excluding pay-as-you-go API data — [Shieldra](https://www.shieldra.ai/ai-vendors/mistral-la-plateforme) [secondary]
- Exceptions on any plan (per search summary): thumbs up/down feedback can be used for training, and "Labs" models can train on data regardless of opt-out when activated — [Mistral help article 455207](https://help.mistral.ai/en/articles/455207-can-i-opt-out-of-my-input-or-output-data-being-used-for-training) [snippet; page not fetched]
- An unconfirmed Hacker News question asks whether opting in still earns free credits. No confirmation found — [HN thread](https://news.ycombinator.com/item?id=49535284) [secondary]

### Inferences
- The user's premise ("requires opting into training") conflicts with every source found. The better reading is default-on training with an opt-out. Whether that changed in March 2026 is unclear from secondary sources. The primary help article and Commercial Terms must be read to settle it.

### Gaps
- Exact current wording of the Experiment plan terms: not confirmed. help.mistral.ai was unreachable.
- Whether the Commercial Terms change of March 12, 2026 applies to the Experiment plan: not confirmed.

## Key Question 3: OpenRouter free models: how are they listed, how often do they change, and is there a stable ID?

### Takeaway
Free models carry a `:free` suffix on their model ID and rotate often. The `openrouter/free` router is the only stable free ID found, and it picks a random free model under the same rate limits. Per-model IDs have been retired on short notice. Plan for two or three fallbacks.

### Cited Findings
- Free models are limited to 20 requests per minute and 50 per day. Buying at least $10 in credits raises the daily limit to 1,000. The per-minute limit is account-wide, so extra keys don't help. Failed attempts count toward the daily limit. A negative balance blocks free models — [OpenRouter pricing](https://openrouter.ai/pricing) [snippet]; [Fast.io guide](https://fast.io/resources/openrouter-rate-limit/) [secondary]
- The `openrouter/free` router selects a random free model and counts against the same limits — [OpenRouter Free Models Router](https://openrouter.ai/openrouter/free) [snippet]
- Free-model count: sources disagree. The pricing page reportedly says 25+ and one guide says 19. One tracker logged 24 free models on 2026-09-21, with 1 removed and 0 added — [GitHub tracker issue #75](https://github.com/cyclez2000/openrouter-free-models/issues/75) [secondary]
- The official model list filtered to free models is the reference for what is currently free: openrouter.ai/models?pricing=free, as cited by a secondary guide — [PriceperToken](https://pricepertoken.com/endpoints/openrouter/free) [secondary]. Not fetched.
- Sources recommend listing two or three free fallbacks because models rotate in and out with little notice — [OpenRouter pricing](https://openrouter.ai/pricing) [snippet; search summary]

### Inferences
- `openrouter/free` is the only ID that will likely survive model churn. Its underlying model, and therefore its host and data policy, will change at random. That makes it unsuitable for a fixed data-policy assumption.

### Gaps
- Official OpenRouter changelog and deprecation notices: not reached (site unreachable). Dates below come from third-party trackers.
- Whether the router excludes hosts with training-on-prompt policies: not found.

## Key Question 4: How often have free models been removed or moved to paid in the past year?

### Takeaway
Removals are frequent for OpenRouter and for Google's Gemini family. The evidence is from third-party trackers and release notes that I could only see as snippets. No provider publishes a rate, and I found no complete changelog for any free tier.

### Cited Findings
- OpenRouter: a deprecation batch of six `:free` models with dates between July 19 and 21, 2026 (including qwen/qwen3-coder:free on July 19 and tencent/hy3:free on July 21), listed in a gist — [gist](https://gist.github.com/rlnorthcutt/e6f392cd1ffb1339cc42dfb024c3cf7f) [secondary]. The gist also says qwen3-coder:free is deprecating, which conflicts with an OpenRouter page quoted in another source.
- OpenRouter: the meta-llama/llama-3.3-70b-instruct:free page showed a "Going away July 19, 2026" banner, per a GitHub issue. The issue says the page recommends gpt-oss-20b:free or qwen3-coder:free as replacements — [digithings issue #1592](https://github.com/digithings-ai/digithings/issues/1592) [secondary]. The same issue flags deepseek-chat-v3:free, deepseek-r1:free, gemini-2.0-flash-exp:free and qwen3-235b-a22b:free as possibly no longer free. This was not verified.
- OpenRouter: one tracker logged the removal of a DeepSeek V4 Flash free variant on 2026-09-21 — [GitHub tracker issue #75](https://github.com/cyclez2000/openrouter-free-models/issues/75) [secondary]. A release note lists MiniMax M2.5, DeepSeek V4 Flash and Baidu CoBuddy `:free` variants as dropped — [free-coding-models v0.3.69](https://github.com/vava-nessa/free-coding-models/releases/tag/v0.3.69) [secondary]. Other removals (tencent/hy3-preview:free, inclusionai/ling-2.6-1t:free, google/gemma-3n-e2b-it:free) are attributed to providers dropping their free tiers — [free-coding-models v0.4.3](https://github.com/vava-nessa/free-coding-models/releases/tag/v0.4.3) [secondary].
- Google Gemini: the Gemini 2.0 Flash models (including -001 and Lite variants) were scheduled to shut down on June 1, 2026, per an announcement dated February 18, 2026 — [Gemini deprecations](https://ai.google.dev/gemini-api/docs/deprecations.md) [snippet]; [release notes](https://ai.google.dev/gemini-api/docs/changelog) [snippet]; [AI Weekly](https://aiweekly.co/alerts/google-retires-gemini-20-flash-001-replace-with-25-flash) [secondary]
- Google Gemini: Firebase docs say Gemini 2.5 models will shut down in October 2026 — [Firebase AI Logic data governance](https://firebase.google.com/docs/ai-logic/data-governance?authuser=9&hl=pl) [snippet]. Some sources give October 16, 2026. That date has already passed as of 2026-11-04, and I could not confirm whether it was extended.
- Google Gemini: third-party sources say Pro models have been dropped from the free tier since April 1, 2026, leaving 2.5 Flash, 2.5 Flash-Lite, 3 Flash and 3.1 Flash-Lite with reduced daily quotas. Not confirmed on Google's pages — [CloudZero](https://www.cloudzero.com/blog/gemini-pricing/) [secondary]
- Hugging Face: a GitHub issue says that since 2026-10-07, free accounts get no Inference Providers credits; earlier pages still advertised about $0.10 per month. This is a secondhand tracker report — [agentdeals issue #2732](https://github.com/robhunter/agentdeals/issues/2732) [secondary]
- Cerebras: free access became a trial. New accounts reportedly get $5 in credits after adding a verified payment method, expiring 30 days after they are granted, and Cerebras reportedly says there is no permanently free tier — [BenchLM](https://benchlm.ai/free-tier/cerebras) [secondary]; [agentdeals](https://agentdeals.dev/vendor/cerebras) [secondary]
- SambaNova: one source says the $5 free-credit grant was removed as of August 2026. Another says the service is paid with no free tier. Sources conflict on the current state — [costbench](https://costbench.com/software/llm-api-providers/sambanova-cloud/free-plan/) [secondary]
- Together AI: no free trial, and a $5 minimum credit purchase is required, per a billing-page summary. The earlier $25 signup credit reportedly ended in July 2025. Some aggregators still list a $5 signup credit — [BenchLM](https://benchlm.ai/free-tier/together) [secondary]; [aipricing.guru](https://www.aipricing.guru/together-pricing/) [secondary]

### Inferences
- The past year shows at least three kinds of change: per-model `:free` removals on OpenRouter (roughly monthly in the sources, though the sample is small), Google retiring models on fixed dates, and several providers (Cerebras, SambaNova, Together, Hugging Face) moving from free to trial or paid. Treat any free tier as able to disappear at short notice.

### Gaps
- No provider's own changelog for free-tier removals was read. Frequency rates are only inferred from a few tracker data points.
- Google free-tier pricing changes: not confirmed on ai.google.dev.

## Key Question 5: Are free tiers allowed for commercial or public-facing use, and do their terms restrict apps like a public web game?

### Takeaway
Not answered. I did not find or read any provider's acceptable-use or commercial-use terms for its free tier. Do not assume the free tiers permit a public game. Read each provider's terms before use.

### Cited Findings
- None. No source was found that states free-tier commercial-use or public-app restrictions for any of the ten providers.

### Inferences
- The only usage constraint I can state with support is the Google point above, which is about data use, not commercial use: free-tier prompts may be used by Google for product improvement and read by human reviewers. A public game that sends player chat would expose that chat to those practices.

### Gaps
- Commercial and public-app use terms: not checked for any provider. All primary terms pages were unreachable (except that the Google pages were not fetched at all).
- Acceptable-use restrictions on games or user-facing apps: not checked.

## Summary of access blockers

Add these hosts to the session network allowlist to verify the findings above: help.mistral.ai, openrouter.ai, groq.com, developers.cloudflare.com, www.cerebras.ai, cdn.deepseek.com, www.together.ai, huggingface.co. Also add the sites for SambaNova (cloud.sambanova.ai) and ai.google.dev (ai.google.dev was blocked, as the brief noted, and was not retried).
