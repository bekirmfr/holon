# Anthropic Claude API: free tier, free credits, and free-use terms (as of 2026-11-04)

Official pages below were fetched on 2026-11-04 (the session date). Pages without a visible date are marked "no visible date". Figures marked UNCONFIRMED come only from secondary sites and are not on an official page.

## 1. Does a free tier, free credits, or a free-usage program exist?

### Takeaway
There is no permanent free API tier. The official pricing FAQ says only that "New users receive a small amount of free credits to test the API", with no amount, expiry or eligibility rule. The only other API credits are monthly credits bundled with paid Max, Ultra and Team plans.

### Cited Findings
- Official pricing FAQ: "Are there free tiers or trials? New users receive a small amount of free credits to test the API. Contact sales for information about extended trials for enterprise evaluation." — [Anthropic pricing, platform.claude.com](https://platform.claude.com/docs/en/about-claude/pricing) (read 2026-11-04; no visible date)
- The same pricing page gives no free-credit amount, expiry or eligibility criteria. Its billing section says "Billing is based on actual monthly usage" and "Credit card and invoicing options are available." — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing)
- Paid-plan API credits: Max $100/month, Ultra $200/month, Team $20 per Standard seat and $100 per Premium seat (pooled, capped at $500/month). Credits expire at the end of each billing cycle "with no rollover". Claiming needs a Console organization link and "You don't need to add a payment method to Claude Platform." Free, Pro and Enterprise plans are "aren't eligible." — [API credits for Max, Ultra, and Team plans](https://platform.claude.com/docs/en/about-claude/api-credits-for-subscribers) (read 2026-11-04; no visible date)
- Credit terms: "Promotional Credits expire at the time indicated when issued"; "Credits may only be used by the holder of the Anthropic account"; "Customer may not transfer or sell Credits." — [Credit terms](https://www.anthropic.com/legal/credit-terms) (effective Mar 4, 2024, per page)
- UNCONFIRMED (secondary sites only): a one-time $5 credit for new Console accounts, expiring about 14 days after claim, with SMS verification. Also a claim of a "credit window closes April 17" from a dev.to post, of unknown year. Neither is on an official page. Treat both as leads only. — [tkmxai.it.com](https://www.tkmxai.it.com/the-free-ai-api-tier-trap), [dev.to](https://dev.to/fithappensml/anthropics-free-credit-window-closes-april-17-dont-miss-it-417i)

### Inferences
- A new developer may get a small, unspecified starter credit. Its size and expiry cannot be confirmed from official sources.
- Subscriber API credits are not free access: they require a paid plan, and they are usable on the Claude API and the Agent SDK in the Console.

### Gaps
- Starter-credit amount, expiry and eligibility: not on any official page fetched. The Help Center article "How do I claim my free credits" (support.anthropic.com/en/articles/8994925) could not be read: the host failed DNS resolution (ENOTFOUND) from this sandbox. support.anthropic.com needs to be allowed in the session's network settings before it can be read.
- Enterprise "extended trials": only "Contact sales", no terms published.

## 2. Which models are included, and what are the rate limits by tier?

### Takeaway
Every current Claude model is paid per token. There are no free-model carve-outs. Rate limits are set per usage tier, and new organizations may start in an "Evaluation" tier with limits below the standard table. Start tier limits are generous for a small app.

### Cited Findings
- Model pricing covers Fable 5.1, Mythos 5.1 (limited availability), Fable 5, Opus 5.5, Opus 5, Opus 4.8, Sonnet 5.5, Sonnet 5, Sonnet 4.6, Haiku 5.5 and Haiku 4.5. Haiku 5.5 (prompts up to 100,000 tokens) is $0.10 per million input tokens and $0.50 per million output tokens. Those are the lowest per-token prices in the table, and they are not free. — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing)
- Start tier (standard limits, RPM / input TPM / output TPM):
  - Haiku 5.5: 4,000 / 2,000,000 / 400,000
  - Sonnet 5.5: 4,000 / 2,000,000 / 400,000
  - Sonnet 5: 1,000 / 2,000,000 / 400,000
  - Opus 5.5: 4,000 / 2,000,000 / 400,000
  - Fable 5.x (combined): 4,000 / 500,000 / 100,000
  — [Rate limits, platform.claude.com/docs/en/api/rate-limits](https://platform.claude.com/docs/en/api/rate-limits) (read 2026-11-04; no visible date)
- Build tier (Haiku 5.5): 10,000 / 5,000,000 / 1,000,000. Scale tier (Haiku 5.5): 13,000 / 10,000,000 / 2,000,000. Same source.
- Monthly spend caps: Start $500, Build $1,000, Scale $200,000. Once the cap is reached, requests return HTTP 429 with no `retry-after` header until the next month. — [Rate limits](https://platform.claude.com/docs/en/api/rate-limits)
- "New organizations and organizations with limited usage history may start in the Evaluation tier, with limits below the standard limits shown on this page." Tiers are assigned automatically "based on usage history and account standing." — [Rate limits](https://platform.claude.com/docs/en/api/rate-limits)
- The page https://platform.claude.com/docs/en/build-with-claude/rate-limits returned HTTP 404 on 2026-11-04. The correct URL is /docs/en/api/rate-limits.

### Inferences
- A small chess app that makes one model call per move (for example, about 2,000 input tokens and 300 output tokens on Haiku 5.5) would cost about $0.00035 per move at the listed prices. This is an estimate from the official prices and the assumed token counts, not a measured figure. At Start tier limits, RPM and token limits are not the constraint for a small app. The binding constraint is spend, which is paid usage with no free allowance.

### Gaps
- Evaluation-tier numeric limits are not published on the rate-limits page.
- The page does not say what usage qualifies an organization to move from Evaluation to Start.

## 3. Data-use terms for API traffic (training and retention)

### Takeaway
The official API terms state that Anthropic may not train on customer content from the commercial services. Retention is "not by default" for prompts and outputs, except for Covered Models (Fable and Mythos 5 family), which require 30 days. No separate rule for free or trial API traffic was found, because no official free-traffic rule exists.

### Cited Findings
- Commercial Terms, Section B: "Anthropic may not train models on Customer Content from Services." Customers "retain[] all rights to its Inputs" and "own[] its Outputs." — [Commercial Terms](https://www.anthropic.com/legal/commercial-terms) (effective Jun 17, 2025, per page)
- API data retention: "Retained data is never used for model training without your express permission." — [API and data retention](https://platform.claude.com/docs/en/manage-claude/api-and-data-retention) (read 2026-11-04; no visible date)
- Same page: "Conversation content (your prompts and Claude's outputs) is not retained by default; the exception is Covered Models, which require 30-day retention."
- Covered Models: "Claude Fable 5.1, Claude Mythos 5.1, Claude Fable 5, and Claude Mythos 5 ... require 30-day data retention and are not available under ZDR unless expressly authorized by Anthropic." — same page
- Zero data retention (ZDR) is arranged with an account representative, not by default. Even under ZDR, "Anthropic may retain data where required by law or where it has been flagged by Anthropic's automated trust and safety systems... up to 2 years." — same page
- Consumer Privacy Policy: "This Privacy Policy does not apply to content that we process on behalf of customers of our business offerings." Its consumer training language is "We may use your Inputs and Outputs to train and improve Anthropic AI models, unless you opt out." — [Privacy policy](https://www.anthropic.com/legal/privacy) (effective Sep 10, 2026, per page). Note: this policy does not cover API traffic.

### Inferences
- For API traffic under commercial terms, the stated position is no training on customer content and no default retention beyond the Covered Model rule. The "not by default" retention language is from a docs page, not from the contract, so it is weaker evidence than the Commercial Terms clause.
- Developers who use free credits or trials would plausibly be on the same API terms, but no official source states this directly.

### Gaps
- The Data Processing Addendum (incorporated by reference in Commercial Terms Section C) was not fetched.
- Commercial Terms do not give a retention period for ordinary API inputs and outputs. The docs page gives only the "not by default" statement.

## 4. Free access outside the API, and use in a public web app

### Takeaway
The claude.ai free plan is a consumer chat product with no API access. Its terms prohibit automated access other than through an API key. Any public app therefore has to use a paid API key, billed per token, or spend subscriber credits from a paid plan.

### Cited Findings
- claude.com/pricing (redirected from anthropic.com/pricing): the Free plan is "Free for everyone" at $0, with "Limited usage every 5 hours and every week" and "Chat on web, desktop, and mobile." The comparison table marks "Claude Code" and "New Monthly API credits" as "No" for Free. The page states "Price and plans are subject to change at Anthropic's discretion." — [Claude pricing](https://claude.com/pricing) (read 2026-11-04; no visible date)
- Consumer Terms, Section 3: prohibits "to access the Services through automated or non-human means, whether through a bot, script, or otherwise," with the exception "Except when you are accessing our Services via an Anthropic API Key." Scraping is separately prohibited. — [Consumer Terms](https://www.anthropic.com/legal/consumer-terms) (effective Oct 8, 2025, per page)
- Consumer plans are not API access. Using a consumer account from an app is prohibited by the Consumer Terms quoted above.
- Credit terms are silent on public-facing applications. They say only that credits may be used by the account holder and cannot be transferred or sold. — [Credit terms](https://www.anthropic.com/legal/credit-terms)

### Inferences
- A public chess web app should call the API with its own API key from a server or a proxy. It should not use a consumer login, and the key should not be placed in browser code. Usage is billed to the key's organization.
- No free route for a public app was found. Paid-plan API credits could fund one, but they require a Max, Ultra or Team subscription.

### Gaps
- Whether the Usage Policy or Commercial Terms restrict specific public app types was not checked.
- The Help Center article on subscriber credits could not be read, because support.anthropic.com is unreachable from this sandbox.

## Sources (date read 2026-11-04 unless noted)
- https://platform.claude.com/docs/en/about-claude/pricing (no visible date)
- https://platform.claude.com/docs/en/api/rate-limits (no visible date)
- https://platform.claude.com/docs/en/manage-claude/api-and-data-retention (no visible date)
- https://platform.claude.com/docs/en/about-claude/api-credits-for-subscribers (no visible date)
- https://claude.com/pricing (redirected from https://www.anthropic.com/pricing; no visible date)
- https://www.anthropic.com/legal/commercial-terms (effective Jun 17, 2025)
- https://www.anthropic.com/legal/consumer-terms (effective Oct 8, 2025)
- https://www.anthropic.com/legal/credit-terms (effective Mar 4, 2024)
- https://www.anthropic.com/legal/privacy (effective Sep 10, 2026)
- Not reached: https://support.anthropic.com/en/articles/8994925 (ENOTFOUND); https://platform.claude.com/docs/en/build-with-claude/rate-limits (HTTP 404)
- Secondary, unverified leads only: https://tkmxai.it.com/prototyping-llm-features-without-a-credit-card, https://www.tkmxai.it.com/the-free-ai-api-tier-trap, https://dev.to/fithappensml/anthropics-free-credit-window-closes-april-17-dont-miss-it-417i, https://guptadeepak.com/anthropic-claude-for-startups-the-complete-guide-to-credits-tiers-and-eligibility-2026/
- ai.google.dev was not attempted (blocked per instructions).
