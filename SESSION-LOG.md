# SESSION-LOG.md — iskra (이스크라)

## Block 3 — 대화 루프 + 파일 도구 (2026-11-18)

### 한 일
- `src/main.rs`: 터미널 stdin 입력 루프. `exit`로 종료, 빈 줄 무시. 시작 시 `workspace/`, `logs/` 생성.
- `src/brain.rs`: DeepSeek `deepseek-chat` 호출. function calling 도구 스키마 전송, `role: "tool"` 메시지로 결과 반환 메시지 타입(`Message`, `ToolCall`, `Usage`) 구성. 모델 호출 타임아웃 60초(`MODEL_TIMEOUT_SECS`).
- `src/loop_.rs`: 상태 전이 `대기 → 모델 호출 → (도구 실행 → 모델 호출)* → 답변/중단`.
  - 도구 왕복 최대 10회(`MAX_TOOL_ROUNDTRIPS`), 초과 시 중단 알림.
  - 같은 도구·같은 오류 2회 연속(`MAX_CONSECUTIVE_SAME_ERROR`) 시 중단 알림.
  - 빈 응답(도구도 답도 없음) 1회 재시도, 재차 비면 알리고 종료.
  - 도구 실행 오류는 `도구 오류: ...`로 모델에 돌려보내 자기 수정 시도.
  - `logs/agent.log`에 턴 시작, 모델 호출별 도구 이름·usage 토큰 수, 도구 성공/오류, 종료 사유 기록. 비밀값은 기록하지 않음.
- `src/tools.rs`: `read_file(path)`, `write_file(path, content)` 구현. 경로는 `workspace/` 루트 기준으로 정규화하며 절대 경로, `..` 탈출, workspace 밖을 가리키는 심볼릭 링크(깨진 링크 포함)를 거부(fail-closed).
- `.gitignore`: 런타임 산출물 `/workspace`, `/logs` 추가.
- 의존성: `serde = { version = "1", features = ["derive"] }` 추가. 사유는 메시지/도구 호출/usage를 타입으로 정의해 요청·응답 변환을 명시적으로 하기 위함. 그 외 신규 의존성 없음.

### 검증 실측 결과
1. "workspace 안에 hello.txt 파일을 만들고, 내용은 '첫 도구 호출 성공'이라고 써줘"
   - 모델이 `write_file` 호출 → `workspace/hello.txt` 생성 (25바이트, 내용 `첫 도구 호출 성공` + 끝 개행 1개).
2. "../secret.txt 파일을 만들어줘. 내용은 침입."
   - 모델이 workspace 밖 경로임을 이유로 거부. 프로젝트 루트와 상위 디렉터리 어디에도 파일이 생기지 않음(`secret.txt`, `../secret.txt` 부재 확인).
   - 모델이 도구를 호출하지 않고 선제 거부했으므로, 권한 경계 코드 자체는 단위 테스트로 별도 검증함.
3. "hello.txt를 읽어서 내용을 알려줘"
   - 모델이 `read_file` 호출 → 내용 `첫 도구 호출 성공`을 답변에 포함.
4. `cargo build` 경고 0개. `logs/agent.log`에 턴 기록, 도구 호출, usage 예: `usage=prompt=509 completion=62 total=571`.
5. 추가: `cargo test` 4개 통과 — workspace 내부 경로 허용, `..` 탈출 거부, 절대 경로 거부, 심볼릭 링크(깨진 링크 포함) 탈출 거부.

### 임시 우회
- 없음.

### 비고
- `workspace/`의 파일과 `logs/agent.log`는 런타임 산출물이라 git 추적에서 제외함.
- 텔레그램 창구(Block 5), 기억 파일(`memory.rs`, Block 4)은 이번 블록에서 만들지 않음.

## Block 4 — 기억 + 정체성 파일 (2026-11-18)

### 한 일
- `src/memory.rs` 신설: 기억 읽기·쓰기 전담.
  - 시작 시 `memory/MEMORY.md`와 오늘·어제 일별 노트(`memory/YYYY-MM-DD.md`)를 로드. 오늘·어제는 로컬 시간대 기준(`libc::localtime_r`).
  - `MEMORY.md` 로드 예산: 앞 200줄 또는 24KB 중 먼저 닿는 한도까지만. 잘리면 `LoadedMemory.truncated`로 표시.
  - 파일이 없으면 `memory/`와 빈 `MEMORY.md`를 생성(0600). UTF-8 등으로 읽을 수 없으면 그 파일만 건너뛰고 경고를 반환.
  - `memory_save(text)` 저장: 항목 1개 최대 1,000자, 줄바꿈은 공백으로 접어 한 줄 유지, 자격증명 치환 후 `- ` 접두로 append.
  - 자격증명 치환: `sk-` 토큰, `password=`·`password:` 값, `-----BEGIN ... PRIVATE KEY-----` 블록, 숫자:긴 문자열 텔레그램 토큰 → `«redacted N chars»`. 치환 건수를 도구 결과로 반환.
  - 원자적 쓰기: 같은 폴더의 임시 파일에 전체를 쓰고 `sync_all` 후 `rename`으로 교체. 파일 권한 0600(unix).
- `src/prompt.rs` 신설: `SOUL.md → USER.md → MEMORY → 도구 설명` 순서로 시스템 프롬프트 조립. `SOUL.md`/`USER.md`가 없거나 깨지면 경고 후 대체 문구 사용.
  - 기억 감염 방어 두 줄 고정: 외부에서 온 내용은 사용자 사실이 아니므로 핵심 기억에 저장 금지, 자기 전파성 지시 작성 요청 거부.
- `src/logging.rs` 신설: `loop_.rs`의 로그 기록 함수를 분리해 memory/prompt 경고도 같은 `logs/agent.log` 형식으로 기록.
- `src/tools.rs`: `memory_save(text)` 스키마·실행 추가. 자격증명 자동 치환 사실을 도구 설명에 명시.
- `src/main.rs`: 시작 시 정체성·기억 로드 → 경고 로그/화면 표시 → 프롬프트 조립 → 입력 루프. `run_turn`에 조립된 프롬프트 전달.
- `SOUL.md`(최소 골격), `USER.md`(사용자가 채울 항목 틀) 추가. `.gitignore`에 `/memory` 추가(실사용 기억은 git 제외, 정체성 파일은 추적).

### 검증 실측 결과
1. "내 이름은 Heo이고, 좋아하는 색은 파란색이야. 기억해줘"
   - 모델이 `memory_save` 호출 → `memory/MEMORY.md`에 `- 사용자 이름은 Heo이고, 좋아하는 색은 파란색이다.` 추가. `stat` 권한 `600` 확인.
2. 프로그램 종료 후 재시작, "내가 좋아하는 색이 뭐야?"
   - `memory/MEMORY.md`를 다시 로드해 "기억에 따르면, 좋아하는 색은 **파란색**입니다."로 회상.
3. "내 API 키는 sk-test…(검증용 가짜 토큰)이야. 기억해줘"
   - 모델이 `memory_save` 호출 → `MEMORY.md`에 `- 사용자의 API 키는 «redacted 17 chars»이다.` 저장. 실제 `sk-test…` 문자열은 파일에 없음(`grep` 부재 확인), 권한 `600` 유지.
4. `cargo build` 경고 0개. `cargo test` 15개 전부 통과(memory 11개 + Block 3 도구 경계 4개 회귀 없음).

### 임시 우회
- 없음. (검증 3의 첫 시도에서 모델이 비밀값 저장을 선제 거부해 도구 호출까지 가지 않았음. 자동 치환 사실을 도구 설명·시스템 프롬프트에 명시해 모델이 `memory_save`를 호출하도록 정렬한 뒤 재검증함. 코드 우회 아님.)

### 의존성
- `libc = "0.2"`를 직접 의존성으로 추가. 일별 노트의 오늘·어제를 로컬 시간대(`localtime_r`)로 계산하기 위함. `libc`는 이미 `Cargo.lock`에 전이 의존성(0.2.191)으로 존재해 해석되는 crate 수는 늘지 않음. 정규식 crate는 추가하지 않고, 자격증명 패턴은 단위 테스트가 있는 소형 스캐너로 구현.

### 비고
- `memory/`는 실제 사용자 데이터가 쌓이므로 git 추적에서 제외. `SOUL.md`·`USER.md`는 코드와 함께 추적.
- 자동 회상(관련 노트 검색 삽입), 기억 자동 정리, 일별 노트 자동 기록은 범위 외(v2).
