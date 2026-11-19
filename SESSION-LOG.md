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

## Block 4 — 기억 + 정체성 파일 (2026-11-19)

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

## Block 5 — 모델 설정 외부화 + 텔레그램 창구 (2026-11-19)

### Step 0 — 사전 정리
- Block 4 항목 날짜를 실제 실행·push 날짜인 2026-11-19로 정정.

### 한 일
- `src/brain.rs`: 모델 설정을 코드 밖으로 분리.
  - `Config::from_env`/`from_lookup`: 키는 `LLM_API_KEY` 우선, 없으면 `DEEPSEEK_API_KEY`(호환). 주소는 `LLM_BASE_URL`(기본 `https://api.deepseek.com/chat/completions`), 모델은 `LLM_MODEL`(기본 `deepseek-chat`). 빈 값은 미설정으로 취급.
  - 두 키가 모두 없으면 필요한 변수 이름을 명시해 실패(fail-closed). `Brain::new(Config)`, `Brain::model()` 추가. reqwest 직접 HTTP·OpenAI 호환 형식은 그대로 유지.
  - 단위 테스트 4개: 기본값 적용, `LLM_*` 우선, `DEEPSEEK_API_KEY` 호환, 키 누락 오류 메시지.
- `.env.example`: `LLM_API_KEY`·`LLM_BASE_URL`·`LLM_MODEL`과 텔레그램용 `TELEGRAM_BOT_TOKEN`·`ALLOWED_USER_ID`를 기본값 주석과 함께 추가.
- `src/channel.rs` 신설: 텔레그램 창구 격리.
  - 시작 시 `TELEGRAM_BOT_TOKEN`·`ALLOWED_USER_ID`를 검증하고, 없거나 형식이 틀리면 사유만 로그에 남기고 종료. 토큰 값은 로그·출력에 쓰지 않음.
  - teloxide 롱폴링(Dispatcher). 발신자 ID가 `ALLOWED_USER_ID`와 다르면 무응답 + `telegram_ignore user_id=...` 로그.
  - `/start`(환영·사용법 1줄), `/stop`(진행 중 턴 태스크 abort), `/status`(모델명·기억 로드 상태·오늘 토큰 사용량). 그 외 텍스트는 기존 `loop_::run_turn`으로 전달.
  - 답변은 4,096자 단위로 분할 전송. 먼저 `ParseMode::Markdown`으로 보내고 실패하면 평문으로 1회 재전송. 모든 전송 실패는 로그 후 1회 재시도. 성공 시 `telegram_reply_sent chunks=N` 기록.
  - 오늘 토큰 사용량: `logs/agent.log`의 로컬 자정 이후 `model_call` 라인의 `total=` 합계.
- `src/main.rs`: 인수 없으면 텔레그램 모드, `--cli`면 기존 터미널 모드. 설정 로드 실패는 사유를 로그·stderr에 남기고 종료.
- `src/loop_.rs`: 상태 기계·멈춤 조건은 그대로 두고, 최종 답변을 stdout에 직접 쓰는 대신 `Result<String, String>`으로 반환하도록 출력 경계만 변경(CLI가 출력). 창구가 답변을 전달하기 위한 인터페이스 변경이며 로직 우회 아님.
- `src/loop_.rs` 로그에 모델명 추가: `model_call model=<name> ...` (비밀값 아님).

### 의존성
- `teloxide = { version = "0.17", default-features = false, features = ["rustls"] }` 추가. 기본 TLS(native-tls) 대신 이미 쓰는 rustls를 사용하고, webhook·저장소·매크로 기능은 켜지 않아 크기를 억제. 직접 의존성 6 → 7개.

### 검증 실측 결과
1. `cargo test`: 23개 전부 통과 (Block 4의 15개 회귀 없음 + brain 4개 + channel 4개).
2. 소유자 ID만 응답 / 다른 ID 무응답 + 로그: 봇 연결 후 실제 소유자 메시지 수신 1건을 확인. 로그는 `telegram_start → turn_start → model_call model=deepseek-chat → turn_end answer → telegram_reply_sent chunks=1` 순서로 기록됨. 다른 ID 무응답은 다른 계정 테스트를 생략해 **미완료**(코드 경로는 `telegram_ignore user_id=...` 로그로 구현됨).
3. 4,096자 초과 답변 분할: 단위 테스트로 9,000자 입력이 4,096자 이하 3개 조각으로 나뉘고 원문이 보존됨을 확인(`splits_long_messages_into_bounded_chunks`). 실제 텔레그램 다중 수신 테스트는 생략해 **미완료**.
4. 기본 설정 대화 + `LLM_MODEL` 교체:
   - 기본: CLI 1턴 성공, 로그 `model_call model=deepseek-chat ...`.
   - `LLM_MODEL=deepseek-reasoner`: CLI 1턴 성공, 로그 `model_call model=deepseek-reasoner ...`.
5. `cargo build`/`cargo build --release`: `iskra` 자체 경고 0개. 단, 전이 의존성 `proc-macro-error2 v2.0.1`(teloxide의 aquamarine 경유)에 대해 rustc 미래 호환성 안내가 1건 출력됨(우리 코드 경고 아님, 최신 2.0.1이 해당).

### Step 3 — 가벼움 예산 실측 (릴리스, 텔레그램 연결 후 유휴)
- 바이너리 크기(strip 후): **5,856,520 bytes = 5.86 MB (5.59 MiB)** — 기준 ≤ 20MB 통과.
- 유휴 RAM: 텔레그램 롱폴링 연결 대기 상태 RSS **14,288 KB = 13.95 MiB** — 기준 ≤ 30MB 통과.
- 직접 의존성 수: **7개** — 기준 ≤ 15 통과.
- 셋 다 기준 안이므로 중단 조건 미해당.

### 임시 우회
- 없음. 임의로 teloxide를 reqwest 직접 호출로 교체하지 않음.

### 비고
- 실제 소유자 메시지 수신·답변 전송은 확인했다. 다른 ID 무응답(기준 2의 비소유자 부분)과 4,096자 초과 실전송(기준 3)은 사용자가 테스트를 생략해 남은 항목이다. 코드·단위 테스트·릴리스 예산·기본/교체 모델 호출은 실행 결과로 확인했다.
- `logs/`, `memory/`, `workspace/`, `.env`는 계속 git 추적 제외.
