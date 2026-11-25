# Iskra (Искра)

[English](README.md) | [Русский](README.ru.md) | **한국어**

![Iskra mascot](assets/iskra-mascot.gif)

**Rust로 만든, 지속 실행을 갖춘 가볍고 모델 중립적인 에이전트 런타임.**
*Iskra*(Искра)는 러시아어로 "불꽃" — 모든 것이 시작되는 작은 불씨라는 뜻입니다.

![License: FSL-1.1-ALv2](https://img.shields.io/badge/License-FSL--1.1--ALv2-blue.svg)
![Built with Rust](https://img.shields.io/badge/Built%20with-Rust-orange.svg)
![Status](https://img.shields.io/badge/Status-v2%20in%20progress%20%C2%B7%20two--agent%20runtime-blue.svg)

Iskra는 지속 작업, 명시적 라우팅, 읽을 수 있는 기억, 엄격한 도구 경계를 갖춘 개인 에이전트 런타임입니다. 중단된 작업을 재시작 시 자동으로 다시 실행하지 않으며, Core 권한을 옮기지 않은 채 모델·창구·도구를 교체할 수 있습니다.

> **상태:** v1은 완성되어 실사용 테스트 중입니다. v2의 첫 조각인 두 에이전트 워크벤치(지속 작업 큐·명시적 라우팅)는 구현과 라이브 검증을 마쳤습니다. Stage 2.5-A 전달 신뢰성은 진행 중입니다. Outcome·Evidence·Headless·ACP·위임·결정 모델 라우팅은 아직 구현되지 않았으며, 각각 수요·보안·구현 게이트 뒤의 목표입니다.
> 이 저장소에는 코드만 있습니다 — 개인 기억, 데이터, 비밀값은 여기에 두지 않습니다.

## 설계 원칙

1. **희망이 아니라 예산으로 가볍게** — 단일 바이너리, 크기와 유휴 메모리, 의존성 개수에 하드 리밋을 걸고 릴리스마다 측정합니다.
2. **규격으로 모듈화** — 두뇌, 창구, 도구, 기억은 버전이 있는 인터페이스 규격(`brain/v1`, `channel/v1`, `tool/v1`, `memory/v1`)으로 연결되도록 설계되었고, 규격은 v2에서 코드로 정식화됩니다. 확장은 코어 수정이 아니라 부품 추가입니다.
3. **fail-closed 안전** — 도구는 샌드박스 폴더 안에서만 돕니다. 셸 도구는 목록에 있는 프로그램·하위 명령·인자만 허용하고, 비운 환경 변수로 실행되며, macOS에서는 네트워크가 막힌 OS 샌드박스 안에서 돕니다. 기억에 저장하기 전에 자격증명을 가립니다. 되돌릴 수 없는 행동의 사람 승인은 v2에서 도입할 예정입니다.
4. **원가 규율** — (예정) 반복 작업은 모델 호출 없이 도는 절차로 결정화합니다. 비싼 감각(화면)은 최후 수단이고, 구조화 통로(API · DOM)가 먼저입니다.
5. **읽을 수 있는 기억** — 장기 기억은 로드 예산이 있는 평범한 마크다운이며, 저장 시 자격증명을 가리고, 감염 방어를 갖췄습니다 — 지금은 모델에 주는 규칙 수준이고, 외부 내용의 구조적 격리는 v2에서 도입할 예정입니다.

## 현재 상태 (v1 완료 · v2 진행 중)

- [x] Rust 골격 + 두뇌 (SDK 없이 HTTP 직접 호출) — 모델은 교체 가능한 부품, v1 기본값은 DeepSeek
- [x] 멈춤 조건이 있는 대화 루프 (도구 왕복 상한, 같은 오류 반복 시 정지)
- [x] `workspace/`로 샌드박스된 파일 도구 (`read_file`, `write_file`)
- [x] 예산 로드 + 가림 저장되는 마크다운 기억, 메시지마다 다시 읽기(재시작 불필요), 정체성 파일 (SOUL.md / USER.md)
- [x] 발신자 허용 목록이 있는 텔레그램 창구
- [x] 셸 도구 (`ls`, `cat`, `git`, `cargo`) — 하위 명령·인자 정책, 비운 환경 변수, macOS 샌드박스 (OS 샌드박스가 없는 곳에서는 cargo 꺼짐) + 검증된 코딩 과제
- [x] 셸 보강 (v1.1): 우회 경로 21개 차단, 전부 테스트로 고정
- [x] 두 에이전트 런타임 (Stage 2): 로컬 `iskra.json` 레지스트리(에이전트 최대 2개), 에이전트별로 분리된 모델·도구·기억·세션·작업 폴더·사용량, `memory/runs.json`의 지속 작업 큐, 명시적 `@agent` 라우팅, `/agents`·`/status [@agent]`·`/stop [@agent]`·`/resume [@agent]`, 다른 에이전트 답변의 `@이름` 머리말
- [ ] 진행 중: Stage 2.5-A 전달 신뢰성(`delivery/v1`) — Goal 1–2는 구현과 자동 검증을 마쳤지만 durable recovery, 최종 milestone, owner-run 전달 게이트는 남아 있습니다. 이후 Outcome·Evidence·Headless·ACP·위임·제품 표면은 각각 별도 수요·구현 게이트가 필요합니다

## 로드맵

| 버전 | 방향 |
|---|---|
| v1 | 한 사람을 위한 개인 에이전트 (텔레그램, 두뇌 1개, 도구 5개, 마크다운 기억) |
| v2 (진행 중) | 갈아끼우는 부품 — 두 에이전트 런타임·지속 작업 큐·명시적 라우팅 완료. 전달 신뢰성(`delivery/v1`) 진행 중. 이후 Outcome·Evidence·Headless·ACP·위임·라우팅은 자동 순서가 아니라 수요 게이트로 결정 |
| v3 | 일을 맡기는 에이전트 — 트리거, 봇 패키지, 신원 규격 (`identity/v1`) |
| v4 | 데스크톱 앱 (Tauri) — 멀티봇, 승인 게이트 뒤의 컴퓨터 유즈 |
| v5 | 제품화 — 호스티드 모드, 사용자별 기억 + 어댑터 서빙, 서명 릴리스 |

## 빠른 시작

```bash
git clone https://github.com/jeonghoheo/iskra.git
cd iskra
cp .env.example .env   # 모델 API 키를 넣으세요 (LLM_API_KEY — 어떤 제공자든 가능, 기본값은 DeepSeek)
cargo run
```

두 번째 에이전트를 더하려면 실행 위치에 로컬 `iskra.json`을 만드세요(형식은 `docs/ARCHITECTURE.md` §12). `iskra.json`이 없으면 기존처럼 `main` 하나로 동작합니다. 한 데이터 디렉터리에는 Iskra 인스턴스를 하나만 실행할 수 있으며, 두 번째 프로세스는 모델 호출이나 상태 파일 쓰기 전에 거부됩니다. 추가 에이전트의 작업 폴더와 기억 폴더는 ID에서 파생된 실제 디렉터리여야 하고(`agent-workspaces/<id>/`, `memory/agents/<id>/`), `main`·다른 에이전트·바깥 경로를 가리키는 심볼릭 링크나 임의 경로는 시작 시 거부됩니다.

## 라이선스

**Functional Source License, Version 1.1 (FSL-1.1-ALv2)** 로 소스 공개 — [LICENSE](LICENSE) 참조. 자유롭게 사용, 연구, 수정, 셀프호스팅할 수 있습니다. 다만 경쟁하는 상업 제품이나 서비스로 타인에게 제공하는 것은 허용되지 않습니다. 각 릴리스는 공개 2년 후 Apache 2.0으로 전환됩니다.
