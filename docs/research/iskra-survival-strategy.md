# Iskra 생존 전략 v2 — 거래 우선, 단일 사고, 단일 제품화 경로

- **작성일:** 2026-11-25
- **대상 저장소:** `https://github.com/jeonghoheo/iskra`
- **상태:** §1–4는 재검증하지 않은 기존 조사 입력이며, §5 이후는 이를 바탕으로 승인한 전략 재구성이다. P0 Stage 2.5-A는 완료됐고 P1 실행 패키지는 로컬에 준비됐다. 후속 Core 구현·외부 게시·결제·배포는 각각 별도 승인이 필요하다. §11은 Herdr 및 인접 경쟁자의 공식 공개 자료 재조사다.

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
| **유지** | Stage 2.5-A `delivery/v1`을 먼저 완료했다 | 전달 불확실성은 이미 현재 실행 경로의 실제 문제였으며 범위를 넓힐 이유가 없었다 |
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

### P0 — Stage 2.5-A 종료 (완료)

**완료:** Stage 2.5-A Goal 1–4와 owner-run Telegram 검증을 완료했다. 구현 기준선은 `bd62255`, 후속 기록 기준선은 `c769deb`다.

- 기존 합격 기준과 허용 파일 안에서 완료했다.
- Outcome·Evidence·ACP·새 명령을 Stage 2.5-A 카드에 추가하지 않았다.
- 전달 `unknown`, 무자동 재전송, 재시작 무자동 재실행, `/result`의 모델 호출 없는 결과 조회를 증명했다.

**통과:** 기존 Stage 2.5-A의 자동 검증·자원 예산·owner-run 게이트를 통과했다.

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

> **2026-11-26 갱신 (이 절의 §2–4항보다 우선):** 주인 승인으로 `headless/v1` 최소 슬라이스를 유료 신호 이전에 착수한다(단일 테넌트·파일 네이티브·텔레메트리 0·무재실행, API 3개 이하). `Agent Replay & Delivery Audit` 판매 트랙은 삭제가 아니라 보류이며, 재개 조건은 headless Goal 1~3 완료 또는 2027-01-07 중 먼저 오는 쪽이다. §5–§9의 판단 근거와 중단 규칙은 그대로 유효하고, 정본은 `DECISIONS.md` 2026-11-26과 `SESSION.md`의 `headless/v1` 카드다.

1. **완료된 코드 기준선:** Stage 2.5-A는 자동 검증과 owner-run Telegram 게이트를 통과해 `bd62255`로 구현됐고, 후속 기록 커밋은 `c769deb`다. 다음 Core 코드 작업은 자동으로 시작하지 않는다.
2. **준비된 P1 실행 패키지:** `docs/AUDIT-LAUNCH.md`를 기준 실행서로 사용한다. 가상 표본·검사기·판매 페이지·10문항 intake·게시 초안은 `docs/audit/`에 있다. 2026-11-25 실제 실행 결과는 fixture 검사 **8/8 PASS**, full Rust suite **270/270 PASS**(집중 표본 테스트 3개 포함)다. 4 owner hours 재납품은 owner 리허설 전이므로 아직 주장하지 않는다.
3. **owner 실행:** intake form과 실제 결제·정산 경로를 만들고, placeholder를 제거하며, 판매·표본 링크를 검수한 뒤 최대 두 채널에만 별도 승인으로 게시한다.
4. **보류:** Outcome·Evidence·Headless·ACP·Managed 구현은 P3·P4의 결제 및 필요 게이트 전까지 새 코드 작업으로 시작하지 않는다. 외부 게시, 고객 발송, 결제, 계정, 배포, 커밋·푸시도 각각 owner 승인이 필요하다.

생존의 기준은 더 많은 아키텍처를 먼저 완성하는 것이 아니다. **현재 신뢰성 경계를 끝내고, 한 가지 사고에 대한 실제 거래를 얻고, 같은 유료 요구가 반복될 때만 제품을 넓히는 것**이다.

---

## 11. Herdr 및 인접 경쟁자 재조사 — 2026-11-25

### 11.1 확인 범위

공식 제품 문서, 공식 GitHub 저장소와 issue를 확인했다. 기능은 공개 문서의 설명이며 직접 설치·성능 측정·동일 시나리오 bake-off는 아직 하지 않았다. Star 수는 결제 고객이나 매출이 아니라 배포력 신호로만 해석한다.

| 대상 | 공식 공개 자료에서 확인한 내용 | 전략상 의미 |
|---|---|---|
| [Herdr](https://github.com/herdrdev/herdr) / [제품 페이지](https://herdr.dev) | 실제 터미널, 다수 coding agent, SSH·모바일·agent-to-agent, 플러그인, Apache 2.0. 조사 시점 TUI 저장소 약 52.8k stars | 가벼운 Rust 단일 바이너리 런타임만으로 정면 승부하지 않는다. 경쟁자이면서 첫 배포 접점이다 |
| [Herdr 복구 문서](https://herdr.dev/tui/docs/session-state/) | detach 후 프로세스 유지, layout 복원, pane history, native agent session restore는 서로 다른 상태이며, 서버 재시작 시 원래 프로세스는 살아나지 않는다고 명시 | `layout ≠ process ≠ conversation ≠ external-effect safety`라는 네 계층을 감사·교육 자료로 사용한다 |
| [Herdr Marketplace](https://herdr.dev/tui/docs/marketplace/) | 공개 GitHub 저장소의 `herdr-plugin.toml`과 topic으로 자동 등록되며 무심사 목록이다 | 실제 좁은 plugin이 생긴 뒤 배포할 수 있지만, 등록 자체는 보증이나 수요 증명이 아니다 |
| [Herdr Odysseus](https://github.com/jpolec/herdr-plugin-odysseus), [Herdr Replay](https://github.com/neospeed83/herdr-replay), [patchreplay](https://github.com/maromocooo/patchreplay), [Termaxa](https://github.com/termaxa/termaxa) | 작업 분리·검사·승인·감사·복구·타임라인·before/after 증거·명령 결과 미리보기 기능이 이미 공개돼 있다 | 일반적인 Herdr 권한·증거·복구 플러그인도 빈 공간이 아니다 |
| [AgentLedger](https://github.com/yaogdu/AgentLedger) | durable execution, Tool Ledger, idempotency, evidence/replay, policy/approval과 duplicate-side-effect crash 예제를 공개 | Iskra의 장기 기능 목록도 이미 겹친다. 기능 수보다 특정 사고와 배포가 우선이다 |
| [AgentField](https://github.com/Agent-Field/agentfield) / [CodeAF](https://github.com/Agent-Field/CodeAF) | Agent API, queue/retry, identity·정책·감사, coding harness, headless, open models와 단일 바이너리를 공개 | Headless·모델 중립·단일 바이너리·감사를 독자 차별점으로 주장하지 않는다 |
| [Google Agent Executor](https://github.com/google/ax), [Anthropic Managed Agents](https://www.anthropic.com/engineering/managed-agents), [Temporal](https://temporal.io/solutions/ai) | durable execution, sandbox, session consistency, harness 분리, checkpoint·HITL 등이 벤더·인프라 제품으로 제공된다 | 범용 runtime/control plane 확장보다 고객의 구체적 사고 해결이 우선이다 |
| [OpenClaw](https://github.com/openclaw/openclaw) | 로컬 개인비서, 20개 이상 채널과 plugin, 약 395k stars, MIT | Telegram 개인비서 시장으로 이동하지 않는다 |
| [Vibe Kanban 종료 공지](https://www.vibekanban.com/blog/shutdown) | 수천 daily 사용자와 약 28.8k stars에도 회사는 유료 전환 부족과 사업 모델 부재로 종료를 발표했다 | Free adoption·Star·기능 수는 생존 증거가 아니다. 실제 결제가 먼저다 |

### 11.2 Herdr 사고 보고의 사용 경계

[Herdr #5231](https://github.com/herdrdev/herdr/issues/5231)에는 socket discovery 실패 후 여러 서버가 생기면서 한 worktree에 6개의 중복 writer가 생겼다는 사용자 보고와 재현 과정이 있다. 조사 시점 `maintainer-needed` 상태이며 maintainer의 단순 재현은 성공하지 않았다. [#6192](https://github.com/herdrdev/herdr/issues/6192)도 복구 시도의 명령·횟수·종료 상태가 로그에 없다는 최신 사용자 보고다.

따라서 이들은 **확정된 Herdr 결함 광고**가 아니라 독립 재현과 상태·권한 지도가 필요한 공개 사례로만 사용한다. 실제 환경에서 재현하기 전에는 `user report`와 `maintainer reproduction`을 구분하고, Iskra가 Herdr과 호환되거나 이 사고를 해결한다고 주장하지 않는다.

### 11.3 실행에 반영한 결정

- Herdr을 대체하라고 요구하지 않고, Herdr을 포함한 무인·반복 coding agent 사용자의 사고를 진단한다.
- 첫 유료 상품은 계속 `Agent Replay & Delivery Audit` **한 사고 고정 범위**로 유지한다.
- 첫 표본은 미검증 Herdr 재현 대신, 실제 Iskra 경로로 확인 가능한 **가상 중단 전송 사고**로 만든다.
- Herdr 커뮤니티와 실제 구매 marketplace는 배포 채널로만 사용하며, Herdr 전용 Core나 plugin을 예측 구현하지 않는다.
- 같은 유료 제품화 요청이 반복된 뒤에도 Odysseus·AgentLedger·Herdr과 동일 시나리오를 비교해 설치·재현·증거·복구·총owner 시간 우위가 없으면 확장하지 않는다.

### 11.4 소스 백분 조사 — 2026-11-26

`Agent-Field/agentfield`와 `herdrdev/herdr`의 공개 소스(Apache-2.0)를 읽기 전용으로 확인했다. 설치·실행·런타임 재현은 하지 않았고, 코드 복사는 없다. 아래는 공개 문서와 소스 파일에 근거한 사실이며 실행 동작 비교가 아니다.

| 대상 | 확인 내용 | 근거 |
|---|---|---|
| AgentField 컨트롤 플레인 | PostgreSQL 15+ 필수, goose 마이그레이션 39개, 기본 바인드 all interfaces(`AGENTFIELD_HOST` 빈 값 = 전체), loopback은 명시 설정 | `control-plane/README.md` |
| AgentField 내구성·증거 | append-only 실행 이벤트(단조 시퀀스)·lease·`status_reason`; 실행 VC에 input/output 해시·서명 | `migrations/013_workflow_execution_state.sql`, `004_create_execution_vcs.sql` |
| AgentField 재생 자세 | `inbound_events`에 idempotency_key가 있으나 replay 시 키를 지워 dedup 재전송 거부를 우회; 재시작은 새 execution + `restarted_as_execution_id` 연결 | `migrations/032_inbound_event_replay_of.sql`, `036_execution_restarted_as.sql` |
| Herdr 세션 재개 | resume = 각 에이전트 자체 resume 명령을 복원된 pane 셸에 타이핑; 인자 상한·제어문자/아포스트로피 금지·dedupe 검증 | `src/agent_resume.rs` |
| Herdr 경계 | 외부 부작용(중복 전송·중복 실행) 추적 레이어는 해당 범위에 없음. 코드 품질이 아니라 레이어 커버리지 차이로만 주장한다 | `src/agent_resume.rs` |

경계: "AgentField에 내구성·증거가 없다"는 주장은 소스로 반증된다 — 공개 비교는 "재생을 기본 자세로 두는가, 보이지 않는 재생을 금지하는가"의 차이로만 한다. AgentField 크래시 시 in-flight 회복 정책은 애플리케이션 코드·런타임 재현 미확인. Herdr 다중 writer 사고는 여전히 user report다. 런타임 bake-off(설치·재시작·중복 재현) 전에는 비교 공개 주장을 하지 않는다.

### 11.5 Paperclip 등재 — 2026-11-26

공식 GitHub README와 공개 문서만 확인했다. 설치·실행·코드 감사·동일 시나리오 bake-off는 하지 않았고, 보안 배지·기능 목록은 제품 측 주장으로만 취급한다. Star·fork 수는 배포력 신호이며 매출·결제 고객 증거가 아니다.

| 대상 | 공식 공개 자료에서 확인한 내용 | 전략상 의미 |
|---|---|---|
| [Paperclip](https://github.com/paperclipai/paperclip) / [문서](https://docs.paperclip.ing) | Node.js 24.11+·pnpm·임베디드 PostgreSQL·React UI·Docker, MIT. 외부 에이전트(Claude Code, Codex, OpenClaw, Cursor, Gemini CLI, OpenCode 등)를 heartbeat로 조직처럼 지휘. 조직도·atomic task checkout·승인 게이트·에이전트/프로젝트별 예산 hard stop·시크릿 매니저·MCP tool gateway(Allowed/Ask first/Off)·cron/webhook routine·multi-org·out-of-process plugin. 조사 시점 약 117.3k stars, 19.5k forks, 8,679 commits | 오케스트레이션 관제평면은 자금·커뮤니티가 붙은 헤비급이 이미 점유했다. Headless·거버넌스·예산·승인을 기능 수로 차별화하지 않는다는 §11.1 결론을 강화한다 |
| Paperclip 운영 자세 | 익명 텔레메트리 **기본 켜짐**(`PAPERCLIP_TELEMETRY_DISABLED=1`, `DO_NOT_TRACK=1`, `CI=true`, config로 해제). Sentry·OTel은 opt-in. quickstart 기본은 trusted local loopback, `--bind lan/tailnet`은 인증 모드 | Iskra의 "기본 텔레메트리 0"은 유지 가능한 차별점이다. 비교 시 기본값(default)과 해제 경로를 함께 적고, 기능 유무만 비교하지 않는다 |
| Paperclip 복구·예산 자세 | 로드맵에 self-healing runs·automatic recovery 완료 표시. 예산 집행은 기록된 지출 기준이라 in-flight 작업이 stop을 늦출 수 있다고 자체 명시. export/import는 참조 시크릿을 빼지만 plain env 값·로컬 경로가 남을 수 있다고 경고 | "자동 회복"과 "무재실행"은 같은 문제에 대한 반대 자세다. Iskra는 자동 재시도 대신 명시 재개·`unknown` 유지로 비교하되, 우위는 런타임 재현 전까지 주장하지 않는다 |
| Paperclip 설치·보안 표기 | `npx paperclipai@latest onboard --yes` 설치 경로, Docker 이미지. README에 "security posture verified by Superagent" 배지 | npx·대형 의존 트리는 공급망 검토 대상이다. 제3자 배지는 독립 검증이 아니므로 안전·불안전 어느 쪽으로도 인용하지 않는다 |

**경계.** 위 내용은 README·공개 문서 기준이며 코드 감사와 런타임 재현이 없다. "Paperclip이 위험하다/안전하다"는 결론을 공개 문서에 쓰지 않는다. Iskra와의 비교는 아래 항목을 같은 시나리오로 측정한 뒤에만 한다.

**비교 항목(전부 미측정).** 양쪽 제품에 동일하게 적용한다.

1. 설치·기동: 설치 시간, 필요 런타임·DB, 단일 바이너리 여부, 오프라인 동작.
2. 자원: 유휴 RSS, 디스크, 배포물 크기, 1개월 상시 운용 비용.
3. 외부 부작용 안전: 크래시 중 전송·결제·도구 실행의 중복 여부, 애매한 전송의 `unknown` 처리.
4. 복구 자세: 자동 재시도 여부, 재시도·재개 기록의 가시성, 사람 개입 지점.
5. 텔레메트리: 기본값, 해제 경로, 수집 필드, 전송 대상.
6. 권한: 기본 바인드, 승인 게이트 기본값, 시크릿 주입 범위, 도구 게이트웨이 기본 정책.
7. 총소유비용: 한 사고를 재현·진단·복구하는 데 든 owner 시간.

측정 전에는 Paperclip을 Iskra의 대체재나 비교 우위 대상으로 인용하지 않고, "오케스트레이터 시장이 이미 크다"는 수요 반증 자료로만 사용한다.
