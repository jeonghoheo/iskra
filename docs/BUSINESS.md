# Iskra Business Strategy

Status: **recommended strategy; no paid product, price, account, or external service has been launched**.
Date: 2026-11-23.

## 1. Product thesis

Iskra should be the **lightweight, model-neutral, safety-first workbench for people who want several AI agents to finish real work efficiently**.

The differentiator is not the largest model catalog, the most connected apps, or an always-full-access agent. It is the combination of:

- A small local-first core.
- Replaceable models and agent engines.
- Independent workspaces and explicit ownership.
- Core-controlled policy, approvals, memory, and audit.
- Measured cost and human effort per successfully completed Task.

The initial customer is a solo technical user who already has one or more AI subscriptions or API keys. The later customer is a small team that needs shared, always-on, auditable agent capacity without giving every engine unrestricted host access.

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

## 3. Recommended offer structure

### Free Local

Include the complete local orchestration core:

- Multiple Agents and replaceable Engine profiles.
- Explicit routing, durable queues, independent sessions/workspaces, core policy, approvals, usage records, and exportable diagnostics.
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

### Phase A — prove the local workbench

- Finish `session/v1` live verification.
- Ship the compatibility runtime and two-Agent explicit-routing slice.
- Publish reproducible coding/file-workflow evaluations using fictional or synthetic data.
- Document exact model/provider and resource limits rather than claiming universal efficiency.

Gate: repeated real use, successful isolated Tasks, no state/permission cross-contamination, and known cost/latency measurements.

### Phase B — reusable work and external engines

- Add one securely mediated ACP engine path.
- Add portable Agent/Team packages and bounded, explicit delegation if evaluations justify them.
- Publish a small set of high-quality role packages for concrete outcomes rather than a large unreviewed catalog.

Gate: users reuse packages and complete recurring Tasks with less intervention; external engines cannot bypass core authority.

### Phase C — paid always-on pilot

- Offer a limited, explicitly approved managed pilot only after unit economics are measured.
- Test one simple hosting offer before adding multiple tiers.
- Keep BYOK and local export available so cloud use does not become lock-in.

Gate: owners pay for reliable always-on operation, infrastructure and support costs are understood, and cancellation/export/restore work.

### Phase D — team and organization services

- Add human collaboration, organization policy, and support only after the single-owner foundation is stable.
- Price larger custom deployments and support through direct consultation.

## 7. Metrics and stop rules

Track locally and, for any future hosted service, with explicit consent:

### Product efficiency

- Verified successful Tasks per active week.
- Cost and wall time per successful Task.
- Retry, cancellation, tool-error, and manual-repair rates.
- Queue wait and approval latency.

### Product value

- Four-week retained active owners.
- Recurring Tasks per retained owner.
- Package reuse and successful import rate.
- Percentage of new users who complete one verified useful Task.

### Hosted economics

- Cost per active workspace and per included compute unit.
- Gross margin after compute, storage, network, support, and payment costs.
- Backup/restore success and recovery time.
- Support time per paid workspace.

Stop or narrow expansion when:

- Multi-Agent coordination costs more than it saves on the same Tasks.
- External engines require bypassing core policy to be useful.
- Hosted usage grows but unit economics remain negative after price/allowance correction.
- Users create Agents but do not retain or reuse successful workflows.
- A new surface would violate the local binary, privacy, security, or dependency budgets without a separately approved reason.

## 8. Explicitly avoided strategies

- Charging per Agent or making the desired multi-Agent workflow artificially expensive.
- Mandatory commercial licensing layered on top of hosting for the same ordinary internal use.
- Hidden inference markup, silent provider substitution, or selling user data.
- Default telemetry or a required account for local use.
- Competing first on hardware, hundreds of connectors, voice, or remote desktop.
- Full-access external agents after one blanket consent.
- Publishing projected revenue or prices without actual cost and retention evidence.
