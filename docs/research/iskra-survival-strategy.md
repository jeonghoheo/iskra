# Iskra 생존 전략 v2 — 거래 우선, 단일 사고, 단일 제품화 경로

- **작성일:** 2026-11-25
- **대상 저장소:** `https://github.com/jeonghoheo/iskra`
- **상태:** §1–4는 재검증하지 않은 기존 조사 입력이며, §5 이후는 이를 바탕으로 승인한 전략 재구성이다. P0인 Stage 2.5-A를 제외한 후속 구현·외부 게시·결제·배포는 각각 별도 승인이 필요하다.

---

## 1. 이 조사로 무엇을 결정하는가

Stage 2.5-A(`delivery/v1`) 완료 직후 Iskra가 시장에서 생존하고 첫 유료 검증(`G2`)을 통과하기 위해, Stage 3(ACP·위임)와 Stage 4(`outcome/v1`·`evidence/v1`·`headless/v1`)의 구현 우선순위 및 최소 제품 범위(MVP)를 어떻게 재배치할 것인가를 결정한다.

---

## 2. 확인된 사실 + 출처 + 확인 날짜

* **공개 저장소 현황:** GitHub 저장소(`https://github.com/jeonghoheo/iskra`, 확인일: 2026-11-25)는 Star 0, 소스 빌드(`cargo run`) 전용이며, 소개 문구는 "Rust로 만든 가벼운 개인용 AI 에이전트(Telegram 채널, Markdown 메모리)"로 표기되어 있고 Stage 2(2개 에이전트 런타임, `runs.json` 영속 큐, `@agent` 라우팅)까지 라이브 검증이 완료되었다.
* **아키텍처 현재 단계 및 자원 예산:** `docs/ARCHITECTURE.md`(문서 기준일: 2026-11-24, 확인일: 2026-11-25)상 Stage 2는 커밋 `3d3c98f`로 완료되었고 다음 구현 목표는 Stage 2.5-A(`delivery/v1`)이며, 코어 자원 예산은 stripped 바이너리 20MB 이하, 유휴 RSS 30MB 이하, 직접 의존성 15개 이하로 고정되어 있다[cite: 7].
* **로드맵 순서 및 미구현 범위:** `docs/ARCHITECTURE.md`(문서 기준일: 2026-11-24, 확인일: 2026-11-25)상 `outcome/v1`, `headless/v1`, `evidence/v1`, BYOUI, Lease는 Stage 4에 배치되어 있으며 현재 미구현 상태다[cite: 7].
* **타겟 시장 및 G2 합격 기준:** `docs/BUSINESS.md`(문서 기준일: 2026-11-24, 확인일: 2026-11-25)상 1차 타겟은 1~10인 소프트웨어/AI 팀 및 테크 창업자이며, 첫 유료 검증(`G2`)은 $49 비동기 `Agent Ops Audit` 1건을 48시간 및 소유자 투입 4시간 이내에 Iskra 실행 경로를 통해 납품하고 초기 구매자 3명 중 1명에게 `Managed Headless Workspace` 디자인 파트너 제안을 수락받는 것이다[cite: 8].
* **중단 규칙(Stop Rules) 및 경쟁 구도:** `docs/BUSINESS.md`(문서 기준일: 2026-11-24, 확인일: 2026-11-25)상 AgentField, CodeAF, OpenClaw, Vercel Board가 이미 존재하므로 '가장 작은 Outcome 검증 권한 허브'로 포지셔닝해야 하며, Audit 3회 연속 맞춤형 분석이 필요하거나 경쟁사 대비 실질적 우위가 없으면 범용 컨트롤 플레인 확장을 중단한다[cite: 8].
* **개발 토큰 소비 실태:** `WORKFLOW.md`(문서 기준일: 2026-11-23, 확인일: 2026-11-25)상 2026-11-25 로컬 로그 집계 기준 23개 세션에서 1,984회 LLM 호출, 누적 입력 약 3.812억 토큰(200k 초과 턴 121회), 파일 읽기 570회 중 동일 경로 중복 읽기 224회가 발생했으며, 구현은 10줄 내외 Task Card 단위로 진행한다[cite: 9].
* **와이어프레임 목표 UX 계약:** `iskra-ops-wireframe.jpg`(확인일: 2026-11-25)상 UI는 `TARGET CONCEPT · NOT IMPLEMENTED` 상태이며, 실행 횟수 대신 `Verified Outcome Rate`, `Cost / verified`, `Human 수정 / outcome`, `Baseline → Target → Current` 지표와 테스트 통과·`sha256` 해시·`secret access 0` 증거, Core Command 기반 `Approve once` 1회 승인 게이트를 전면에 배치한다[cite: 5].
* **시장 수요 증거(`G1`) 결과:** `demand-evidence.md`(조사일: 2026-11-24, 저장소 HEAD `34b8e58f`, 확인일: 2026-11-25)상 다중 에이전트/프로바이더 운영 고통 증거 17건(합격선 8건)과 상위 공통층 요청 6건(합격선 3건)이 확인되어 G1을 통과했으며, 핵심 장애 유형은 재시작/헬스체크 중복 실행·전송(`E08`, `E10`), 에이전트 거짓 완료(`E02`, `E13`), 권한 철회 무력화 및 명령어 변형 우회(`E05`, `S01`, `S02`), 상태 파일 폭증(`E14`: 33일간 24GB), 스트리밍 서브에이전트 토큰 2.7배 과소 집계(`E11`), 설정 드리프트(`E07`, `E17`)다[cite: 10].

---

## 3. 추론

* **외부 포지셔닝과 내부 전략의 괴리로 인한 초기 이탈 위험:** 내부 문서와 와이어프레임은 `Outcome`·`Evidence`·`No invisible replay` 기반의 컨트롤 플레인을 지향하지만, 공개 저장소는 개인용 텔레그램 봇으로 소개되고 `cargo run` 소스 빌드만 지원하므로 1~10인 개발팀이 Iskra의 핵심 차별점을 인지하지 못하고 이탈할 가능성이 높다.
* **로드맵 순서와 G2 시장 검증 시점의 충돌:** `BUSINESS.md`의 G2 게이트는 'Iskra 실행 경로를 통한 $49 Audit 납품'과 '15분 내 Outcome 정의 및 증거 획득'을 요구하지만, `ARCHITECTURE.md`상 `outcome/v1`과 `evidence/v1`이 Stage 3(ACP·위임) 뒤인 Stage 4에 묶여 있어 기존 순서를 고수하면 시장 검증 전에 구현 기간과 컨텍스트 예산이 과도하게 소모될 수 있다.
* **과잉 추상화로 인한 1인 개발 병목 위험:** Stage 4에 예정된 6종 계약(`adapter/v1`, `capability/v1`, `policy/v1`, `event/v1`, `evidence/v1`, `outcome/v1`)과 광범위한 연동 계층을 처음부터 분리 설계하면, `WORKFLOW.md`에서 관측된 컨텍스트 비대화(200k 초과 턴 빈발)가 재발해 개발 속도가 급격히 저하될 수 있다.
* **내부 운영 규율의 코어 기능화 및 Audit 반자동화 가능성:** `WORKFLOW.md`의 중복 파일 읽기 차단(`sha256` 포인터 반환)·2회 실패 자동 중단 규칙과 `demand-evidence.md`의 로그 진단 항목(`E08`, `E11`, `S02`, `E14`)을 Core 기본 기능 및 표준 진단 패키지로 전환하면, 맞춤형 컨설팅 인력 투입 없이도 `$49 Agent Ops Audit`을 일관되게 납품할 수 있다.

---

## 4. 미확인

* **외부 고객의 실제 결제 의사:** `G1` 증거(17건)는 공개된 운영 고통의 존재만 증명할 뿐, 낯선 실무자가 실제로 $49 Audit이나 월 구독형 `Managed Headless Workspace`에 비용을 지불할지는 미확인 상태다[cite: 8, 10].
* **경쟁사 대비 정량적 실측 우위:** AgentField, CodeAF, OpenClaw, Vercel Board의 최신 버전 대비 Iskra의 설치 시간, 총 자원 소모, 장애 복구, 프로바이더 교체, 데이터 이식성 우위는 정량 비교(Bake-off) 수행 전이므로 미확인 상태다[cite: 7, 8, 10].
* **GLM 및 러시아 개발자 시장 대표성:** Z.AI/GLM 관련 공개 이슈(`E14`, `E15`)와 설정 가이드는 확인되었으나, 러시아 개발자 커뮤니티 전반의 대규모 GLM 이주 여부나 대표성은 미확인 상태다[cite: 8, 10].
* **미첨부 SVG 5종의 내부 세부 텍스트:** `iskra-byo-ui.svg`, `iskra-control-plane-architecture.svg`, `iskra-final-product.svg`, `iskra-headless-strategy.svg`, `iskra-outcome-model.svg`는 렌더링 제약으로 박스 레이아웃만 확인되었으며 내부 세부 문구는 미확인 상태다[cite: 1, 2, 3, 4, 6].

---

## 5. 재구성 결론

**장기적인 Outcome-verified Authority Hub 방향은 유지한다. 그러나 초기 생존 순서는 `현재 Stage 완료 → 좁은 유료 감사 → 반복 결제 수요가 확인된 한 가지 제품화 → 필요할 때 Headless·Managed`로 바꾼다.**

처음부터 더 많은 제품을 만들어 수요를 증명하려 하지 않는다. 먼저 낯선 고객이 특정 사고를 해결하기 위해 돈을 내는지 확인하고, 같은 요구가 반복될 때만 Core를 확장한다.

초기에 검증할 제품 약속은 다음 하나다.

> **에이전트의 완료 선언이 아니라 독립적인 증거로 결과를 확인하고, 재시작 후 같은 작업이나 전송을 조용히 반복하지 않는 실행 경로.**

이 약속도 실제 시나리오의 실패 주입 검증이 끝난 범위에서만 공개한다. `Headless`, `Authority Hub`, `BYOUI`는 당분간 외부에 판매하는 현재 제품 범주가 아니라 내부 목표 아키텍처다.

### 5.1 기존 권고에서 유지·수정·폐기하는 결정

| 구분 | 결정 | 이유 |
|---|---|---|
| **유지** | Stage 2.5-A `delivery/v1`을 먼저 끝낸다 | 전달 불확실성은 이미 현재 실행 경로의 실제 문제이며 범위를 넓힐 이유가 없다 |
| **유지** | Outcome 달성과 Task/Run 완료를 분리한다 | 모델의 완료 선언은 독립적인 acceptance evidence가 아니다 |
| **유지** | No invisible replay, Core 권한, 단일 바이너리, 모델 비종속 | 장기 제품의 안전성과 교체 가능성을 지키는 불변 조건이다 |
| **유지** | 무료 Local → 유료 Managed → Team의 장기 구조 | 상시 운영과 협업을 수익화 축으로 유지한다 |
| **수정** | Outcome·Evidence·Headless 3종을 함께 앞당기지 않는다 | 감사는 현재 Iskra 경로로 시작하고, 제품 계약은 반복 유료 수요 뒤에 만든다 |
| **수정** | Outcome Pack 3종이 아니라 실제 수요가 확인된 1종만 만든다 | 세 표준 패키지를 예측으로 만들지 않기 위해서다 |
| **수정** | Managed 제안 수락이 아니라 실제 결제를 통과 조건으로 한다 | 진단 수요와 지속 운영 수요를 구분하기 위해서다 |
| **수정** | README를 미구현 Headless 제품으로 전면 전환하지 않는다 | 공개 문구는 검증된 현재 기능과 명시된 목표를 구분해야 한다 |
| **폐기** | 첫 감사에 provider 전환·전체 권한·비용·Outcome 3개를 모두 포함 | $49·4시간 고정 납품과 맞지 않고 맞춤 컨설팅 위험이 크다 |
| **폐기** | `iskra doctor`·`iskra export`·일반 로그 분석기를 예방적으로 선행 구현 | 실제 온보딩·납품 병목이 관찰되기 전에는 제품화 근거가 없다 |
| **보류** | ACP·위임·BYOUI·Team·provider shootout | 각각 외부 엔진·클라이언트·협업·비교에 대한 실제 필요와 결제를 먼저 확인한다 |

### 5.2 첫 고객과 포지셔닝

첫 대상은 “여러 모델을 쓰는 모든 1~10인 팀”보다 좁게 정의한다.

> **이미 코딩 에이전트를 무인 또는 반복 실행하면서, 재시작 중복 실행 또는 중복 전송 사고를 겪은 기술 소유자.**

- 고객은 하나의 반복 가능한 사고와 재현 가능한 로그를 가져와야 한다.
- 다중 provider와 GLM 사용은 필수 자격이 아니다. 관련 커뮤니티는 초기 배포 접점으로만 사용한다.
- 비개발자용 business autopilot, 범용 connector marketplace, 대규모 엔터프라이즈 관제는 초기 대상이 아니다.
- 20MB·30MB·의존성 15개 예산은 고객 가치 그 자체가 아니라 낮은 운영 비용과 단순한 복구를 지키는 가드레일이다.

공개 메시지는 두 단계로 관리한다.

1. **현재:** 검증된 durable runs, 명시적 routing, no automatic restart replay를 중심으로 설명하고 Outcome·Headless는 target/later로 표시한다.
2. **P0와 해당 시나리오 증명 후:** “Verify agent results independently. Recover without invisible replay.”처럼 검증된 범위만 약속한다. `Headless Control Plane`은 실제 외부 클라이언트 계약이 구현된 뒤 사용한다.

## 6. 첫 유료 오퍼 — Agent Replay & Delivery Audit

기존 `$49 Agent Ops Audit`을 다음의 고정 범위 상품으로 좁힌다. 이름과 `$49`는 모두 시험 가설이다.

### 6.1 입력

- 재시작 중복 실행 또는 중복 전송 **한 사고**의 편집된 로그·실행 이력.
- 재현에 필요한 최소 환경·버전·설정 목록.
- 고객이 공유에 동의한 가상 또는 비밀값 제거 자료만 사용한다. 실제 비밀값과 불필요한 개인정보는 받지 않는다.

### 6.2 산출물

1. 사고의 상태 전이도와 현재 실행·전달 권한의 소유 지점.
2. 같은 조건을 재현하는 최소 fixture.
3. 중복 실행·전송을 pass/fail로 판정하는 비LLM 검사.
4. 짧은 수정·검증 권고와 고객이 다시 실행할 수 있는 증거 묶음.

### 6.3 제외 범위

- provider 전환 또는 모델 선정 컨설팅.
- 전체 권한·보안 감사.
- 일반적인 토큰·비용 최적화.
- Outcome 3개 설계나 넓은 workflow 구축.
- 고객 전용 connector·private Core fork.
- 전화 인터뷰나 상시 컨설팅.

### 6.4 실행 방식과 해석

- 아직 없는 `headless/v1`을 전제로 하지 않는다.
- Stage 2.5-A 완료 후 현재 Iskra CLI·Run·Session·Delivery 경로와 고정된 로컬 감사 키트를 사용한다.
- 사람이 수행한 수집·분류·수정은 숨기지 않고 시간과 함께 기록한다.
- 유료 감사는 **고정 범위 진단 수요**만 증명한다. Outcome API·Headless·Managed 수요로 집계하지 않는다.

## 7. 실행 로드맵 P0–P4

### P0 — 현재 Stage 종료

**지금 할 일:** Stage 2.5-A Goal 3–4를 순서대로 끝내고 owner-run Telegram 검증을 수행한다. Goal 1–2의 기존 자동 검증 결과는 완료 기록으로 유지한다.

- 기존 합격 기준과 허용 파일을 유지한다.
- Outcome·Evidence·ACP·새 명령을 현재 카드에 추가하지 않는다.
- 전달 `unknown`, 무자동 재전송, 재시작 무자동 재실행, `/result`의 모델 호출 없는 결과 조회를 증명한다.

**통과:** 기존 Stage 2.5-A의 자동 검증·자원 예산·owner-run 게이트 통과.

### P1 — 감사 상품 준비

**새 Core 계약 없이** 가상 데이터로 표본 감사 1건을 제작한다.

- 현재 Iskra 경로로 표본 fixture와 재현 가능한 증거 묶음을 만든다.
- 4 owner hours 이내에 같은 표본을 다시 납품해 본다.
- 고정 범위 판매 페이지, 10문항 비동기 intake, 결제·환불·세금 가능성을 준비한다.
- 실제 게시·결정된 가격·결제 경로·고객 자료 수집은 owner가 별도 승인한다.

**통과:** 표본이 재현 가능하고 4시간 이내이며 자동·수동 작업이 구분된다.

**만들지 않는 것:** 새 Core 계약, 일반 로그 분석기, 웹 UI, Outcome Pack 3종.

### P2 — 실제 거래 확인

- 최대 2개 채널에만 게시하고 views·intake starts·paid orders를 기록한다.
- 첫 주문은 동일한 고정 범위로 48시간·4 owner hours 이내에 납품한다.
- 채널별 수치를 합산할 때 중복 방문과 실제 구매를 구분한다.

**초기 가설:** 200+ qualified views, 5+ intake starts, 1+ paid order.

**실패 시:** 기능을 추가하지 않고 사고 선택·오퍼·가격·채널 중 무엇이 실패했는지 구분해 바꾼다.

### P3 — 반복 수요가 확인된 한 가지만 제품화

다음 둘 중 하나를 별도로 확보한 뒤에만 Core 제품화 설계를 시작한다.

- 같은 제품화 요구를 가진 **유료 감사 2건**.
- 해당 기능을 실제 사용할 **유료 product pilot 1건**.

통과한 시나리오 하나에만 다음 최소 범위를 설계한다.

- `outcome/v1`: owner, desired result, acceptance, budget, scope, delivery 정책 중 그 시나리오에 필요한 필드.
- `evidence/v1`: 기계적 판정 결과, exit code, artifact hash, Run/Delivery identity, 비용을 알 수 없는 경우의 명시적 표현.
- Outcome Pack **1종**과 반복 가능한 실패 주입 검사.

`Approve once`는 권한 있는 부작용을 허가한 기록이지 Outcome이 참이라는 증거가 아니다. Outcome 달성은 선언된 비LLM acceptance check와 required evidence로 판정한다.

**보류:** 추가 Pack, 6종 추상화의 선행 분리, `doctor`·`export`, 일반화된 비용·provider 분석. 실제 납품 병목이나 유료 요구가 생기면 각각 별도 카드로 검토한다.

### P4 — 필요할 때 Headless와 Managed

- 실제 외부 클라이언트가 Iskra에 Outcome을 제출해야 하는 요구가 확인되면 `headless/v1`을 별도 설계한다.
- ACP·자유 위임은 내장 HTTP Agent만으로 결제가 확인된 작업을 끝낼 수 없다는 증거가 있을 때만 연다.
- Managed는 로컬에서 같은 Outcome이 반복 성공하고 backup/restore/export/delete와 총원가를 검증한 뒤 유료 pilot으로 시작한다.
- Managed **제안 수락은 통과가 아니다. 실제 결제만 통과다.**
- Managed 확장은 3개 유료 workspace, 그중 2개의 4주 유지, 반복 Outcome의 무수동 수리 운영, 복원·export·삭제와 총원가 검증 후 결정한다.

BYOUI·Team·자동 delegation·Lease는 이 단계의 기본 선행 작업이 아니라 각각 실제 고객 필요와 별도 보안·제품 게이트가 있는 후속 항목이다.

## 8. 무엇을 측정하고 어떻게 판단하는가

초기에는 작은 표본의 비율 하나보다 실제 건수·시간·반복 패턴을 우선한다.

| 영역 | 우선 측정 | 판단 주의 |
|---|---|---|
| 거래 | qualified views, intake starts, paid orders, 실제 Managed 결제 | 감사 구매를 Managed 구매로 합산하지 않는다 |
| 납품 | 감사당 owner 시간, 수동 수정 횟수, 같은 사고 패턴 반복 건수 | 한 번의 고난도 맞춤 작업을 표준 제품 수요로 보지 않는다 |
| 제품 증명 | 실패 주입에서 자동 재실행·중복 전송 횟수, acceptance 재현 여부, evidence까지 걸린 시간 | 통과한 fixture 밖의 절대적 안전을 주장하지 않는다 |
| 비용 | 실제 provider·외부 엔진·인프라·owner 시간, `cost_known: false` 항목 수 | 목록 가격만으로 실제 Outcome 비용을 계산했다고 하지 않는다 |

Verified Outcome Rate·Cost per Verified Outcome·gross margin은 충분한 반복 표본과 실제 운영 데이터가 생긴 뒤 확장 판단에 사용한다.

### 각 신호가 증명하는 것

| 신호 | 증명 | 증명하지 않는 것 |
|---|---|---|
| G1 공개 사고 기록 | 운영 고통이 공개적으로 보고됨 | Iskra나 감사에 대한 결제 의사 |
| 유료 고정 범위 감사 | 특정 진단 결과에 돈을 낼 고객 존재 | Outcome API·Headless·Managed 수요 |
| 같은 유료 요구의 반복 | 표준화할 제품 후보 존재 | 장기 유지나 Managed unit economics |
| owner dogfooding | Iskra 실행 경로와 증거가 동작함 | 외부 고객 수요 |
| Managed 제안 수락 | 다음 제안에 대한 관심 | 실제 매출 또는 4주 유지 |
| Managed 실제 결제와 유지 | Managed 가설에 대한 직접 증거 | 다른 시장·다른 workflow로의 일반화 |

## 9. 중단·축소 규칙

- 200+ qualified views에서 주문 0건이면 기능이나 connector를 추가하지 않고 사고·오퍼·가격·채널을 바꾼다.
- 감사 3회 연속 서로 다른 맞춤 분석이 필요하면 고정 오퍼를 중단한다.
- 유료 감사에서 같은 제품화 요구가 반복되지 않으면 Outcome·Headless Core 확장을 시작하지 않는다.
- 감사는 팔려도 Managed 실제 결제가 없으면 Managed 확장을 멈추고 감사 상품 또는 작은 adapter로 재분류한다.
- 외부 엔진이 Core 정책을 우회해야만 유용하다면 해당 연동을 중단한다.
- 동일 시나리오 비교에서 기존 제품이 설치·복구·증거·총원가 요구를 더 잘 충족하면 generic Hub 확장을 중단하고 작은 adapter 또는 감사 제품으로 좁힌다.
- 새 표면이 단일 바이너리·보안·개인정보·의존성 예산을 침범하면 별도 승인 없이는 추가하지 않는다.

## 10. 현재 다음 행동

1. **코드:** Stage 2.5-A Goal 3 — durable terminal recovery를 기존 승인 범위와 별도 구현 지시에 따라 진행한다. Goal 1–2의 기존 변경을 보존하며, 이 전략 문서가 새 구현 Goal을 자동으로 시작하지는 않는다.
2. **다음 사업 작업:** P0 완료 후 `Agent Replay & Delivery Audit` 표본 1건과 고정 범위 오퍼 초안을 별도 작업으로 준비한다.
3. **보류:** Outcome·Evidence·Headless·ACP·Managed 구현은 P3·P4의 결제 및 필요 게이트 전까지 새 코드 작업으로 시작하지 않는다.
4. **외부 행동:** 게시, 고객 발송, 결제, 계정, 배포, 커밋·푸시는 각각 owner 승인 후 수행한다.

생존의 기준은 더 많은 아키텍처를 먼저 완성하는 것이 아니다. **현재 신뢰성 경계를 끝내고, 한 가지 사고에 대한 실제 거래를 얻고, 같은 유료 요구가 반복될 때만 제품을 넓히는 것**이다.
