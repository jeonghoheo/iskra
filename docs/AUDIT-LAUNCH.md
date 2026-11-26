# Agent Replay & Delivery Audit — 소유자 즉시 실행서

이 문서는 **14일 동안 첫 유료 감사 1건을 확인하기 위한 실행 순서**다. 목표는 Herdr을 대체하는 제품을 만드는 것이 아니라, Herdr을 포함한 코딩 에이전트 사용자의 재시작 중복 실행·중복 전송 사고 한 건을 유료로 진단하는 것이다.

## 0. 현재 준비 상태

### 준비된 것

- 가상 중단 전송 사고 fixture와 상태·권한 지도
- Node.js zero-dependency 결정론적 검사기: **8/8 PASS**
- 실제 Iskra full Rust suite: **270/270 PASS** (집중 표본 테스트 3개 포함)
- 영어 판매 페이지와 표본 감사 리포트
- 10문항 비동기 intake와 분류 규칙
- Herdr 커뮤니티·구매 marketplace용 게시 초안
- 측정 항목, 중단 규칙, 고객 납품 절차

### 아직 하지 않은 것

- 외부 게시, 고객 연락, 실제 결제, 계정 생성, 배포, 커밋·푸시
- 실제 고객 사고나 실제 Herdr 환경 재현
- release build와 실제 Telegram/Herdr 라이브 검증
- `Outcome`, `Evidence`, `Headless`, `ACP`, Herdr plugin 구현

**따라서 지금 바로 할 수 있는 것은 로컬 검수와 판매 채널 설정이다. 외부 행동은 각 단계의 확인란을 직접 검토하고 승인한 뒤에만 진행한다.**

---

## 1. 준비물 위치

| 용도 | 파일 |
|---|---|
| 영어 판매 페이지 | `docs/audit/index.html` |
| 공개 표본 리포트 | `docs/audit/sample.html` |
| 표본 설명·재현 명령 | `docs/audit/replay-delivery/README.md` |
| 가상 사고 데이터 | `docs/audit/replay-delivery/fixture.json` |
| 결정론적 검사기 | `docs/audit/replay-delivery/check.mjs` |
| 10문항 intake | `docs/audit/intake.md` |
| 게시물·UTM·지표 | `docs/audit/marketing.md` |
| 이 실행서 | `docs/AUDIT-LAUNCH.md` |

---

## 2. 오늘 바로 실행하는 순서

### Step 1 — 로컬 검증: 예상 20분

저장소 루트에서 실행한다.

```bash
cd /Users/heojeongho/Desktop/projects/iskra
git status --short --branch

node docs/audit/replay-delivery/check.mjs

cargo test
```

확인:

- [ ] 검사기가 `state: PASS`, `passed: 8`, `failed: 0`을 출력한다.
- [ ] full Rust suite가 `270 passed, 0 failed`다.
- [ ] 작업 트리에 이미 있던 변경을 덮어쓰거나 되돌리지 않는다.

로컬 미리보기:

```bash
python3 -m http.server 8000 --directory docs
```

다른 터미널이나 브라우저에서 다음을 연다.

- `http://localhost:8000/audit/` — 판매 페이지
- `http://localhost:8000/audit/sample.html` — 표본 감사
- `http://localhost:8000/audit/replay-delivery/README.md` — 재현 설명

검토 후 미리보기 서버를 `Ctrl-C`로 종료한다.

### Step 1.5 — 표본 재납품 리허설: 최대 4 owner hours, 게시 전 필수

공개 판매 전에 새 Agent 세션에서 같은 표본을 다시 납품해 실제 owner 시간을 측정한다.

1. 시작·종료 시각을 기록한다.
2. 새 Agent 세션에 아래 지시를 준다.

```text
Read only docs/audit/replay-delivery/ and docs/audit/sample.html.
Reproduce the synthetic audit from the fixture without copying the existing verdict.
Create audit deliverables only inside memory/audits/synthetic-rehearsal/.
Run the Node checker and the three named Rust tests, then produce a fresh
state/authority map, deterministic result, unresolved-unknowns list, and exact
rerun commands. Separate automated work from owner review. Do not edit public
files, use external services, or claim live Telegram/Herdr verification.
Stop and report if the four-owner-hour rehearsal budget cannot be met.
```

3. owner는 생성물을 실제 명령으로 다시 확인하고 다음을 기록한다.

```text
memory/audits/synthetic-rehearsal/owner-time.md
```

기록 항목: 시작·종료 시각, owner 검토·수정 시간, Agent 실행 시간, 수동 수정 횟수, 자동 검사 결과, 미검증 항목.

- [ ] 동일한 4개 산출물(상태·권한 지도, fixture, deterministic check, evidence bundle)이 재현됐다.
- [ ] owner 총작업이 240분 이내다.
- [ ] 자동 작업과 사람 작업이 구분됐다.
- [ ] 실패·미검증 항목이 삭제되지 않고 남아 있다.

4시간을 넘기거나 재현이 실패하면 **외부 게시를 멈추고, 새 기능 대신 fixture와 절차를 더 단순화한다.**

### Step 2 — intake form 생성: 예상 20분

`docs/audit/intake.md`를 승인한 form 도구에 복사한다.

설정:

- [ ] 질문 10개만 사용한다.
- [ ] 파일 업로드는 끈다.
- [ ] raw log 대신 짧은 비밀값 제거 발췌만 받는다.
- [ ] 확인 화면에 “intake는 결제·작업 승인이 아니다”를 표시한다.
- [ ] 제출 알림은 owner만 받는다.
- [ ] form 시작 수를 측정할 수 없다면 수치를 추정하지 않고 `unknown`으로 기록한다.

생성한 공개 URL을 `INTAKE_URL`로 확정한다.

### Step 3 — 결제·정산 경로 확정: 예상 30분, 외부 조건에 따라 더 걸릴 수 있음

첫 실험은 다음 둘 중 **하나만** 선택한다.

| 경로 | 적합한 경우 | 게시 전 확인 |
|---|---|---|
| 구매 marketplace gig | 새 고객과 결제를 한 곳에서 얻고 싶을 때 | 판매자 등록, 국가/정산 가능, 수수료, 환불, 세금, 고객 자료 조건 |
| 직접 intake + checkout | 이미 사용 가능한 합법적 결제 경로가 있을 때 | 결제 링크, 통화, 수수료, 환불, 세금/VAT, 자료 삭제 |

확인:

- [ ] 실제 판매와 정산이 가능한 계정·국가·통화를 확인했다.
- [ ] `$49`는 첫 3건의 검증 가설이며 최종 가격이 아니라고 표기했다.
- [ ] intake만으로는 작업을 시작하지 않고, 서면 scope 확인 후 결제한다.
- [ ] 결제 완료 전에는 주문 수에 넣지 않는다.
- [ ] 실제 결제 경로가 없으면 **외부 판매 게시를 멈추고 준비물만 유지한다.**

### Step 3.5 — 참고: 가격 닻 조사 (2026-11-26)

이 절은 가격을 바꾸라는 지시가 아니다. `$49` 유지 범위와 이후 인상 여지를 판단할 때 참고하는 외부 조사 기록이다. 수치는 검색 결과 스니펫에서 해당 업체 페이지가 노출한 값이며, FAULTLINE 전체 페이지는 열지 못했다.

| 항목 | 값 | 출처 |
|---|---|---|
| 가장 가까운 독립 진단 오퍼 | $500 일회성 (Rapid Diagnostic) | faultline-labs.com (스니펫) |
| 에이전트 보안 감사 + 모니터링 | 월 $150–300 | ayfar.dev (스니펫) |
| 무료 감사(리드마그넷) 경쟁자 | 회사당 1회 무료 / 무료 티어 | omium.ai, agent-shield.com (스니펫) |
| 프리랜서 개발 시세(자가 진단 대안) | 중급 $40–70/hr, AI 중앙값 ~$30/hr | Upwork 2026 (스니펫) |
| 사고 비용 예시 | $4,200 환불 오류 → 총 $7,300 | activepieces 블로그 (단일 사례) |
| 셀프호스팅 일회성 라이선스 선례 | $99–500 | spryn.io, MailFlow (스니펫) |
| 관리형 인스턴스 시세 | 월 $10–20 (관리형 OpenClaw $9.99/월 서버, API 포함 총 $44–84) | xcloud.host, elest.io, openship.io, ordi.one (스니펫) |

해석: `$49`는 첫 3건 검증용으로 유지한다(시장 닻 대비 저가 — 신뢰 없는 첫 거래 문턱). 유료 1–3건 실측 후 §5 중단·변경 규칙의 "한 레버"로 가격을 조정할 수 있고, 그때 참고 밴드는 감사 $150–500, 박스 $99–500, 관리형 월 $15–30(고객 BYO 키 전제)이다. 이 숫자들은 확정가가 아니다.

### Step 4 — 판매 자료의 칸 채우기: 예상 15분

1. `docs/audit/index.html`의 CTA 링크 두 곳에 있는 `{{INTAKE_URL}}`을 form 주소로 바꾼다.
2. 같은 파일 상단의 `DRAFT — DO NOT PUBLISH YET` 상자를 제거한다.
3. `docs/audit/marketing.md`에서 다음을 실제 주소로 바꾼다.
   - `{{OFFER_URL}}`
   - `{{SAMPLE_URL}}`
   - `{{INTAKE_URL}}`
4. Herdr 커뮤니티 게시에는 실제 채널명으로 UTM source를 정한다.

게시용 두 파일에서 다음을 검색한다.

```bash
rg -n '\{\{[^}]+\}\}' docs/audit/index.html docs/audit/marketing.md
```

- [ ] 공개할 문구에 placeholder가 남지 않는다.
- [ ] 표본이 `synthetic`임을 삭제하지 않는다.
- [ ] Herdr 결함 확정, universal exactly-once, 실제 고객 성과 같은 미검증 문구가 없다.
- [ ] 가격·48시간 목표·제외 범위·자료 삭제 조건이 일치한다.
- [ ] 판매·표본 페이지의 임시 `noindex,nofollow`는 아직 제거하지 않는다.

### Step 5 — 공개 경로 확정: 예상 15분

`docs/` 호스팅이 활성화되어 있으면 push 즉시 `/audit/`가 공개된다. Step 1–4와 §6의 나머지 항목을 모두 통과하기 전에는 커밋·푸시하지 않는다.

- 기존 `docs/` 호스팅이 활성화되어 있다면 실제 `/audit/` 공개 URL을 확인한다.
- 호스팅이 없다면 owner가 승인한 정적 호스팅만 사용한다.
- 외부 배포와 커밋·푸시는 owner가 변경 diff를 검토한 뒤 별도로 실행한다.
- 배포 직전에 판매·표본 페이지의 `<meta name="robots" content="noindex,nofollow">`를 제거하고 §6의 해당 항목을 다시 확인한다.
- 배포 후 판매 페이지, 표본, fixture, checker, intake 링크를 실제 공개 환경에서 각각 연다.

### Step 6 — 두 채널만 게시: 예상 30분

**채널 1 — Herdr 생태계**

- `docs/audit/marketing.md` §3을 허용된 showcase/discussion 공간에 게시한다.
- 관련 issue에는 판매 링크 없이 §4의 진단 내용만 사용한다.
- Herdr 결함이나 공식 endorsement를 주장하지 않는다.

**채널 2 — 구매 채널**

- §5의 제목·설명·intake 링크를 선택한 marketplace에 게시한다.
- marketplace를 쓰지 못하면 self-promotion이 허용된 관련 개발 커뮤니티 한 곳으로 대체한다.
- 세 번째 채널은 이번 실험이 끝날 때까지 추가하지 않는다.

---

## 3. 매일 15분 측정

실제 데이터가 생기면 공개 저장소가 아닌 로컬 전용 파일에 기록한다.

```text
memory/audit-metrics.csv
```

`memory/`는 Git 추적 대상이 아니다. 형식은 `docs/audit/marketing.md` §8을 사용한다.

매일 기록:

- 채널별 qualified views
- intake starts / completed
- 실제 결제 수
- 납품 수와 환불 수
- owner 작업 시간
- 직접 비용
- 같은 제품화 요청 건수

주의:

- issue 조회수, Star, 반응, checkout 시작, Managed 제안 수락은 결제가 아니다.
- form 시작 수를 측정할 수 없으면 `unknown`으로 남긴다.
- 방문과 결제를 서로 다른 채널에 임의로 귀속시키지 않는다.

### Agent에게 맡길 분석 프롬프트

```text
Read only memory/audit-metrics.csv and docs/audit/marketing.md.
Separate measured values from unknowns. Compare channels by qualified views,
intake completion, actual paid orders, owner minutes, and direct cost.
Do not infer revenue from views, proposals, stars, or free usage.
Recommend exactly one change to the incident, offer, price, or channel.
Do not edit product code and do not publish or contact anyone.
```

---

## 4. 첫 유료 주문 납품 절차

### 4.1 접수와 scope 확인 — 목표 20분

- [ ] 한 사고인가?
- [ ] 재시작 중복 실행·중복 전송 또는 그에 준하는 불확실 전달인가?
- [ ] 전체 보안 감사, provider migration, 비용 최적화, connector 개발 요청은 아닌가?
- [ ] 비밀값과 불필요한 개인정보가 없는가?
- [ ] 재현에 필요한 최소 정보가 있는가?

맞으면 산출물·48시간 목표·자료 보관/삭제·결제 방법을 서면으로 확정한다. 맞지 않으면 범위를 늘리지 말고 거절하거나 별도 오퍼로 분류한다.

### 4.2 로컬 사건 폴터 생성 — 목표 10분

고객 자료는 공개 `docs/`가 아니라 다음 로컬 전용 위치에 둔다.

```text
memory/audits/<date>-<anonymous-case-id>/
```

저장할 것:

- scope와 intake 사본
- 비밀값 제거 입력
- owner 시간 기록
- fixture와 검사기
- 결과 보고서

실제 고객 로그·이름·회사명·사례는 별도 명시적 승인 없이 공개하지 않는다.

### 4.3 Agent에게 맡길 납품 프롬프트

```text
Work only inside memory/audits/<case>/ and use docs/audit/replay-delivery/
as the format reference. Build one synthetic/redacted reproduction fixture for
this single restart duplicate-execution or duplicate-delivery incident.
Create a deterministic non-LLM checker and an evidence report containing:
state/authority map, observed facts, reproduced facts, unresolved unknowns,
one remediation recommendation, and exact rerun commands.
Never fabricate a reproduction. Never access secrets, external accounts,
customer systems, or files outside the case directory. Do not publish,
contact the customer, modify Core, or change public documents.
Stop if the incident needs a custom connector or broader security analysis.
```

### 4.4 owner 최종 검수 — 목표 30분

- [ ] 입력에 비밀값이 없다.
- [ ] `reported`, `observed`, `reproduced`, `unknown`이 구분된다.
- [ ] 검사가 모델 완료 선언이 아니라 실제 사건·파일·프로세스 결과를 판정한다.
- [ ] 재현하지 못한 부분을 성공으로 쓰지 않았다.
- [ ] 원래 `unknown`이 새 전달 성공으로 덮이지 않는다.
- [ ] rerun 명령이 현재 환경에서 실제로 동작한다.
- [ ] 권고가 한 사고 범위를 벗어나지 않는다.

### 4.5 시간 예산

| 작업 | 목표 상한 |
|---|---:|
| scope와 intake 검토 | 20분 |
| 자료 수집·분류·고객 질문 | 45분 |
| fixture와 검사 검수 | 60분 |
| 보고서 검수와 수정 | 60분 |
| 최종 전달과 기록 | 25분 |
| 예비 | 30분 |
| **합계** | **240분 / 4시간** |

Agent 실행 시간을 owner 작업 시간으로 잘못 넣지 않고, 사람이 실제로 검토·수정·통신한 시간을 기록한다.

---

## 5. 7일차와 14일차 판단

### 정상 진행

- 첫 유료 감사를 48시간·4 owner hours 안에 납품했다면 같은 범위를 유지한다.
- 두 번째 주문도 동일한 절차로 처리한다.
- **같은 제품화 요청이 유료 감사 2건에서 반복되거나 실제 유료 product pilot 1건이 생길 때만** 다음 구현 카드를 만든다.

### 바로 기능을 추가하지 않는 경우

- 조회는 있지만 결제가 없음
- 무료 fixture 다운로드만 증가
- Herdr plugin 요청이지만 결제 없음
- Managed 관심 표명만 있음
- ACP·Headless·Dashboard를 먼저 만들면 팔릴 것 같은 추측

### 중단·변경 규칙

| 상황 | 다음 행동 |
|---|---|
| 200+ qualified views, 결제 0 | 사고 선택·오퍼·가격·채널 중 하나만 변경 |
| 조회 자체가 부족 | 제품이 아니라 채널과 제목을 변경 |
| intake는 있지만 결제 없음 | 신뢰·범위·가격·결제 마찰을 확인 |
| 3회 연속 서로 다른 맞춤 분석 | 고정 상품 중단, 컨설팅 또는 더 좁은 사고로 재분류 |
| 같은 유료 제품 요청 2건 | 한 기능만 별도 task card로 설계 |
| Managed 실제 결제 없음 | Managed 구현 금지 |
| 비밀값·개인정보 발견 | 작업 중단, 해당 자료 삭제, 비밀값 제거본 재요청 |
| Herdr·Odysseus·AgentLedger가 같은 사고를 더 잘 해결 | 범용 Hub/plugin 확장 중단 |

---

## 6. 외부 공개 직전 최종 확인

- [ ] `node docs/audit/replay-delivery/check.mjs`가 8/8 PASS다.
- [ ] full Rust suite가 `270 passed, 0 failed`다.
- [ ] 새 Agent 세션의 표본 재납품 리허설이 240 owner minutes 이내로 끝났고 시간 기록이 남아 있다.
- [ ] 판매 페이지와 표본이 로컬·공개 환경에서 모두 열린다.
- [ ] placeholder와 DRAFT 상자가 없고, 판매·표본 페이지의 `noindex,nofollow`가 제거됐다.
- [ ] intake 10문항과 form 확인 화면이 준비됐다.
- [ ] 결제·정산·환불·세금 가능 여부를 owner가 확인했다.
- [ ] 자료 보관·삭제 조건을 고객에게 보여줄 수 있다.
- [ ] 미검증 Herdr 결함·고객 성과·Outcome·Headless·Managed를 광고하지 않는다.
- [ ] 사용 채널이 정확히 두 개다.
- [ ] 외부 게시·배포·커밋·푸시를 owner가 별도 승인했다.

---

## 7. 다음에 Agent에게 줄 수 있는 작업

1. **표본 개선:** “공개 fixture를 변경하지 않은 채 판매 페이지와 intake 문구의 불일치만 찾아줘.”
2. **고객 납품:** §4.3 프롬프트를 사용해 로컬 사건 폴터에만 fixture·검사·보고서를 생성.
3. **채널 분석:** §3 프롬프트로 측정값과 미확인 값을 분리하고 한 가지 변경만 추천.
4. **반복 수요 검토:** “두 유료 사건의 공통 요청이 정말 같은지 비교하고, 다르면 제품화하지 말고 이유를 보고해줘.”
5. **제품화 카드:** 유료 게이트 통과 후에만 “한 시나리오에 대한 최소 Outcome/Evidence 또는 Herdr read-only plugin 중 하나를 task card로 작성해줘.”

**원칙은 하나다: 먼저 거래를 얻고, 같은 유료 요구가 반복될 때만 만든다.**
