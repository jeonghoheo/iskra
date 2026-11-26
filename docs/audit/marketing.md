# Agent Replay & Delivery Audit — publication kit

All external publication requires separate owner approval. Replace every uppercase URL placeholder before use. Use at most two acquisition channels in the first experiment.

## 1. Links and UTM

```text
Offer page:  {{OFFER_URL}}
Sample page: {{SAMPLE_URL}}
Intake form: {{INTAKE_URL}}

Herdr/community:
{{OFFER_URL}}?utm_source=herdr_community&utm_medium=community&utm_campaign=replay_audit_v1

Purchase channel:
{{OFFER_URL}}?utm_source=purchase_channel&utm_medium=listing&utm_campaign=replay_audit_v1
```

Use the actual channel name in `utm_source` when it is known. Do not claim view or intake numbers that the selected platform cannot measure.

## 2. Sixty-second demonstration script

1. **0–10 s:** Show the sample timeline. “The Run completed, but one answer chunk is still pending.”
2. **10–20 s:** Show the killed process and restart. “The Core does not know whether the pending chunk arrived.”
3. **20–30 s:** Show chunk `1` becoming `unknown`. “There is no automatic model replay and no original-Delivery retry.”
4. **30–40 s:** Run `/result 22`. “A person explicitly requests the stored answer. This creates a new Delivery, not silent proof that the old one arrived.”
5. **40–50 s:** Run `node docs/audit/replay-delivery/check.mjs`. Show `PASS`, eight checks, zero automatic effects.
6. **50–60 s:** Show the three Rust test commands and results. End with: “One incident, one reproduction, one deterministic check. Inspect the sample: {{SAMPLE_URL}}”

Record only the synthetic fixture. Never show real memory, `.env`, tokens, customer logs, or private Telegram chats.

## 3. Herdr/community post — English

**Title:** A checklist for the fourth kind of agent recovery: external effects

> Herdr distinguishes live terminals, restored layouts, screen history, and native agent-session restore. I found it useful to add a fourth question to my own restart checklist: **what happened to external effects and delivery while those states were changing?**
>
> ```text
> layout restored
>   ≠ original process survived
>   ≠ agent conversation resumed
>   ≠ external effect is safe to repeat
> ```
>
> I packaged one synthetic interrupted-delivery incident as a small audit: a state/authority map, a JSON fixture, eight deterministic checks, and three focused Rust regression tests.
>
> After restart, the original Delivery keeps one unresolved `unknown`; there is no automatic model call or original-Delivery retry. Only an explicit `/result` creates a separate Delivery with new message ids.
>
> Fixture, checker, commands, and claim boundaries: {{SAMPLE_URL}}
>
> This is not a claim of a Herdr bug or universal exactly-once behavior. It is a rerunnable example of the evidence I think agent recovery should leave behind. Feedback on the audit format is welcome.

Do not post this as an unrelated comment under an issue. Use a permitted showcase/discussion area and follow current community rules.

## 4. Public-issue contribution — no marketing CTA

Use only when the reproduction details are genuinely relevant. Do not attach the offer page.

> One distinction that may help the diagnosis is separating restored UI/session state from external-effect state. For each affected pane, can we record: pane id, agent session id, process start attempt, exit/signal, and whether the same worktree or session was already claimed? A reproduction that counts both restored writers and confirmed external writes would separate “layout/session restored” from “work executed twice.” The current report and maintainer reproduction should remain labelled separately until both observe the same sequence.

Do not state that the bug is confirmed, that a fix exists, or that Iskra reproduces it unless that has actually been demonstrated in the reported environment.

## 5. Fixed-scope purchase listing — English

**Title**

> I will audit one AI agent restart duplicate-execution or duplicate-delivery incident

**Short description**

> One incident. One reproduction fixture. One deterministic non-LLM check. A rerunnable evidence bundle within a 48-hour target after usable evidence and payment.

**Full description**

> Are you running Claude Code, Codex, OpenCode, Herdr, OpenClaw, or another coding agent unattended or repeatedly, and suspect that a restart caused duplicate work or duplicate sends?
>
> I will investigate **one** incident and deliver:
>
> - a state/authority map for Run, Session, tools, Delivery, and recovery;
> - one minimal sanitized reproduction fixture;
> - one deterministic non-LLM pass/fail check;
> - a short remediation recommendation and a rerunnable evidence bundle.
>
> **Fixed-scope validation price: $49 for the first three qualifying incidents.** The price and offer are unvalidated hypotheses, not a subscription.
>
> Please use the ten-question intake before ordering: {{INTAKE_URL}}. Do not send secrets, raw proprietary logs, or unnecessary personal data. Scope, evidence handling, and deletion timing are confirmed before payment.
>
> **Not included:** full security/cost/provider audits, multiple workflows, custom connectors, private forks, managed hosting, scheduled calls, or a guarantee of exactly-once behavior.
>
> Inspect the synthetic sample first: {{SAMPLE_URL}}

**Search tags**

```text
AI agent debugging, Claude Code, Codex, OpenCode, Herdr, agent restart, duplicate execution, duplicate delivery, durable execution, agent recovery
```

Use only tags supported by the marketplace and relevant to the actual offer.

## 6. Russian technical-summary draft — later channel

**Do not publish in the initial two-channel experiment unless it replaces one of them.**

> **Что происходит с внешними действиями агента после перезапуска?**
>
> Восстановление layout, процесса и диалога — разные вещи. И ни одно из них само по себе не доказывает, что повторная отправка или запись безопасна.
>
> Я подготовил синтетический пример аудита: карта состояний и полномочий, JSON fixture, 8 детерминированных проверок и 3 Rust regression-теста. После перезапуска исходная доставка сохраняет `unknown`; повторного вызова модели или отправки исходной доставки нет. Новая доставка создаётся только явной командой `/result`.
>
> Пример и границы утверждений: {{SAMPLE_URL}}
>
> Это не доказательство ошибки Herdr и не обещание exactly-once для любого агента.

## 7. Korean owner-share draft — later channel

**초기 두 채널 실험이 끝난 뒤 실제 사례와 함께 사용합니다.**

> **에이전트가 재시작 후 두 번 일했는지, 두 번 본냈는지 확인하는 법**
>
> 화면과 대화 세션이 복원돼도 외부 부작용까지 안전한 것은 아닙니다. Iskra의 중단 전송 사고를 가상 fixture로 만들어 8개 결정론적 검사와 3개 Rust 회귀 테스트로 확인했습니다.
>
> 재시작 후 원래 Delivery의 `unknown`은 그대로 두고 자동 모델 호출·자동 재전송은 0건입니다. 새 전송은 사람이 `/result`를 명시할 때만 새 Delivery로 기록됩니다.
>
> 표본과 재현 명령: {{SAMPLE_URL}}
>
> 모든 런타임의 안전성이나 Herdr 결함을 주장하는 자료는 아닙니다.

## 8. Metrics sheet

Create a local CSV outside the public repository when real data exists:

```csv
date,channel,asset,qualified_views,intake_starts,intake_completed,paid_orders,delivered_orders,refunds,owner_minutes,direct_cost_usd,repeat_product_request,notes
```

Definitions:

- `qualified_views`: unique visits to the offer/sample from the intended technical audience, measured by an approved method. Impressions, issue views, stars, and unknown duplicates are not substitutes.
- `intake_starts`: measured only if the form/redirect can observe a start. Otherwise use `unknown`, not an estimate.
- `paid_orders`: actual completed payments; proposals, checkout starts, and LOIs are excluded.
- `owner_minutes`: owner review, communication, manual analysis, and delivery time; agent execution time is separate.
- `direct_cost_usd`: model, infrastructure, marketplace/payment fees, and refunds as applicable.

## 9. Publication cadence

- Day 0: publish the sample and one offer in at most two channels.
- Days 1–3: respond to substantive questions without reposting or spamming.
- Days 4–7: publish one factual reproduction note only if it adds new evidence.
- Days 8–14: publish results only with owner approval and customer permission where relevant. Change one of incident, offer, price, or channel based on measured results—not the feature set.
