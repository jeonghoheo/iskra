# Agent Replay & Delivery Audit — asynchronous intake

Use this file to create a form in the owner's approved form tool. Keep it asynchronous and do not enable file uploads. The intake qualifies one incident; it is not a checkout and does not authorize work.

## Form introduction

> Describe **one** restart duplicate-execution or duplicate-delivery incident. The fixed-scope audit produces one state/authority map, one minimal reproduction fixture, one deterministic check, and a short rerunnable evidence bundle. The current validation price hypothesis is **$49 for the first three qualifying incidents**. Scope and payment instructions are confirmed separately; do not pay before confirmation. Target delivery is within 48 hours after usable sanitized evidence and payment.
>
> **Do not submit API keys, tokens, passwords, cookies, private credentials, proprietary source dumps, or unnecessary personal data. Do not attach files.** Only synthetic or redacted information is allowed.

## Questions

### 1. Where should the scope decision and audit be delivered?

- Type: email
- Required: yes
- Help text: use an address you can access; no newsletter subscription is included.

### 2. What runtime, operating system, and versions were involved?

- Type: short answer
- Required: yes
- Example: `Herdr 0.9.5 + OpenCode 2.2.6, macOS 26.3.1, local machine`

### 3. Which agents, models, or CLIs were running, and how many were active?

- Type: paragraph
- Required: yes
- Help text: include the number of active agents/panes/workers and whether they ran unattended, scheduled, or repeatedly.

### 4. Which **one** incident should be investigated?

- Type: single choice
- Required: yes
- Choices:
  - Restart duplicate execution
  - Restart duplicate delivery/send
  - Ambiguous delivery/recovery state without confirmed duplication
  - Another incident (understand that it may be outside the fixed scope)

### 5. What did you expect, and what actually happened?

- Type: paragraph
- Required: yes
- Help text: include observed counts where possible, such as expected one message and received two.

### 6. What happened immediately before the incident?

- Type: paragraph
- Required: yes
- Help text: list the restart, crash, reconnect, update, health check, or retry sequence in order, with approximate timestamps if available.

### 7. What external side effects occurred, and how many times?

- Type: paragraph
- Required: yes
- Examples: files written, Git commits, messages, emails, tickets, payments, API calls. State `none visible` if applicable and distinguish confirmed effects from suspected effects.

### 8. What sanitized evidence and identifiers are available?

- Type: paragraph
- Required: yes
- Help text: list available Run/Session/Delivery IDs, agent session IDs, channel message IDs, logs, and relevant time ranges. Paste only short redacted excerpts; do not attach raw logs.

### 9. What is the smallest reproduction path you know?

- Type: paragraph
- Required: yes
- Help text: provide numbered steps, or write `not yet reproducible` and explain what is missing.

### 10. Do you accept the evidence-handling conditions?

- Type: required checkbox
- Label: `I confirm that my answers contain no secrets or unnecessary personal data. I will provide only synthetic or redacted evidence. I understand that handling and deletion timing must be agreed before evidence is exchanged, and that public case-study use requires separate permission.`
- Required: yes

## Confirmation screen

> Thank you. This is an intake, not a payment or an acceptance of work. If the incident fits the fixed scope, you will receive the exact deliverables, evidence-handling terms, and payment instructions. Do not send raw logs or make a payment before that confirmation.

## Owner routing rules

- **Reject or redirect:** multiple unrelated incidents, broad security/cost/provider audit, custom connector, ongoing consulting, unredacted secrets, or no usable incident description.
- **Needs clarification:** incident type is clear but counts, sequence, or identifiers are missing. Ask one consolidated asynchronous follow-up.
- **Potentially qualified:** one restart duplicate-execution or duplicate-delivery incident with a plausible timeline and enough sanitized evidence to build a fixture.
- **Do not infer a bug:** a report or suspected duplicate is not proof of duplicate execution. Keep `observed`, `reported`, `reproduced`, and `unknown` separate.
- **Payment:** send the approved payment path only after written fixed-scope confirmation. Record the order only after actual payment, not after form completion or proposal acceptance.
