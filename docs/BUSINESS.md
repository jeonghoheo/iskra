# Iskra Business Strategy

Status: **Outcome-first Headless Control Plane strategy approved; current implementation remains Stage 2.5-A `delivery/v1`; no paid product, final price, payment account, customer campaign, or external service has been launched**.
Date: 2026-11-24.

## 0. Current product and commercial strategy

Build Iskra toward a MausBot-like **free local Agent workbench plus paid Always-on Cloud and Team**, but differentiate it as a **model-neutral, Headless Agent Control Plane**. Headless means **any UI, not no UI**: customer software, other agents, CLI, Telegram, a thin web console, and generated interfaces are replaceable projections of one authorized Outcome/event model.

The public positioning hypothesis is:

> **The model-neutral, headless control plane for agent work.** Run different agents, models and tools behind one durable API, policy boundary and audit trail.

The public customer contract is **Outcome → Task → Run → Event/Artifact/Evidence/Delivery**, not merely an Agent chat or a count of executed Tasks. An Outcome has an owner, desired result, baseline/target metric, budget, data/tool scope, approval policy, acceptance checks and required evidence. A completed Run never automatically means an achieved Outcome.

This section supersedes the earlier `$300 generic installation` offer and the rejected vertical-service-first draft. Limited design-partner support remains, but only to deliver the same managed Iskra product—not to become a different custom consultancy for every customer.

### Beachhead customer

The first paid-customer hypothesis is a one-to-ten-person software or AI product team, or a technical founder, that already uses multiple models, Agents or CLIs; has API/webhook/routine work that must continue while a laptop is closed; and needs one policy, cost, delivery and recovery boundary. They do not want to be locked into one model provider's hosted runtime or assemble a large framework stack merely to operate Agents.

Free Local adoption targets technical users who combine cost-efficient models such as GLM, DeepSeek or Qwen with existing coding agents and value a small self-hosted runtime and readable customer-owned data. Non-technical “business autopilot” buyers are not the initial market.

The customer pays for this operating result:

> “My software or another Agent can submit a measurable Outcome to Iskra, keep it running after my laptop closes, and rely on Core-owned permissions, approvals, evidence, delivery and recovery.”

### Offer structure

| Offer | Commercial role | Included value | Pricing boundary |
|---|---|---|---|
| **Free Local** | Adoption and proof | FSL single binary, local Outcome/event API, isolated Agent state, provider profiles, CLI/Telegram clients, export and no mandatory account | BYOK; no per-Agent fee; only implemented capabilities are public |
| **Managed Always-on** | Recurring revenue | Remote authenticated API, schedules/webhooks, queue resources, backup/restore, health, updates, approvals, delivery/usage evidence and termination export | Platform/operations fee plus actual resources; optionally add Verified Outcome pricing only when acceptance is objective |
| **Team** | Human collaboration | Seats, shared Agent/Procedure packages, private overrides, organization policy, approval roles and audit retention | Human seats plus resources after single-owner managed retention is proven |
| **Private Deployment / Support** | Higher-touch revenue | Self-hosted deployment, migration, security review, training and support | Scoped services without private Core forks or unimplemented Team claims |

Do not set final public prices by copying competitors. The product's first paid offer is a monthly **Managed Headless Workspace** design-partner beta, not a one-time installation fee. A realistic first paid-beta target is **8–16 weeks after product and security gates**, not an immediate revenue promise. The $49 **Agent Ops Audit** described under demand validation is a separate demand instrument that funds discovery; it is not the product, and it must be produced through the same Iskra path so that delivering it is also a product test. LOIs, waitlists and free usage are not revenue.

### Outcome and BYOUI as product differentiators

Vercel Board already provides a hosted, harness-agnostic message store with channels, threads, claims, leases, dependencies, watchers, schedules, structured data, grants, event logs and external sync. It is free while experimental. Iskra must not clone that primitive.

Iskra's intended differentiation is the combination of:

- **Outcome and evidence:** measurable customer results rather than message/Task counts.
- **Local and customer-selected data residency:** a single local binary, customer-owned readable state and no required Vercel/provider account.
- **Actual per-Agent authority:** independent workspaces, tools, budgets and approval policies rather than a display-only Agent profile.
- **BYOUI:** one canonical, scoped event projection rendered as official Web Ops, Telegram/Slack, CLI, Kanban or customer-generated UI.
- **Cross-provider evidence:** verified Outcome rate, total cost, latency and human intervention by model/engine/workflow.
- **No invisible replay:** delivery uncertainty, lease expiry and restart do not silently repeat side effects.

Vercel Board can become an optional communication adapter later; Iskra does not need to recreate every Board or Slack feature.

### Demand and GLM validation

Demand is validated by transactions, not by scheduled interviews. An owner who cannot hold intro calls runs an interview-free ladder instead: (1) collect dated public evidence of cross-vendor Agent operation pain from GitHub issues, Reddit/HN, Habr and provider forums; (2) publish one asynchronous, buyable **Agent Ops Audit** under a $49 price hypothesis with a ten-question intake form and a payment path that needs no call; (3) list it on at most two channels that require no existing audience, measuring views, intake starts and paid orders; (4) deliver the first paid audit through the same Iskra path the product will use, then offer the Managed Headless Workspace design-partner beta at a price set after measured cost.

Gate: one paid audit delivered within 48 hours and four owner hours with Iskra in the execution path, and one of the first three buyers accepting a Managed design-partner proposal. Fifteen interviews and three budget conversations are no longer a gate; written answers through an Iskra-run intake may replace them, and five completed responses count as the qualitative evidence. The audit is a demand instrument, not the product: three consecutive audits requiring bespoke work stop the offer and reclassify it as consultancy. Details and failure rules are in `SESSION.md` "Interview-free demand ladder".

Z.AI officially supports GLM Coding Plan integration with Claude Code, and Russian-language setup/payment guides exist. No representative evidence has been found that most Russian developers are migrating to GLM. Treat GLM as an early Russian-speaking developer-community wedge and the next provider-compatibility candidate, never as the whole market thesis or a hard-coded default.

### Current technical dependency

The approved Stage 2.5-A `delivery/v1` contract remains the current implementation target. The commercial and long-term architecture strategy does not replace it or start a parallel code Goal. Subsequent sequential cards cover provider compatibility, `outcome/v1`, `headless/v1`, scoped event projections, managed operation and only then bounded delegation/leases where required.

### Authority Hub and anti-absorption strategy

Iskra is not a hub merely because it connects to other products. It becomes one only when customers choose it as the canonical owner of **Identity, Authority, Outcome, Evidence and Delivery**. External Agents and models are replaceable capabilities; external UIs and Boards are replaceable projections. An adapter may translate transport and IDs but never decides admission, permission, acceptance, settlement or retry.

The revised competitive position is **the smallest Outcome-verified Authority Hub**, not the most feature-rich one:

- A single binary with measured resource budgets instead of a required framework/Docker/DB stack.
- Customer Outcomes with baseline/target, acceptance and required evidence, compared against AgentField decision receipts rather than assumed to be unique.
- BYOUI and customer-generated interfaces whose scopes and approval path remain enforced outside Core.
- No invisible replay on provider timeout, lease expiry, delivery `unknown` or restart, proven through failure injection.
- One Local → Managed path with working backup/restore/export/delete and measured total cost.
- GLM and other provider profiles as a distribution wedge, never the permanent product identity.

AgentField is the first required comparison: its public documentation includes Agents as APIs, identity, verifiable credentials, access policies, decision receipts, signed audit chain, multiple SDKs and `docker compose`. CodeAF adds managers, budgets, schedules and headless mode. OpenClaw already provides local multi-channel assistants, daemon and hosting choices. Iskra must not claim those categories as open space.

The required bake-off measures installation time, total RAM/CPU/support effort, Outcome acceptance, BYOUI scope enforcement, duplicate/replay behavior, provider swap, exit portability and managed recovery cost. If Iskra has no practical advantage over AgentField/CodeAF/OpenClaw, stop the generic Hub build or reposition it as a small adapter rather than a competing control plane.

| Platform | Allowed relationship | Authority boundary |
|---|---|---|
| Vercel Board | Optional task inlet/outlet and BYOUI channel | Board is never canonical policy or evidence; no full message mirroring |
| Anthropic Managed Agents | Limited external execution capability and artifacts | Iskra does not claim ownership of its hosted session/sandbox |
| OpenMausBot | External harness through ProviderAdapter/capability contracts | MausBot keeps its own turn/permission authority; no control claim |
| AgentField / CodeAF | Direct comparison and optional REST/SDK/HTTP capability | Never claim identity, policy, audit, budgets or headless as Iskra-only |
| OpenClaw / Hermes Agent | Limited external execution or channel integration | Local free and multi-channel are not differentiators; unverified Kanban/budget/hosting claims are excluded |
| LangGraph / Temporal | External worker/runtime via A2A/HTTP/CloudEvents | Iskra does not replace their runtimes or reimplement their frameworks |
| GLM / DeepSeek | Provider and evaluation profiles | No hard-coded default or provider-specific Core policy |

The defensibility assets are the open conformance kit, labelled Outcome evaluations, Procedure/failure corpus, evidence continuity, reference integrations, managed restore/delivery assurance and actual customer distribution. Exportability remains a feature; forced lock-in is not the moat.

## 1. Product thesis

Iskra should be the **lightweight, model-neutral, safety-first Outcome control plane for software teams that want several AI agents to complete verifiable work through any client or UI**.

The differentiator is not the largest model catalog, the most connected apps, or an always-full-access agent. It is the combination of:

- A small local-first core.
- Replaceable models and agent engines.
- Independent workspaces and explicit ownership.
- Core-controlled policy, approvals, memory, and audit.
- Measured cost and human effort per **Verified Outcome**, with Tasks and Runs subordinate to customer acceptance evidence.

The initial customer is the beachhead technical owner defined in §0. The later customer is a small team that needs shared, always-on, auditable agent capacity without giving every engine unrestricted host access.

## 2. What the reference projects demonstrate

### MausBot / OpenMausBot

Observed public structure:

- Free local application under Apache 2.0 outside the separate `enterprise/` tree.
- Bring-your-own AI account or API key; provider charges remain separate.
- Paid Cloud plans for always-on execution, cloud computers, concurrent work, storage, backups, and related allowances.
- Public Cloud prices at the time of review: Starter **$15/month**, Pro **$49/month**, Max **$99/month**.
- Separate enterprise/team offerings, partner and inference sponsorships, and GitHub Sponsors.
- A MausBox hardware waitlist.

Useful lesson: a free local product can drive adoption while always-on compute and operations create a clear reason to pay. The public open-core boundary also separates general-purpose features from organization/admin/tier features.

Do not copy blindly: the broad desktop, VM, voice, connected-app, and hardware surface would conflict with Iskra's current focus and resource discipline.

Sources:

- https://github.com/milind-soni/OpenMausBot
- https://github.com/milind-soni/OpenMausBot/blob/main/LICENSING.md
- https://mausbot.com/

### OpenBot

Observed public structure:

- Personal/noncommercial use is free under PolyForm Noncommercial 1.0.0.
- Company use requires a commercial license.
- Managed servers, team environments, custom integrations, setup, and continuing support are paid offerings.
- The public pricing page charges separately for hosting and business licenses, with Enterprise contact.
- The repository also contains account, billing, credits, marketplace, and hosted-server infrastructure.

Useful lesson: managed operation, team setup, integration, and support can coexist with a free personal product.

Do not copy blindly: OpenBot's noncommercial license does not fit copying its code into a commercial Iskra product. Iskra should also avoid the confusing possibility of charging separately for a mandatory business license and the server on which that same business use runs. OpenBot's documented full-access/no-per-action-approval mode is not an acceptable Iskra security model.

Sources:

- https://github.com/nightly-labs/openbot
- https://github.com/nightly-labs/openbot/blob/main/LICENSE
- https://openbot.run/
- https://openbot.run/pricing

### Headless and orchestration references

Observed public structures:

- **Vercel Board:** an experimental hosted communication primitive and message store for people and coding Agents. Channels/threads/tasks, claims and renewable leases, dependencies, watchers/schedules, structured data, grants, ordered event logs, Slack/GitHub/Linear sync, and sandbox policies are documented. It is free during the experiment. Public documentation also notes US-only storage, broad default team visibility, profiles without independent permissions, and permissions/sync spill risks.
- **Anthropic Managed Agents:** a hosted meta-harness separating session, harness and sandbox, with many brains/hands and credentials outside the sandbox.
- **LangGraph / LangSmith:** a low-level stateful Agent orchestration runtime plus observability/evaluation/deployment products.
- **Microsoft Agent Framework:** multi-provider Agents, harnesses, explicit workflows and integrations.
- **Temporal:** durable orchestration for AI applications and Agent control planes.

Useful lesson: Headless coordination, leases and durable state are becoming standard infrastructure, not a standalone moat. Iskra should integrate useful primitives where sensible and win on local/customer-owned deployment, actual per-Agent policy, Outcome evidence, provider neutrality and simple managed operation.

Sources:

- https://vercel.com/blog/introducing-vercel-board
- https://vercel.com/docs/board
- https://www.anthropic.com/engineering/managed-agents
- https://docs.langchain.com/oss/python/langgraph/overview
- https://learn.microsoft.com/en-us/agent-framework/overview/
- https://temporal.io/solutions/ai
- https://modelcontextprotocol.io/docs/getting-started/intro
- https://a2a-protocol.org/latest/
- https://docs.z.ai/devpack/tool/claude

## 3. Recommended offer structure

### Free Local

Include the complete local orchestration core:

- Multiple Agents and replaceable Engine profiles.
- Explicit routing, durable queues, independent sessions/workspaces, core policy, approvals, usage records, and exportable diagnostics.
- Versioned `outcome/v1` and `headless/v1` contracts, scoped event projections for BYOUI, and acceptance/evidence linked to customer-visible results.
- BYOK/BYO-agent with no hidden model markup.
- No mandatory account, cloud connection, telemetry, or paid license for internal business use under the existing FSL terms.

The objective is adoption, trust, and real efficiency data—not an artificially crippled local product.

### Paid Always-on

Charge for operating value rather than Agent count:

- An always-on host while the owner's laptop is closed.
- Included compute/storage allowances and transparent overage.
- Encrypted backups and restore points.
- Authenticated remote access from web/mobile companions.
- Queue/concurrency capacity that maps to actual infrastructure cost.
- Clear separation of model charges from Iskra hosting charges.

Do not set final prices from competitor anchors alone. First measure monthly compute time, CPU/RAM, storage, backup, network, and support cost per active workspace. Publish included units in terms users can understand and show current consumption before charging overage.

### Team

Charge for human collaboration and shared operations:

- A shared or private always-on server.
- Human seats, not per-Agent fees.
- Shared Agent/Team packages with private local overrides.
- Organization policy, approval roles, audit retention, and usage budgets.
- Workspace-level access boundaries and accountable Task ownership.

Unlimited or generous Agent creation aligns pricing with the product goal. Human seats and actual server resources map more cleanly to value and cost.

### Enterprise and services

Sell implementation and assurance rather than a fork:

- Deployment, migration, onboarding, training, and support.
- SSO/organization administration and retention requirements when implemented.
- Custom integrations through data or separately supervised processes.
- Security review support, operational runbooks, and service agreements.

Customer-specific configuration, prompts, packages, and connectors should live in customer-owned data/repositories where possible, not as private changes to the core.

### Marketplace — later

Start with a portable, credential-free Agent/Team/Procedure format:

- No credentials, private conversations, long-term memory, host permissions, or connection addresses in a package.
- Routines import paused; connections import disabled until approved.
- Show every requested engine, permission, workspace, routine, and connector before installation.
- Keep packages readable and usable without a marketplace account.

Add paid curation, publisher revenue share, or organization-private libraries only after real sharing demand exists. Do not make the free local runtime dependent on a marketplace service.

## 4. Licensing and ecosystem strategy

Keep the existing Iskra core under **FSL-1.1-ALv2** for now. It permits internal use while restricting competing commercial products/services and converts each release to Apache 2.0 after two years.

Recommended ecosystem split:

- Core: current FSL.
- New portable Agent/Team/Procedure specifications and extension SDK: consider Apache 2.0 to encourage interoperable third-party packages.
- Paid hosted operations and organization services: separate from the local runtime's authority boundaries.

Any actual license file or grant change is a separate explicit decision. Third-party code retains its own license. OpenMausBot ports require Apache/NOTICE compliance; OpenBot code, text, tests, and assets are not copied.

Partner or sponsor relationships are optional. They must not create a fixed default provider, silent telemetry, undisclosed data sharing, or paid placement disguised as the best model for a Task.

## 5. Better differentiation

Iskra should not try to match every reference feature. It should win on a measurable workflow:

```text
Define a useful Task
 -> choose/create the right Agent
 -> run under a bounded policy and budget
 -> observe/approve only when needed
 -> verify the artifact
 -> measure success, time, cost, and intervention
 -> turn repeated success into a reusable procedure
```

Recommended public proof points, after verification:

- Successful Tasks per dollar and per hour.
- Median and p95 completion time.
- Retry and tool-error rates.
- Human interventions per successful Task.
- Percentage of Tasks completed without manual repair.
- Core binary size, idle memory, and startup time.
- Routing/model comparisons on the same labelled Task set.

These measures are more aligned with the goal than downloads, stars, number of built-in integrations, or number of Agents created.

## 6. Go-to-market sequence

### Phase A — demand and wedge evidence, Day 1–14

- Collect at least eight dated public records of cross-vendor Agent operation pain, including at least three asking for a layer that owns state, authority or evidence across agents. Written evidence replaces scheduled interviews.
- Record current providers, spend, switching reasons, laptop-off work, API clients, delivery/recovery failures and existing alternatives from those records and from asynchronous intake responses.
- Ask who owns Task, permissions, outcome acceptance and result evidence today; why direct AgentField/CodeAF/OpenClaw/Vercel use is insufficient; and whether Iskra must sit in the execution path for a specific recurring Outcome.
- Publish one buyable asynchronous Agent Ops Audit, list it on at most two channels that need no existing audience, and measure views, intake starts and paid orders.
- Design three synthetic demonstrations: backend/CI → Outcome API → evidence/result; GLM versus another provider on the same labelled Outcome; scheduled/webhook work → approval → delivery/audit.

Gate: at least one paid audit delivered within four owner hours with Iskra in the path, and one of the first three buyers accepting a Managed design-partner proposal. GLM setup questions alone validate content demand, not Control Plane demand; zero orders at meaningful listing views changes the problem, offer or price rather than the feature set.

### Phase B — reliability and Headless Local v0, 0–3 months

- Implement the approved Stage 2.5-A `delivery/v1` first.
- Add provider profiles and reproducible GLM/DeepSeek comparison gates with unknown cost/balance shown honestly.
- Define `outcome/v1`: desired result, metric, acceptance, evidence, owner, budget and scope.
- Add `headless/v1`: local-only authenticated Outcome API, idempotency, strict schemas, bounded requests and cursor-based event projections.
- Prove that two different clients render the same authorized Outcome state without automatic restart replay.

Gate: a new user defines an acceptance-bearing Outcome and obtains evidence within 15 minutes; existing tests/process checks, release/resource budgets and real provider checks pass.

### Phase C — MausBot-like Local Workbench, 2–6 months

- Add one validated ACP coding-engine path, then only design-partner-required schedules/webhooks, MCP tools and Approval Broker slices.
- Let an Intake Agent turn conversation into an Outcome Contract, a shadow Run and an approved Routine/Procedure; unsupported capabilities return an integration need or refusal.
- Add verification, artifacts, evidence and reusable Procedures before a broad connector catalog.
- Keep an official thin Outcome Board as one projection while allowing customer-generated read-only/sandboxed interfaces.

Gate: real users repeatedly achieve Outcomes with less human intervention; external engines cannot bypass Core authority, and generated UIs cannot write stores directly.

### Phase D — paid Managed Headless Workspace, 3–9 months

- Sell only implemented and verified capabilities: a single-tenant always-on workspace, validated providers, one remote client integration, one recurring Outcome, backup/restore, delivery/usage evidence and termination export.
- Start with a manually provisioned monthly beta for at most three design partners sharing the same Core and operations path. No private forks or one-time installation-only offer.
- Use GLM/Russian-language technical content for adoption and Headless quickstarts/provider evaluations for English-speaking acquisition; attach paid conversion to always-on operation, policy and recovery.
- Measure total model, external-engine, infrastructure, support and payment cost before final pricing.

Gate: three partners actually pay, at least two remain active after four weeks, one recurring Outcome runs without manual repair for an agreed period, restore/export/deletion works, and target gross margin is at least 70% after stabilization. Twenty qualified contacts plus three concrete paid proposals with no purchase stops generic managed-cloud expansion.

### Phase E — Headless Agent Control Plane, 6–18 months

- Add scoped BYOUI projections, typed event schemas, Outcome/Task/Run dependency graphs and bounded delegation.
- Introduce leases only when multi-dispatcher/Agent claims require them. Expiry produces an explicit interrupted/unknown state, never automatic side-effect replay.
- Add A2A and external credential brokering only behind separate compatibility and security gates.
- Expand delegation only when it beats single-Agent/deterministic baselines on the same Outcome evaluation.

Gate: customer work genuinely requires dynamic multi-Agent coordination and its measured outcome/cost/latency improves without weakening policy.

### Phase F — Team and ecosystem after recurring revenue

- Add remote thin Web Ops, human seats, shared packages, organization policy and audit retention.
- Add self-serve accounts/billing only after repeatable managed operation and margin are demonstrated.
- Consider credential-free SDK/package licensing, marketplace, desktop companion, voice and cloud computers separately; none is a launch prerequisite.

## 7. Metrics and stop rules

Track locally and, for any future hosted service, with explicit consent:

### Outcome efficiency

- **Verified Outcome Rate:** Outcomes passing acceptance evidence / accepted Outcomes.
- **Cost per Verified Outcome:** model + external engine + infrastructure + human intervention + support.
- **Time to Verified Outcome:** p50/p95 from admission to evidence confirmation.
- **Unassisted Outcome Rate:** Outcomes achieved without human repair.
- Retry, cancellation, lease expiry, tool-error, approval latency and delivery-unknown rates.

### Product value

- **Repeat Outcome Rate:** repeated achievement of the same workflow by a retained customer.
- Four-week retained active owners and verified recurring Outcomes per owner.
- Package/Procedure reuse and successful import rate.
- Outcome Delivery Assurance: achieved results whose delivery is confirmed at the client.

### Hosted economics

- Cost per active workspace and per included compute unit.
- Gross margin after compute, storage, network, support, and payment costs.
- Backup/restore success and recovery time.
- Support time per paid workspace.

Stop or narrow expansion when:

- Multi-Agent coordination costs more than it saves on the same Tasks.
- External engines require bypassing core policy to be useful.
- Hosted usage grows but unit economics remain negative after price/allowance correction.
- AgentField/CodeAF/OpenClaw already wins the customer's installation, Outcome, BYOUI, recovery or total-cost requirements without a practical Iskra advantage; reposition as an adapter instead of a generic Hub.
- Users create Agents but do not retain or reuse successful workflows.
- Three consecutive Agent Ops Audits require bespoke analysis instead of the same Iskra path; reclassify the offer as consultancy or narrow it to a small adapter.
- A new surface would violate the local binary, privacy, security, or dependency budgets without a separately approved reason.

## 8. Explicitly avoided strategies

- Charging per Agent or making the desired multi-Agent workflow artificially expensive.
- Mandatory commercial licensing layered on top of hosting for the same ordinary internal use.
- Hidden inference markup, silent provider substitution, or selling user data.
- Default telemetry or a required account for local use.
- Competing first on hardware, hundreds of connectors, voice, or remote desktop.
- Full-access external agents after one blanket consent.
- Publishing revenue forecasts as facts, or final self-serve prices without actual cost and retention evidence.
- Selling a generic installation, cloning Vercel Board's message primitive, treating BYOUI as “no UI”, or making GLM the permanent product identity.
- Counting Tasks, tokens, Agent activity or PRs as customer success without acceptance evidence and an achieved Outcome.
