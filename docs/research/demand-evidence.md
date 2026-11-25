# G1 데스크 수요 증거: cross-vendor Agent 운영

- 조사일: 2026-11-24
- 저장소 HEAD: `34b8e58f4efe298f7514ddc7f6c8570905391f5f`
- 기준: `SESSION.md` “Interview-free demand ladder”, `docs/BUSINESS.md` §0/§6 Phase A
- 범위: 공개 글 조사만. 외부 게시·연락·계정 생성·유료 API·코드 변경 없음.

## 판정

**G1 통과 — 채택 17건, 그중 에이전트를 묶는 층을 명시적으로 요청한 사례 6건.**

- 고통 증거 합격선: 8건 이상 → **17건 채택** (`E01`–`E15`, `E17`–`E18`).
- 요청층 합격선: 3건 이상 → **6건 채택** (`E04`, `E06`, `E07`, `E15`, `E17`, `E18`).
- 반복되는 고통은 상태 drift·재시도 덮어쓰기·재시작 중복 실행/전달·권한 우회·실행/전달/감사 증거 부재다. 토큰 가격이나 GLM 설정만을 근거로 한 판정이 아니다.
- `E16`은 G2에 유용하지만 원문 날짜를 독립 확인하지 못해 **집계에서 제외**한다. CodeAF `S01`–`S02`도 실제 권한 결함이지만 다중 Agent/Provider 조건이 불명확해 **집계에서 제외**한다.

이 결과는 공개된 운영 고통이 존재한다는 뜻이다. 특정 작성자의 결제 의사, Iskra의 차별화, Managed 월정액 수요를 증명하지는 않는다.

## 채택 기준과 증거 등급

채택 조건은 다음과 같다.

1. 여러 Agent, 여러 Provider 또는 여러 Agent runtime/tool을 운영하는 맥락이 원문에 있다.
2. 상태·권한·증거·중복 실행/전달·복구·독립 검수 중 하나 이상이 깨지거나 그 결핍을 명시한다.
3. 공개 원문 URL, 날짜, 직접 인용이 있다.

요청층 `L`은 단순히 여러 Agent를 나열하는 정도가 아니라, **공유 상태·권한·정책·증거·조정을 위/밖의 공통 층에 맡기자는 요청**이 원문에 있을 때만 붙인다. 직접 제품을 만든 사례는 수요 신호로 채택하되 `L` 집계에는 넣지 않았다.

증거 등급:

- **A — 운영/장애 기록:** 작성자가 실제 설정, 로그, 재현 또는 운영 결과를 제시한다.
- **B — 빌더/사례 기록:** 고통을 해결하려고 orchestration을 만들거나 적용한 기록이다. 자기제품 홍보 편향이 있다.
- **C — 기능 요청/기술 분석:** 실제 장애가 아니라 결핍이나 설계 요구를 보여준다. 지불 의사 증거로 쓰지 않는다.

## 채택 증거

### E01 — r/LocalLLaMA: retries overwrite shared state and handoffs break

- 날짜·출처·등급: 2026-01-12, [Reddit 원문](https://www.reddit.com/r/LocalLLaMA/comments/1qasvdm/i_kept_breaking_multiagent_systems_on_shared/), **A/B**
- 대상: 프로세스·서비스로 분리된 다중 Agent, LangGraph 외부 공유 상태.
- 원문:

> Once agents are split across processes or services, shared state becomes fragile. Retries overwrite things, handoffs break, and debugging turns into digging through logs.

> It is a shared memory layer for agents, backed by Redis, with schema validation and pub sub so agents can react when state changes. It does not replace LangGraph state. It lives outside the orchestrator and is meant for cases where multiple agents or workflows need to share state safely.

- 판정: 상태·재시도·handoff 고통으로 채택. 직접 공유 메모리 층을 만들었으나 제품 기능 요청은 아니므로 `L` 집계 제외.
- 날짜 확인: 원문은 상대 시간만 표시되어 [공개 아카이브의 `created_utc=1768218651`](https://api.pullpush.io/reddit/search/submission/?ids=1qasvdm)을 UTC `2026-01-12T11:50:51Z`로 대조했다.

### E02 — r/ClaudeAI: multi-engine state, audit and approval gaps

- 날짜·출처·등급: 2026-03-04, [Reddit 원문](https://www.reddit.com/r/ClaudeAI/comments/1rl11b4/i_built_an_opensource_orchestrator_for_running/), **A/B**
- 대상: 같은 코드베이스의 여러 Claude Code 인스턴스, Codex, OpenCode.
- 원문:

> I've been running multiple Claude Code instances on the same codebase for a while and kept hitting the same problems: repeating the same setup over and over for each agent session, context windows degrading silently, burning out from constantly context switching between agent terminals.

> Agent Teams stores state in ephemeral file-based task lists. Stoneforge event-sources everything to SQLite + JSONL, so state survives restarts with a full audit trail.

> Running multiple agents burns through tokens fast. There are no approval gates by default. Agents read, write, and push code without asking.

- 판정: 상태·감사·승인 권한 결핍으로 채택. 직접 coordination layer를 만들었으므로 수요 신호지만 `L` 집계 제외.
- 날짜 확인: [공개 아카이브의 `created_utc=1772666331`](https://api.pullpush.io/reddit/search/submission/?ids=1rl11b4)을 UTC `2026-03-04T23:18:51Z`로 대조했다.

### E03 — AgentField #316: agents and LLM stuck, no resume path or health ownership

- 날짜·출처·등급: 2026-03-30, [GitHub issue](https://github.com/Agent-Field/agentfield/issues/316), **A**
- 대상: 여러 SWE-AF Agent, LiteLLM, AgentField, 여러 작업 동시 제출.
- 원문:

> I restarted agentfield, and still, they show up as running and nothing were really getting things done. I had to delete the request but now the question is, how do I resume that work?

> I think a queue system, with a max execution in concurrent is needed, and some way to understand what is the problem is it the agent, the llm, maybe have a agent health system, and an llm health system. Even the agent node it not consistently showing up as up/down properly.

- 판정: 실행 상태·복구·Agent/LLM 장애 분리·queue 필요로 채택. queue와 health를 요청하지만 공통 상태/권한/증거 소유층을 직접 지정하지 않아 `L` 집계 제외.

### E04 — AgentField #988: control plane should keep shared execution context

- 날짜·출처·등급: 2026-08-27, [GitHub issue](https://github.com/Agent-Field/agentfield/issues/988), **C, L**
- 대상: 서로 다른 reasoner·skill·Agent, 외부 Claude/OpenAI 서비스.
- 원문:

> Instead of adding a new field to the pydantic model for reasoner and constantly passing down the same data over and over (say, `user_id`, `profile_url`), it would be great if any type of data would be kept by the control plane and be accessed by any reasoner, skill or different agents without doing manual prop drilling.

> Sometimes you want to give information that won't be useful to AF but is when forwarding to another service (like Claude or OpenAI). Those extra fields can make or break a complex workflow

- 판정: **control plane이 execution context를 보유하고 서로 다른 Agent가 접근하는 명시적 상태층 요청.** `L` 채택. 실제 장애 로그는 없으므로 운영 장애 증거로는 쓰지 않는다.

### E05 — AgentField #374: re-registration clears revocation across all call paths

- 날짜·출처·등급: 2026-04-09, [GitHub issue](https://github.com/Agent-Field/agentfield/issues/374), **A**
- 대상: serverless Agent 재등록, admin revocation, execute·reasoner·skill·DID 경로.
- 원문:

> A serverless agent whose tags have been **administratively revoked** (lifecycle moved to `pending_approval`, approved tags cleared) can clear its own revocation simply by calling `POST /api/v1/nodes/register-serverless` again. After re-discovery the agent is written back with `LifecycleStatus = ready` and its admin-revoked state is lost.

> once re-registration clears revocation, *all* call paths — execute, reasoner, skill, DID — will permit calls again until the admin manually revokes a second time.

- 판정: 등록 상태와 실행 권한이 어긋나 권한 철회가 물력화되는 사례로 채택. 작성자가 프로젝트 member인 코드 감사이므로 외부 고객 수요로 해석하지 않는다.

### E06 — OpenClaw #20033: per-agent OS authority is requested

- 날짜·출처·등급: 2026-02-18, [GitHub issue](https://github.com/openclaw/openclaw/issues/20033), **A, L**
- 대상: `main`, `assistant-a`, `assistant-b` 다중 Agent, 공유 gateway, 분리된 workspace와 credential.
- 원문:

> In a multi-agent setup, all agents currently run exec commands as the same OS user (the gateway process owner). This means filesystem isolation between agents is only enforced by convention (SOUL.md instructions), not by actual OS-level permissions.

> Other agents restricted to their own workspaces only
> - OS-level enforcement so a misbehaving agent cannot read another agent files

> This provides real OS-level sandboxing between agents.

- 판정: **gateway가 Agent별 `runAs`를 적용해 실제 OS 권한을 강제하는 명시적 권한층 요청.** `L` 채택.

### E07 — OpenClaw #40245: copied state silently drifts; one shared source is requested

- 날짜·출처·등급: 2026-03-08, [GitHub issue](https://github.com/openclaw/openclaw/issues/40245), **A, L**
- 대상: 5개 Agent, 각각의 workspace, 5개 공유 JSON map.
- 원문:

> **Silent drift** — files get updated in one workspace but not others
> - **Sync overhead** — every edit to a shared file requires copying to N workspaces + verification
> - **No single source of truth** — each agent may be reading a different version of the same file

> A `sharedDir` or `commonFiles` config option that lets multiple agents reference the same physical directory for specific files, without triggering the symlink escape guard.

- 판정: **Agent들이 같은 물리 상태를 참조하는 명시적 단일 원본 요청.** `L` 채택. 안전한 공유와 workspace 권한 경계를 함께 설계해야 한다.

### E08 — OpenClaw #74072: shared workspace reruns boot once per Agent on restart

- 날짜·출처·등급: 2026-04-29, [GitHub issue](https://github.com/openclaw/openclaw/issues/74072), **A**
- 대상: primary orchestrator와 여러 specialist Agent, 같은 workspace, Discord.
- 원문:

> When multiple agents share the same workspace directory (e.g. a primary orchestrator agent + several specialist agents all rooted at `~/Jarvis-V2`), `BOOT.md` fires once *per agent* on gateway startup. Each fire posts the same workspace-startup announcement to Discord, producing N copies of the same message every restart.

> `runBootChecklist` is invoked per-agent without tracking which workspaces have already been processed in this gateway lifecycle, so identical workspaces re-run the checklist N times.

- 판정: 재시작·공유 workspace·중복 실행/전달 사례로 채택. 제안은 특정 dedupe set이며 공통 소유층 요청은 아니므로 `L` 제외.

### E09 — OpenClaw #69160: second provider overwrites the first provider's state

- 날짜·출처·등급: 2026-04-20, [GitHub issue](https://github.com/openclaw/openclaw/issues/69160), **A**
- 대상: custom provider + Google Gemini CLI OAuth를 순차 설정.
- 원문:

> When onboarding multiple providers sequentially, the second provider's `configPatch` replaces `agents.defaults.models` entirely instead of merging with models from the first provider.

> Provider / routing chain
>
> openclaw -> custom provider (API key) + google-gemini-cli (OAuth)

- 판정: 다중 Provider 구성 상태가 마지막 Provider로 덮여 채택. 기능 수정 요청이지 별도 공통층 요청은 아니므로 `L` 제외.

### E10 — OpenClaw #31710: health restart duplicates dispatch across sub-agents

- 날짜·출처·등급: 2026-03-02, [GitHub issue](https://github.com/openclaw/openclaw/issues/31710), **A**
- 대상: main 1개 + sub-agent 2개, Discord 별도 bot token, Telegram DM에도 영향.
- 원문:

> Each restart creates a new event listener without cleaning up the previous one
> - Result: incoming messages appear duplicated in both sub-agents' view (same message shown twice with identical timestamps)
> - Affects both Telegram DMs and Discord (provider restart affects all channels for that agent)

> Gateway restart temporarily resolves duplicates but health-monitor triggers the issue again within minutes.

- 판정: 다중 Agent·채널에서 listener lifecycle와 중복 dispatch가 깨진 기록으로 채택. 공통층 요청은 아니므로 `L` 제외.

### E11 — ccusage #938: dedup keeps partial evidence and undercounts subagent output

- 날짜·출처·등급: 2026-04-12, [GitHub issue](https://github.com/ccusage/ccusage/issues/938), **A**
- 대상: 약 6,843개 Claude JSONL 파일, CLI/Desktop, `subagents/agent-*.jsonl`, ccusage와 agentsview 대조.
- 원문:

> ccusage's first-wins deduplication by `${messageId}:${requestId}` keeps the **first** streaming snapshot per Claude API response. For `output_tokens`, the first snapshot has a partial count (typically 1-5 tokens from the first streaming chunk), while the final snapshot has the complete count. This causes ccusage to under-report output tokens by approximately 2.7x on datasets with streaming progress snapshots.

> **Subagent files** (`subagents/agent-*.jsonl`) are still affected in recent data — they continue to show progressive `output_tokens` across snapshots.

- 판정: Agent 비용/사용량 증거가 dedup 선택에 따라 왜곡되는 사례로 채택. 단일 Provider 중심이므로 cross-vendor 일반화는 하지 않는다. 이슈는 현재 closed이며, 과거 결함 증거지 현재 버전 주장이 아니다.

### E12 — Hacker News: 3+ agents cause duplicated work and no review gate

- 날짜·출처·등급: 2026-03-19, [Hacker News](https://news.ycombinator.com/item?id=47441323), **B**
- 대상: Claude Code, OpenCode, Codex 또는 다른 coding Agent, 3개 이상 병렬 실행.
- 원문:

> Anyone running 3+ agents in parallel has hit the same wall: the agents are fine, the coordination is the problem. Merge conflicts. Duplicated work. No review gate. Token burn on orchestration overhead.

> Describe the work. It decomposes, spawns engineers in isolated git worktrees, reviews everything before merge, and delivers clean goal branches. Runtime is bash, tmux, and git. State is three files.

- 판정: 다중 engine의 중복 작업·검수·상태 문제로 채택. 직접 orchestrator를 만든 Show HN이며 특정 고객 장애 보고는 아니다. `L` 집계 제외.
- 날짜 확인: [HN 공개 검색 API](https://hn.algolia.com/api/v1/search?query=multi-agent%20orchestration&tags=story&hitsPerPage=20)의 `created_at=2026-03-19T15:41:24Z`.

### E13 — Habr: tasks, state and acceptance need one orchestrator

- 날짜·출처·등급: 2026-11-04 UTC, [Habr 원문](https://habr.com/ru/articles/1103656/), **B**
- 대상: Claude Code와 Codex, 6개 repository, 12일·약 40개 작업 패키지, 독립 reviewer.
- 원문:

> Память заканчивается вместе с сессией. Решения, договорённости и причины, почему сделано именно так, живут в чате. Новая сессия начинает с нуля и легко отменяет то, о чём договорились вчера.

> Все связи идут через оркестратор: сессии модулей между собой не договариваются, задания, состояние и приёмка проходят через него, поэтому картину всей программы держит одно место.

> Правило концепции звучит так: сообщение агента «готово» это заявление, а не факт.

- 판정: 세션 상태·작업·수락·독립 검증을 한 orchestrator에 모은 실제 프로젝트 사례로 채택. 직접 plugin을 만들었으므로 `L` 집계에는 넣지 않는다. 성공·생산성 수치는 독립 검증하지 않았다.
- 날짜 확인: 원문 HTML `datetime="2026-11-04T21:50:23.000Z"` (`datePublished` 현지 시각 2026-11-05 +03:00).

### E14 — Z.AI/ZCode plugin #152: multi-agent state grows without retention or cleanup

- 날짜·출처·등급: 2026-11-02, [GitHub issue](https://github.com/zai-org/zcode-plugins/issues/152), **A**
- 대상: ZCode 장기 실행·동적 workflow의 heavy multi-agent 사용, 프로젝트별 session/actor baseline.
- 원문:

> A single project's `.mimosa/hook-state/` accumulated **24 GB across ~1.82 million files in 33 days** (directory created 2026-09-30, measured 2026-11-02). That's roughly **0.73 GB / 55k files per day** for one working project.

> The 5 documented config options (engine, failure modes, status level, project hints) include **no retention/max-age setting**.
> 2. The CLI (26 subcommands as of 1.0.4) has **no state/cleanup/prune/gc command** — `history`/`ledger` are list/show only.

- 판정: Agent 상태 보존 정책과 운영 resource 상한 부재로 채택. 상태 lifecycle 요청이지 cross-Agent 공통 소유층 요청은 아니므로 `L` 제외.

### E15 — Z.AI feedback #157: role-based file authority across a swarm

- 날짜·출처·등급: 2026-07-19, [GitHub issue](https://github.com/zai-org/feedback/issues/157), **C, L**
- 대상: ZCode와 Kimi Work를 오가는 다중 Agent 작업, Rust 프로젝트의 파일별 역할.
- 원문:

> Currently, to get multi-agent collaboration for these heavy workloads, I have to step out of ZCode and use Kimi Work with their k3 max model. It is frustrating because I vastly prefer ZCode's overall platform, ecosystem, and UX, but I am forced to switch contexts just to utilize Kimi's "Swarm" feature.

> Assign specific roles and file-access permissions to individual agents.

> If an agent working on a file realizes it needs a new crate to complete its logic, it cannot edit Cargo.toml itself; instead, it sends an internal request to the Cargo.toml agent to add the dependency and resolve versioning. This mimics a real-world dev team and prevents file-writing conflicts.

- 판정: **swarm의 역할별 파일 권한과 agent 간 요청을 협업층에서 강제하는 명시적 권한층 요청.** `L` 채택. 실제 파일 충돌 로그는 없다.

### E16 — OKKY/Earendil: provider-sealed state blocks evidence and provider exit — 보강, 집계 제외

- 날짜·출처·등급: 검색 색인상 2026-07-30, [OKKY 한국어 게시물](https://okky.kr/articles/1561559), [영어 원문](https://earendil.com/posts/session-portability/), **C**
- 대상: OpenAI·Google·Anthropic, hosted Responses Multi-agent와 Codex subagent.
- 원문:

> This is a bundle of non-transferable state: sealed delegation, sealed agent messages, separate automatically compacted contexts, hidden reasoning, and provider-hosted orchestration.

> **The local event log is canonical.** Server storage may mirror or accelerate it, but the client can reconstruct the session without dereferencing server IDs.

> **Subagent communication is auditable.** Persist the exact readable task, messages, results, lineage, model, and tool permissions for every agent.

- 판정: 상태·증거 소유와 provider 교체에 직접 관련되지만, 원문 페이지의 날짜 메타데이터를 두 번 확인하지 못했고 검색 색인 날짜만 있으므로 **G1 건수와 `L` 건수에서 제외**한다. 기술 에세이이며 특정 고객 장애 보고가 아니다.
- 미검증: 실제 운영 조직의 장애·비용·구매 의사, OKKY 게시일.

### E17 — OpenAI Codex #17401: shared policy source across Codex and Claude Code

- 날짜·출처·등급: 2026-04-11, [GitHub issue](https://github.com/openai/codex/issues/17401), **C, L**
- 대상: 같은 프로젝트에서 Codex와 Claude Code를 쓰는 팀, 보안 정책과 팀별 instruction 소유.
- 원문:

> Teams using both tools on the same projects must maintain parallel, divergent instruction files. This undermines AGENTS.md's role as an open cross-tool standard under the Linux Foundation / AAIF.

> **Multi-tool teams**: single source of truth consumed by both Codex and Claude Code
> - **Enterprise**: centralized policy files referenced from any project's AGENTS.md

> No duplication, no drift. Same `@` syntax, same resolution semantics.

- 판정: **Codex·Claude Code가 같은 policy 원본을 공유하는 명시적 정책층 요청.** `L` 채택. 단, 실행 권한 enforcement나 Run 상태가 아니라 instruction/구성 권한 범주다.

### E18 — OpenClaw #52683: a room session above individual Agent sessions

- 날짜·출처·등급: 2026-03-23, [GitHub issue](https://github.com/openclaw/openclaw/issues/52683), **C, L**
- 대상: Telegram/Discord/Feishu project room의 PM·tech lead·QA·research·marketing Agent와 ACP session.
- 원문:

> Multiple agents do **not** feel like they are in the same room with the same project context
> - Agent-to-agent coordination is mostly hidden in isolated sessions
> - If you want a visible "team discussion" in the group, it becomes noisy and brittle

> Introduce a **room session** abstraction above individual agent sessions
> - Allow multiple agents to bind to the same room context with scoped shared memory

> Shared room memory that survives session restarts

- 판정: **개별 Agent session 위에 scoped task state·memory·조정을 두는 명시적 상태층 요청.** `L` 채택. 실제 장애 로그가 아닌 기능 요청이다.

## 조사 대상이지만 집계에서 제외한 증거

### S01 — CodeAF #2677: dropped permission remains active in running conversations

- 날짜·출처: 2026-11-16, [GitHub issue](https://github.com/Agent-Field/CodeAF/issues/2677)
- 원문:

> After you bank `always, this tool` for `write` and then drop that rule in `/permissions`, the running conversation keeps writing files without asking, for the rest of the session.

> After a drop, the very next tool decision in that conversation, and in any other conversation on the same engine, uses the rules as they now stand: a dropped `always` makes the next `write` ask again.

- 제외 이유: 실제 권한 상태 결함이고 재현 3/3이지만, 단일 CodeAF engine의 conversation들이며 다중 Agent/Provider 조건이 명확하지 않다. 작성자도 collaborator의 hand-QA다.
- G2 의미: 권한 UI의 표시와 실행 gate가 같은 원본을 즉시 사용하는지, 같은 engine의 다른 conversation에도 철회가 전파되는지 검사해야 한다.

### S02 — CodeAF #2813: denial is bypassed by changing command spelling

- 날짜·출처: 2026-11-18, [GitHub issue](https://github.com/Agent-Field/CodeAF/issues/2813)
- 원문:

> Right after that denial the model ran the same delete again as `rm -r /tmp/victim` (no `-f`). The gate does not grade that spelling critical, so no card was drawn and the command ran.

> The person's no has to hold for the act, not for one spelling of it.

- 제외 이유: 심각한 승인 우회지만 다중 Agent/Provider 운영 기록은 아니며 collaborator의 hand-test다.
- G2 의미: audit은 허용 여부만이 아니라 **같은 효과를 내는 명령 변형과 deny 후 재시도**까지 검사해야 한다.

## 조사 커버리지와 미검증 항목

| 대상 | 결과 |
|---|---|
| AgentField | `E03`–`E05` 채택. 외부 사용자 실행 기록과 control-plane 상태/권한 결함 확인. |
| CodeAF | `S01`–`S02`는 권한 증거로 보강하되 다중 Agent/Provider 조건이 없어 G1 집계 제외. |
| OpenClaw | `E06`–`E10`, `E18` 채택. 상태·권한·중복 실행·공유 세션 확인. |
| ccusage | `E11` 채택. subagent 사용량 증거의 dedup 왜곡 확인. |
| r/LocalLLaMA | `E01` 채택. 공유 상태·retry·handoff 확인. |
| r/ClaudeAI | `E02` 채택. multi-engine 상태·audit·승인 결핍 확인. |
| Hacker News | `E12` 채택. multi-engine 중복 작업·review gate 확인. |
| Habr | `E13` 채택. Claude/Codex 프로그램의 상태·수락·독립 검증 사례 확인. |
| Z.AI/GLM | `E14`–`E15`를 공식 GitHub 조직의 공개 issue에서 확인. 별도 공개 web forum의 추가 운영 사례는 **미검증**. |
| 국내 개발 커뮤니티 | OKKY의 `E16`은 확인했으나 날짜 검증 부족으로 제외. 국내 작성자의 직접적인 다중 Agent 장애 로그는 **미검증**. Clien의 다중 Agent 대시보드 제작 사례는 있었지만 장애 증거가 아니어서 채택하지 않음. |

추가로 다음은 이번 공개 증거만으로 알 수 없다.

- 작성자들의 실제 월 지출, Managed 예산, 결제 의사와 팀 규모는 **미검증**이다.
- GLM으로의 대규모 이동이나 러시아 개발자 대표성은 **미검증**이다. `E14`–`E15`는 GLM 생태계의 운영/기능 신호이지 이주 통계가 아니다.
- AgentField·CodeAF·OpenClaw의 현재 최신 버전에서 모든 결함이 재현되는지는 **미검증**이다. closed issue는 당시 기록이며 현재 미해결 주장으로 쓰지 않는다.
- Iskra가 기존 orchestrator보다 설치·Outcome·BYOUI·replay·provider swap·exit·recovery에서 우위인지는 **미검증**이다.
- 자주 나오는 orchestrator 제작 사례는 수요와 동시에 **기존 대안이 이미 많다는 반증**이다. “오케스트레이터가 없다”는 시장 가정은 기각해야 한다.

## G2 Agent Ops Audit 문구에 반영할 점

### 1. 첫 문장은 대시보드가 아니라 소유권과 재현 가능한 실패를 팔아야 한다

권장 문구:

> **여러 AI 에이전트와 프로바이더의 실행 상태·권한·결과 증거가 서로 다릅니까?** $49 Agent Ops Audit은 하나의 반복 워크플로와 비밀값을 제거한 실패 로그를 대상으로, 누가 Task·권한·승인·결과 증거를 소유하는지 지도화하고, 재시도·재시작·Provider 전환 때 중복 실행과 증거 누락을 재현하거나 재현 절차로 확인합니다.

“통합 대시보드”, “모든 Agent 연결”, “자동 운영”은 이번 증거가 바로 뒷받침하지 않는다.

### 2. 산출물은 실제 실행 증거와 재실행 가능한 확인 절차여야 한다

오퍼에 다음을 명시한다.

- Agent/model/engine/workflow별 **Task·permission·approval·acceptance·evidence·delivery 소유자 지도**.
- retry overwrite, restart replay, provider config overwrite, failover duplicate send, stuck run/resume, stale revocation의 **재현 결과 또는 한계가 표시된 재현 노트**.
- Run ID·attempt·event·artifact·channel message ID·delivery 상태를 잇는 **evidence index**.
- executing Agent의 “완료” 주장과 분리된 **acceptance check 3개와 rerun 가능한 스크립트**.
- provider 교체 후에도 유지해야 할 **인계용 상태와 acceptance evidence 목록**.

### 3. 비용은 list price가 아니라 실제 Run과 검증된 Outcome으로 계산한다

`E11` 때문에 token 합계도 그대로 믿지 말아야 한다. 오퍼에는 다음을 넣는다.

- raw JSONL/event log와 집계 결과를 대조하고, streaming snapshot은 first가 아니라 final을 사용하는지 확인한다.
- 모델·외부 engine·인프라·사람 개입 시간을 분리한다.
- “cost per verified Outcome”은 acceptance를 통과한 Run만 분모로 하며, 증거 불완전 시 수치 대신 **미검증**으로 남긴다.

### 4. 권한 audit은 단일 happy path가 아니라 철회와 우회를 검사한다

`E05`, `E06`, `S01`, `S02`에서 직접 나온 검사 항목이다.

- Agent별 workspace/credential/tool scope가 prompt가 아니라 실제 gate에서 강제되는가.
- revocation/drop이 다음 tool call과 같은 engine의 다른 conversation에 즉시 반영되는가.
- deny 후 동일 효과의 명령 변형을 다시 실행하는가.
- shared directory가 single source of truth를 제공하면서도 workspace 밖 읽기·쓰기를 허용하지 않는가.

### 5. Provider 이동성은 암호화된 상태가 아니라 읽을 수 있는 인계 증거로 설명한다

`E16`은 집계에서 제외했지만 G2 질문에는 유효하다. intake와 산출물에 다음을 넣는다.

- subagent의 정확한 지시·메시지·결과·계보·모델·도구 권한을 읽을 수 있는가.
- server response ID나 provider-only encrypted blob 없이 세션을 재구성할 수 있는가.
- hosted search의 실제 입력·출력·근거·시간·필터링이 남는가.
- provider를 제거해도 Task·acceptance·evidence·delivery가 남는가.

### 6. 가격과 범위는 작게 고정하고, Managed는 별도 검증한다

- 하나의 반복 워크플로, 기존 Agent/Provider inventory, 비밀값 제거 로그로 제한한다.
- 넓은 connector 개발, 전사 migration, 월정액 Managed, 24/7 운영은 $49 audit에 포함하지 않는다.
- audit 후 제안은 “기존 orchestrator 교체”가 아니라, 반복 Outcome 하나에 Iskra가 실제로 필요한지와 기존 AgentField/CodeAF/OpenClaw 대비 recovery/exit/acceptance 우위가 있는지로 한정한다.
- 공개 고통이 확인됐으므로 G2에서 검증할 것은 문제 존재의 반복 설명이 아니라 **낯선 사람이 진단 결과와 재현 스크립트에 실제로 돈을 내는지**다.
