# Iskra — 에이전트 작업 지시
## 읽기 순서
1. 공통 기억: `/Users/heojeongho/Desktop/My/AI-personal-memory/AGENTS.md` → `USER.md` → `NOW.md` → Iskra 관련 최신 `DECISIONS.md`. 필요한 범위만 읽고 변경 없는 재독은 피한다.
2. 저장소: `SESSION.md` 현재 카드 → `SESSION-LOG.md` 마지막 관련 기록 → `README`·`docs` 필요한 섹션. 조사·설계·구현·검증 단계가 끝나면 카드와 결과만 남기고 새 세션으로 인계한다.
## 기술 스택
- Rust stable, Tokio, Teloxide 0.17, Reqwest, Serde, Dotenvy. TLS는 rustls 고정(OpenSSL 금지). Telegram Bot API + 로컬 Markdown 기억 + OpenAI 호환 HTTP API.
- 모델은 `LLM_API_KEY`·`LLM_BASE_URL`·`LLM_MODEL`로만 설정하고 코어·공개 문구에 특정 모델을 고정하지 않는다. Tauri·모바일·MQTT·CLI 어댑터는 v1 범위가 아니다.
## 불변 규칙
- fail-closed: 허용 목록에 없는 발신자·경로·명령·도구는 거부. 권한 검사는 코어 관문에서 수행한다.
- `run_command`는 셸 문자열 해석 없이 인자 배열로 실행하고 경로·명령·시간·출력을 제한한다. 제3자 코드는 코어 안에서 실행하지 않는다.
- 기본 텔레메트리·조용한 보안 폴백 금지. 외부 런타임·DB 서버·컨테이너 없이 단일 바이너리. 예산: strip 후 20MB, 유휴 메모리 30MB, 직접 의존성 15개 이하(새 의존성은 기록 후 승인).
- 기억·정책·승인·감사 로그·비용 집계는 코어가 소유한다. 외부 모델·CLI 엔진이 우회하지 못한다.
## 작업 규칙
- Plan/Goal/일반 모드 분업은 `docs/WORKFLOW.md`. Goal은 자동 검증 가능한 구현에만, owner 입력 검증은 일반 모드. 코드·로그·프롬프트는 영어, 사용자 문구는 `en/ko/ru`, 주인 응답은 한국어, 기존 `docs`·`SESSION-LOG` 언어는 임의 변경 금지.
- 개인 기억·자료를 저장소에 복사하지 않는다. 문서 충돌 시 임의 선택 금지. 커밋·푸시는 별도 승인 후.
