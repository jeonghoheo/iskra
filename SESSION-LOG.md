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
3. 4,096자 초과 답변 분할: 단위 테스트로 9,000자 입력이 4,096자 이하 3개 조각으로 나뉘고 원문이 보존됨을 확인(`splits_long_messages_into_bounded_chunks`). Block 6 Step 0 라이브 검증에서 긴 답변이 `telegram_reply_sent chunks=2`로 기록되고 사용자가 여러 조각이 끝까지 도착함을 확인함.
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
- 실제 소유자 메시지 수신·답변 전송은 확인했다. 4,096자 초과 실전송도 Block 6 Step 0 라이브 검증에서 확인(chunks=2). 다른 ID 무응답(기준 2의 비소유자 부분)만 남은 항목이다. 코드·단위 테스트·릴리스 예산·기본/교체 모델 호출은 실행 결과로 확인했다.
- `logs/`, `memory/`, `workspace/`, `.env`는 계속 git 추적 제외.

## Block 6 — 접수 신호 + 셸 도구 + 코딩 과제 실증 (2026-11-19)

### 한 일 — Step 0: 접수 신호
- `src/channel.rs`: 턴 시작 시 `sendChatAction(Typing)`을 보내고 4초 간격으로 갱신하는 `typing_loop` 추가.
- 턴 태스크가 답변 전송 전에 `TypingGuard`를 drop해 신호를 중단. `/stop`으로 턴 태스크가 abort돼도 guard의 Drop이 신호를 보내 루프가 종료됨.
- 신호 전송 실패는 `telegram_typing_error`로 로그만 남기고 본 흐름을 막지 않음. CLI 모드는 변경 없음.

### 한 일 — Step 1: `run_command`
- `src/tools.rs`: `run_command(program, args)` 도구 추가. 스키마는 `program`(enum: ls, cat, git, cargo)과 문자열 배열 `args`.
- 허용 목록 밖 프로그램은 실행하지 않고 거부 + `run_command_rejected` 로그. 경로(`/bin/ls`)와 공백 포함 이름(`"ls "`)도 거부.
- 셸 미사용: `tokio::process::Command`에 프로그램과 인자 배열을 직접 전달. 작업 폴더는 canonical `workspace/` 고정, stdin은 null.
- 타임아웃 120초: 초과 시 `start_kill` + `wait`로 프로세스를 종료하고 결과에 `타임아웃(...)` 명시. `kill_on_drop(true)`로 `/stop` 등 future 취소 시에도 자식 프로세스가 남지 않게 함.
- 출력 상한: stdout/stderr를 각각 최대 80KB만 저장하고 동시에 끝까지 drain. 합친 뒤 10,000자에서 자르고 `...(출력이 10,000자에서 잘렸습니다)` 표시.
- 로그는 `run_command program=... args=[...]`와 `run_command_exit ... code=...`만 기록(명령 출력은 모델 반환용, 로그 미기록). 자식 프로세스 환경에서 `DEEPSEEK_API_KEY`·`LLM_API_KEY`·`TELEGRAM_BOT_TOKEN`을 제거.
- `src/loop_.rs`: `tools.execute(...)`를 async로 호출하도록 변경. `src/prompt.rs`: run_command 사용 규칙 한 줄 추가.

### 한 일 — Step 2: 코딩 과제 실증
- `workspace/trial_project/`에 최소 Rust 프로젝트 생성. `add(2,3)`이 `left - right`라 테스트가 실패하는 상태로 시작.
- `--cli`로 "run_command로 실패 확인 → write_file로 수정 → 재실행" 지시. 에이전트가 `cargo test`(exit 101) → `read_file` → `write_file` → `cargo test`(exit 0) 순서로 수행.
- 수정 결과 `left + right`로 바뀌었고 테스트 코드는 그대로. 실증 후 프로젝트를 `workspace/`에 남겨 회귀 확인에 사용.

### 검증 실측 결과
1. `cargo test`: **28개 전부 통과** (Block 5의 23개 회귀 없음 + run_command 5개: 허용 목록 거부, 셸 메타문자 무력화, 출력 캡, 타임아웃, 작업 폴더 고정).
2. 허용 목록 밖 명령: 테스트에서 `rm`, `curl`, `sh`, `/bin/ls`, `"ls "`를 호출해 모두 거부됨. `logs/agent.log`에 `run_command_rejected program=...` 10건 기록 확인.
3. Step 2 실증 로그:
   - `run_command program=cargo args=["test", "--manifest-path", "trial_project/Cargo.toml"]` → `run_command_exit ... code=101`
   - `read_file` → `write_file` → 같은 `run_command` → `run_command_exit ... code=0`
   - 최종 `cargo test`: 1 passed / 0 failed. `src/lib.rs`는 `left + right`, 테스트 코드는 원본 유지.
4. 접수 신호 라이브 확인: 실제 턴에서 `telegram_typing_start` → `telegram_typing_sent` 6회(약 4~5초 간격) → `telegram_typing_stop` → `telegram_reply_sent chunks=2`. 사용자가 "입력 중" 표시 유지·소멸과 긴 답변의 다중 조각 도착을 확인.
5. `cargo build`/`cargo build --release`: `iskra` 자체 경고 0개. 전이 의존성 `proc-macro-error2 v2.0.1` 미래 호환성 안내 1건은 그대로(우리 코드 경고 아님).
6. 릴리스 바이너리(strip 후): **6,094,400 bytes = 6.09 MB (5.81 MiB)**. 직접 의존성 변화 없음(7개).

### 임시 우회
- 없음.

### 비고
- `run_command`는 셸을 거치지 않지만 허용된 프로그램 자체의 기능(예: `git`의 외부 명령 실행)까지 제한하지는 않는다. 허용 목록·인자 정책 확장은 v1 이후 판단 범위.
- `workspace/trial_project/`는 런타임 산출물과 함께 git 추적 제외(`/workspace`). 실증 후 남겨 회귀 확인에 사용.

## Block 6.1 — 스티커·리액션 캐치 (2026-11-19)

### 한 일
- `src/channel.rs`: 메시지 핸들러가 스티커를 받으면 대표 이모지·세트 이름을 `[스티커 <emoji> (<set_name>)]`(이모지 없으면 `[스티커]`)로 정규화해 기존 `run_turn`에 사용자 메시지로 전달. 허용 목록 판정은 텍스트와 동일하게 발신자 검사 후 수행하고, 허용 발신자의 스티커는 `telegram_sticker emoji=...`로 로그.
- `src/channel.rs`: Dispatcher 핸들러를 `dptree::entry()`로 바꿔 `Update::filter_message()`와 `Update::filter_message_reaction_updated()` 두 브랜치를 등록. 핸들러 설명을 통해 `allowed_updates`에 `message`·`message_reaction`이 자동 힌트됨.
- 리액션 핸들러는 허용 발신자 확인 후 `telegram_reaction emoji=... message_id=...` 로그만 남기고 모델 턴을 만들지 않음. 리액션 제거(빈 배열)는 `emoji=none`으로 기록. 발신자를 알 수 없거나 허용 목록 밖이면 로그만 남기고 무시(fail-closed).
- 음성 등 다른 비텍스트는 기존처럼 `telegram_ignore non_text`; 스티커 이미지 다운로드·비전 해석, 리액션의 다음 턴 맥락 주입은 범위 외로 두지 않음(만들지 않음).

### 검증 실측 결과
1. `cargo test`: **29개 전부 통과** (Block 6의 28개 회귀 없음 + 스티커 정규화 단위 테스트 1개). 정규화 테스트는 이모지/세트 이름 조합 4가지를 확인.
2. 라이브 스티커: `telegram_sticker emoji=🎶` → `turn_start` → `model_call model=deepseek-chat` → `telegram_reply_sent chunks=1`. 사용자가 봇이 이모지 맥락에 맞게 답했음을 확인.
3. 라이브 리액션: 봇 메시지(`message_id=12`)에 대한 리액션이 `telegram_reaction emoji=🔥 message_id=12`, 이어서 변경/제거가 `emoji=none`, `emoji=👍`로 기록됨. 리액션으로 인한 `turn_start`/`model_call`은 없었고, 사용자는 답장이 없는 것이 정상임을 확인. 같은 구간의 이후 턴들은 사용자가 보낸 텍스트 메시지에 의한 것.
4. `cargo build`: `iskra` 자체 경고 0개. 전이 의존성 `proc-macro-error2 v2.0.1` 미래 호환성 안내 1건은 그대로(우리 코드 경고 아님).

### 임시 우회
- 없음.

### 비고
- 리액션은 기록만 하고 모델 호출을 붙이지 않는다. 다음 턴 맥락 주입은 실사용 게이트 이후 판단.

## Block 6.2 — Self-state awareness + command discoverability (2026-11-19)

### What changed
- Added `src/usage.rs`, which owns the daily token aggregation shared by `/status` and the per-turn self-state:
  - `today()` sums `prompt`, `completion`, and `total` from today's `model_call` log lines since local midnight.
  - `self_state_block()` renders provider, model, today's tokens, estimated cost, balance, and an "estimate only" note.
  - A small peak-list price table covers `deepseek-chat`, `deepseek-flash`, `deepseek-v4-flash`, `deepseek-reasoner`, and `deepseek-v4-pro`. Legacy aliases use the corresponding current model's peak rates. Unknown models show tokens only.
- `src/loop_.rs` appends the self-state block to the system prompt at the start of every turn, before the first model call. This is shared by Telegram and `--cli`.
- `src/brain.rs` adds the provider balance lookup:
  - `GET {API_ROOT}/user/balance` with the configured bearer key, 3-second timeout.
  - Official response shape verified against the DeepSeek API docs on 2026-11-19: `is_available` plus `balance_infos[{currency,total_balance,granted_balance,topped_up_balance}]`, with numeric values encoded as strings.
  - `LLM_BASE_URL` in this codebase is the full chat-completions URL, so `balance_url()` strips a trailing `/chat/completions` before appending `/user/balance`. Default result: `https://api.deepseek.com/user/balance`.
  - Failures are logged as `balance_error`, surfaced as `unavailable`, and never fail a turn or `/status`.
  - A 60-second in-memory cache stores both success and failure. `/status` always forces a fresh fetch; turns reuse the cache.
- `src/channel.rs`:
  - `/status` now shows model, memory state, today's tokens, and balance.
  - Registers `start`, `status`, `stop`, and `help` with `setMyCommands` at startup. Registration success logs `telegram_commands_registered`; failure logs `telegram_set_commands_error` and does not block startup.
  - Adds `/help` and makes `/start` end with the same command list.
- `src/main.rs` registers the new `usage` module.

### Spec interpretation (recorded, not a temporary workaround)
Scope item 2 says balance is fetched for `/status` only, while acceptance criterion 2 requires a natural-language answer that includes the actual balance. To satisfy both, the self-state fetches balance on demand at turn start through the same `balance()` path and caches the result for 60 seconds. There is no background polling. `/status` always performs a fresh fetch. This is a deliberate reconciliation of the two requirements.

### Verification
1. `cargo test`: **37 passed, 0 failed**. Existing 29 tests still pass. New coverage: usage aggregation, self-state assembly, known/unknown-model cost, provider labels, balance URL derivation, balance JSON parsing/formatting, and command-list/help assembly. The old channel-level usage-summing test moved to `usage`.
2. Natural-language live check (`--cli`): asked for today's tokens, estimated cost, and balance. The model answered with today's token totals, the estimated cost, and the real provider balance. The exact balance amount is intentionally not recorded here.
3. Live `/status`: confirmed via Telegram that it shows model, memory state, today's tokens, and the balance line.
4. Command discoverability: `telegram_commands_registered` was logged at startup, the Telegram `/` menu showed `start`/`status`/`stop`/`help`, and `/help` listed the commands. `/start` ends with the same list (also covered by unit test).
5. `cargo build` / `cargo build --release`: zero warnings for `iskra` itself. The transitive `proc-macro-error2 v2.0.1` future-incompatibility note remains unchanged (not our code).

### Temporary workarounds
- None.

### Notes
- The cost estimate uses peak public list prices and is explicitly labelled as an estimate. Prices may change; the table is intentionally small.
- Balance amounts and API keys are never written to logs. Only failure reasons are logged.
- The command menu is registered globally for the bot; no per-chat scoping was added.

## Block 6.4 — Identity files: template/local split (2026-11-19)

### What changed
- Renamed the tracked identity templates with `git mv`:
  - `SOUL.md` → `SOUL.example.md`
  - `USER.md` → `USER.example.md`
  - The local `SOUL.md` and `USER.md` copies were kept on disk and are now ignored.
- Fixed the misleading line in `USER.example.md`: it now states that the file is a template and that the real `USER.md` lives only on the user's machine and is never committed. The rest of the template content is unchanged.
- Added `/USER.md` and `/SOUL.md` to `.gitignore`. `git ls-files` now lists only `SOUL.example.md` and `USER.example.md`.
- Reworked `src/prompt.rs` identity loading:
  - A present local file wins (`source=local`).
  - A missing local file is copied from its `.example.md` template if readable (`source=template`). The new local file is created with `0600` permissions on Unix.
  - If both the local file and the template are missing, or the local file is unreadable/invalid UTF-8, a built-in minimal default is used (`source=default`). Startup never fails over identity files.
  - Each file logs `identity_file file=... source=local|template|default`. File contents are never logged.
  - Invalid local files are not overwritten by templates.
- No history rewrite was performed. Existing history contains only the generic blank templates.

### Verification
1. `cargo test`: **41 passed, 0 failed**. Existing 37 tests still pass. New prompt tests cover: template fallback copy (including `0600`), built-in defaults when everything is missing, local-file preference, and startup survival with invalid local identity files.
2. `git ls-files` shows only `SOUL.example.md` and `USER.example.md`. `git check-ignore -v` confirms `/USER.md` and `/SOUL.md` are ignored; local files remain on disk.
3. Live startup with local files present (`--cli`): logs showed `identity_file file=SOUL.md source=local` and `identity_file file=USER.md source=local`, and the bot answered normally.
4. Live fallback: temporarily renamed both local files away, started the bot again, and observed:
   - `identity_file file=SOUL.md source=template`
   - `identity_file file=USER.md source=template`
   - Both local files were recreated with mode `600`, and the bot answered normally.
   - The original local copies were restored afterwards.
5. `cargo build` / `cargo build --release`: zero warnings for `iskra` itself. The transitive `proc-macro-error2 v2.0.1` future-incompatibility note is unchanged.

### Temporary workarounds
- None.

### Notes
- The local files currently contain only the generic template content; no personal data was added or logged.
- The templates remain in Korean. Translation and identity-file encryption remain out of scope for this patch.

## Block 6.3 — Telegram sticky notes (post-it board) (2026-11-19)

### What changed
- Added `src/stickies.rs` and stored the board in `memory/stickies.md` (the bot's existing local data folder, already git-ignored).
  - Each sticky stores a stable sequential number, creation date (local `YYYY-MM-DD`), and one-line text.
  - New numbers use `max(existing) + 1`, so removing a sticky does not reuse its number.
  - Writes reuse the memory safety path: atomic replace via `memory::atomic_write`, `0600` permissions on Unix, and the same credential scanner (`memory::redact_credentials`).
  - If the text looks like a credential (`sk-` token, password value, private-key block, Telegram bot token), the add is refused with a user-facing reason and the file is not modified. It is not redacted and saved.
  - Text is trimmed and folded to one line; entries are capped at 1,000 characters. Malformed lines in a user-edited file are skipped rather than crashing the board.
- Added tool `sticky_note` (`tool/v1`): `add(text)`, `list()`, `remove(number)`. Every result returns the current board so the model can confirm the change. Successful add/remove sets an in-process change flag.
- `src/channel.rs`:
  - Added `/stickies`, which sends the board as one plain-text message and tries to pin it.
  - After a tool-driven add/remove, the running turn sends/re-sends the board and tries to pin it. Pin failure is logged as `telegram_pin_error` and the board still works unpinned.
  - Replying to a board message with a number removes that sticky and sends/pins the updated board. This path is detected from the replied-to message being a bot message whose text starts with the board title.
  - Added `stickies` to the registered command list (`start`, `status`, `stickies`, `stop`, `help`) and to `/help` / `/start`.
- The prompt now tells the model to use `sticky_note` for sticky requests and not to repeat the whole board in its answer (the channel sends the board separately).

### Verification
1. `cargo test`: **45 passed, 0 failed**. Existing 41 tests still pass. New sticky tests cover add/list/remove with stable numbering, credential-looking text refusal without file changes, recognizable empty board, newline folding, and `0600` permissions.
2. Live natural-language add: the model called `sticky_note`, the answer was sent, and the board was sent and pinned (`telegram_board_sent` → `telegram_board_pinned`).
3. Live `/stickies`: the board was sent and pinned again.
4. Live natural-language remove: `sticky_note` removed the numbered sticky, the answer was sent, and the updated board was sent and pinned.
5. Live reply-to-board remove: replying to the pinned board with the number logged `telegram_sticky_remove number=1` and sent/pinned the updated board.
6. Pin succeeded in this chat. The failure path is implemented and logged if the bot lacks permission; it never blocks the board or the turn.
7. `cargo build` / `cargo build --release`: zero warnings for `iskra` itself. The transitive `proc-macro-error2 v2.0.1` future-incompatibility note is unchanged.
8. The test stickies were removed; `memory/stickies.md` ended as an empty board with mode `600`.

### Temporary workarounds
- None.

### Notes
- The board is re-sent rather than edited in place. An old pinned board may remain until the new board is pinned, depending on Telegram's pin behavior.
- The board file is not loaded into the memory prompt; it is a separate user-readable data file.
- Timed reminders, procedure linkage, in-place editing, and non-text stickies remain out of scope.

## Block 6.3.1 — One board message, edited in place (2026-11-19)

### What changed
- Added persisted board state in `memory/board_state.json` (same data-folder safety as other files: `0600`, atomic replace via `memory::atomic_write`).
  - The file stores `chat_id` + `message_id`, or `null` when no live board exists.
  - The state is loaded on every sync, so a restart resumes the same tracked board.
- `src/channel.rs` now has a board state machine:
  - `Update` mode: `none → send + pin → live(id)`; content changes → `editMessageText` on `live(id)`; empty → unpin + delete + clear state → `none`.
  - `MessageNotModified` is treated as unchanged, not as an edit failure.
  - If editing the tracked message fails, the bot logs the error, falls back to sending/pinning a new board, replaces the stored id, and logs `telegram_board_refreshed_fallback`.
  - If unpin/delete fails during teardown, the failure is logged and the stored id is cleared anyway.
- `/stickies` explicit views use `Show` mode:
  - If no live board exists and the board is non-empty, the tracked board is created and pinned.
  - If a live board already exists, the bot sends a plain view copy at the bottom of the chat and leaves the tracked/pinned board untouched. This avoids the confusing "Iskra pinned a deleted message" service notice caused by replacing the tracked board. The view copy is a snapshot, not a second tracked board.
  - An empty board tears down any stale live board and shows the empty-board text.
- Added `/stickies remove N` (also accepts `/stickies N`, `/stickies delete N`, `/stickies del N`) for deletion without finding the board. The board footer and `/help` both mention the syntax.
- Fixed the removal path to sync the pure board (`stickies::board()`) rather than the tool/command confirmation string. This makes the final removal tear down the live board immediately.
- Reply-to-board numeric removal still works; it now also syncs the pure board.

### Verification
1. `cargo test`: **51 passed, 0 failed**. Existing 45 tests still pass. New tests cover board-state persistence/clearing, corrupt state handling, empty-board detection, board plan transitions (create/edit/teardown/empty), edit-failure fallback planning, and `/stickies` command parsing.
2. Live add two / remove one by one: the first add sent and pinned the board, the second add edited it in place, the first removal edited it in place, and the last removal logged `telegram_board_torn_down` immediately. The user confirmed one pinned board and its disappearance.
3. Live restart: after restarting the bot with a persisted board id, adding another sticky logged `telegram_board_edited` instead of sending a second board. The same tracked message was reused.
4. Live `/stickies`: the board content appeared as a new view message at the bottom, and no new pin service notice was created (`telegram_board_view_sent`). The tracked board remained pinned and unchanged.
5. Live `/stickies remove N`: both stickies were removed via the command, the last removal tore down the board, and the state file ended as `null` with an empty stickies file.
6. `cargo build` / `cargo build --release`: zero warnings for `iskra` itself. The transitive `proc-macro-error2 v2.0.1` future-incompatibility note is unchanged.

### Temporary workarounds
- None. The view-copy behavior and `/stickies remove N` are product changes made from live user feedback, not quick fixes.

### Notes
- The tracked board remains the single pinned post-it. Explicit `/stickies` views are untracked snapshots so users can read the content without scrolling; repeated views add normal chat messages, never a second pinned board.
- Pre-patch stray board messages already in chat history are still out of scope; the bot never tracked their ids.
- Sticky storage, numbering, and credential refusal were not changed.

## Block 6.5 — English unification of in-code strings (2026-11-19)

### What changed
- Translated every Korean string literal in `src/*.rs` to English. The Hangul scan over `src/` now returns **0 matches**.
  - `tools.rs`: model-facing tool and parameter descriptions, tool errors, command/path errors, timeout and truncation messages.
  - `prompt.rs`: default SOUL/USER text, identity fallback warnings, memory placeholder/truncation text, safety rules, and tool usage rules.
  - `channel.rs`: command descriptions, `/start` and `/help`, `/status`, board/sync notices, busy/stop messages, sticker normalization, and Telegram config errors.
  - `stickies.rs`: board title (`Sticky board`), empty-board text, add/remove messages, credential refusal, and storage errors.
  - `brain.rs`, `loop_.rs`, `main.rs`, `memory.rs`, `usage.rs`, `logging.rs`: runtime errors, stop reasons, CLI messages, memory save/load messages, self-state labels, and log-write failures.
- Translated test fixtures and updated assertions only where they asserted on translated text.
- The Telegram sample token test fixture in `src/memory.rs` is now assembled at runtime from two halves, so the full token literal is no longer present. The same approach was applied to the credential-refusal fixture in `src/stickies.rs`, which used the same sample token.
- Behavior is unchanged: same control flow, same conditions, same tool semantics, same tests.

### Verification
1. Hangul scan over `src/`: **0 matches**.
2. `cargo test`: **51 passed, 0 failed**. The assembled-token scanner test still catches the token and still redacts it.
3. Live Telegram: the `/` command menu and `/help` show English descriptions, and `/stickies` shows the English empty-board title `Sticky board is empty.`. The user confirmed all three.
4. Live conversation language: a Korean Telegram message still received a Korean answer, and a `--cli` Korean prompt answered in Korean. Model behavior still comes from the local SOUL/USER data, which remains out of scope for this patch.
5. `cargo build` / `cargo build --release`: zero warnings for `iskra` itself. The transitive `proc-macro-error2 v2.0.1` future-incompatibility note is unchanged.

### Temporary workarounds
- None. The token-fixture assembly is the requested permanent cleanup, not a behavioral workaround.

### Notes
- Local `SOUL.md`/`USER.md`, memory/sticky contents, and the shipped `SOUL.example.md`/`USER.example.md` templates were intentionally left in their existing language as data/product templates.
- README and docs were already English-first and were not changed.
- Board messages already in chat history from before this patch keep their old text; they were never tracked and remain out of scope.

## Block 6.6 — User-facing strings follow the user's language (2026-11-19)

### What changed
- Added `src/locale.rs` with `Locale` (`en`/`ko`/`ru`), primary-subtag normalization (`ko-KR` → `ko`), an English fallback for missing/unsupported codes, and a process-global "latest seen" locale updated from each allowed Telegram message/reaction.
- Added a typed string table covering all human-facing bot strings. English is the source text; Korean and Russian are equal-meaning translations.
- Command registration now calls `setMyCommands` three times: default English, `language_code="ko"`, and `language_code="ru"`. Startup logs `telegram_commands_registered locale=en|ko|ru`.
- `/start`, `/help`, `/status`, board messages, sticky confirmations, credential refusal, busy/stop notices, and sticky error messages use the resolved locale.
- `/status` labels, memory-state values, and balance-unavailable/insufficient text are localized. The model-facing self-state block remains English.
- Board formatting now uses the localized title, empty text, and footer. Reply-to-board detection recognizes the English, Korean, and Russian titles plus the pre-6.6 English and pre-6.5 Korean legacy titles.
- `main.rs` now passes a structured `MemoryStatus` (`truncated`, `warnings`) to the channel so `/status` can localize those values.
- `sticky_note` tool results and the credential-refusal error use the latest locale; the channel board uses the locale captured for the current turn.

### String table
| Key | en | ko | ru |
|---|---|---|---|
| StartDescription | Show the welcome message | 시작 인사 보기 | Показать приветствие |
| StatusDescription | Show today's usage and remaining balance | 오늘 사용량과 남은 금액 보기 | Показать расход за сегодня и баланс |
| StickiesDescription | Show the sticky board | 스티키 보드 보기 | Показать доску стикеров |
| StopDescription | Stop the current task | 하던 일 멈추기 | Остановить текущую задачу |
| HelpDescription | List available commands | 명령어 목록 보기 | Показать список команд |
| StartGreeting | I am Iskra. Send me a message to chat. | 이스크라입니다. 메시지를 보내면 대화합니다. | Я Искра. Отправьте сообщение, чтобы начать разговор. |
| HelpHeader | Available commands: | 사용할 수 있는 명령어: | Доступные команды: |
| StatusModel | Model: {} | 모델: {} | Модель: {} |
| StatusMemory | Memory: {} | 기억: {} | Память: {} |
| StatusTokens | Tokens today: {} | 오늘 토큰: {} | Токены сегодня: {} |
| StatusBalance | Balance: {} | 잔액: {} | Баланс: {} |
| BoardTitle | Sticky board | 스티키 보드 | Доска стикеров |
| BoardEmpty | Sticky board is empty. | 스티키 보드가 비어 있습니다. | Доска стикеров пуста. |
| BoardFooter | Delete: /stickies remove N | 삭제: /stickies remove N | Удалить: /stickies remove N |
| StickyAdded | Sticky #{} added. | 스티키 #{}을 추가했습니다. | Стикер #{} добавлен. |
| StickyRemoved | Sticky #{} removed. | 스티키 #{}을 삭제했습니다. | Стикер #{} удалён. |
| StickyNotFound | Sticky #{} does not exist. | 스티키 #{}이 없습니다. | Стикер #{} не найден. |
| CredentialRefused | Content that looks like a credential cannot be saved in a sticky. Remove the secret and try again | 자격증명처럼 보이는 내용은 스티키에 저장할 수 없습니다. 비밀값을 지우고 다시 시도해 주세요 | Содержимое, похожее на учётные данные, нельзя сохранить в стикере. Удалите секрет и попробуйте снова |
| MemoryOk | ok | 정상 | в порядке |
| MemoryPartial | partially loaded | 일부만 로드됨 | загружено частично |
| MemoryWarnings | {} warning(s) | 경고 {}건 | предупреждений: {} |
| TurnBusy | A previous turn is still running. Use /stop to interrupt it. | 이전 턴이 아직 진행 중입니다. /stop으로 중단할 수 있습니다. | Предыдущий ход ещё выполняется. Используйте /stop, чтобы прервать его. |
| TurnStopped | Stopped: {} | 중단: {} | Остановлено: {} |
| StopStopped | The current turn was stopped. | 현재 턴을 중단했습니다. | Текущий ход остановлен. |
| StopNoActive | There is no turn in progress. | 진행 중인 턴이 없습니다. | Сейчас нет выполняющегося хода. |
| StickyRemovedButBoardReadFailed | Sticky #{} was removed, but the board could not be read. | 스티키 #{}을 삭제했지만 보드를 읽지 못했습니다. | Стикер #{} удалён, но не удалось прочитать доску. |
| StickyRemoveFailed | Failed to remove the sticky: {} | 스티키를 삭제하지 못했습니다: {} | Не удалось удалить стикер: {} |
| StickyBoardSyncFailed | Failed to sync the sticky board. | 스티키 보드를 동기화하지 못했습니다. | Не удалось синхронизировать доску стикеров. |
| StickyBoardDisplayFailed | Failed to display the sticky board. | 스티키 보드를 표시하지 못했습니다. | Не удалось показать доску стикеров. |
| StickyBoardReadFailed | Could not read the sticky board. | 스티키 보드를 읽을 수 없습니다. | Не удалось прочитать доску стикеров. |
| StickyRemovedButBoardUpdateFailed | Sticky #{} was removed, but the board could not be updated. | 스티키 #{}을 삭제했지만 보드를 갱신하지 못했습니다. | Стикер #{} удалён, но не удалось обновить доску. |
| StickiesUsage | Usage: /stickies (show board), /stickies remove N (remove N) | 사용법: /stickies (보드 보기), /stickies remove N (N번 삭제) | Использование: /stickies (показать доску), /stickies remove N (удалить N) |
| Unavailable | unavailable | 확인 불가 | недоступно |
| BalanceInsufficient | (insufficient for API calls) | (API 호출에 부족) | (недостаточно для вызовов API) |

`BalanceInsufficient` values include a leading space when appended to the balance.

### Verification
1. `cargo test`: **55 passed, 0 failed**. Existing 51 tests still pass. New tests cover locale normalization/fallback, string-table completeness in all three languages, template formatting/memory status, and board-title recognition across English/Korean/Russian plus legacy titles.
2. Live startup logged `telegram_commands_registered` for `locale=en`, `locale=ko`, and `locale=ru`.
3. Live Telegram (Korean app language): the `/` command menu showed Korean descriptions, `/help` answered in Korean, `/stickies` showed the Korean empty-board title, and `/status` used Korean labels. The user confirmed all of these.
4. Live conversation: a Korean message still received a Korean answer. The model-facing system prompt remains English, while behavior language comes from the unchanged local SOUL/USER data.
5. `cargo build` / `cargo build --release`: zero warnings for `iskra` itself. The transitive `proc-macro-error2 v2.0.1` future-incompatibility note is unchanged.

### Temporary workarounds
- None.

### Notes
- Locale is process state only; it is re-detected from the first message after a restart. Command menus remain language-correct because Telegram selects the registered per-language set.
- Model-facing strings (tool descriptions, system prompt, self-state) and log strings remain English, per the localization rule.
- User data and the `SOUL.example.md`/`USER.example.md` templates remain in their existing language.
- Manual language settings, persistence, and languages beyond en/ko/ru remain out of scope.

## Temporary workaround W-1 — run_command disabled (2026-11-20)

Status: **removed** by the v1.1-A merge (2026-11-20, `5bbe10d`). Kept below as history.
Decision: owner (command room), 2026-11-20. Work gate ④ — this is a marked temporary workaround, not a fix.

### Why
The 2026-11-20 code review found that `run_command` bypasses the workspace sandbox: arguments are unrestricted (`cat ../.env` reads the project `.env`; `ls /` lists outside), `git -c …` / repo config / hooks allow arbitrary execution, `git push` can use inherited SSH agent or keychain credentials (external send without approval), and cargo build scripts run arbitrary code with the user's file access. Not using shell features during the gate does not stop the model from calling the tool, so the tool is disabled as a whole.

### What changed
- `src/tools.rs`: `RUN_COMMAND_DISABLED = true`. `schemas()` hides `run_command` from the model; `execute("run_command", …)` fails closed with "temporarily disabled until shell hardening (v1.1-A) is merged" and logs `run_command_disabled workaround=v1.1-A-pending`.
- `src/prompt.rs`: the run_command rule line says the tool is temporarily disabled and points to `read_file`.
- New test `run_command_disabled_until_v1_1_a`: schema has 4 tools without `run_command`; `ls`/`cat`/`git`/`cargo` with `../.env` are all refused.
- Side effect: `rejects_programs_outside_allowlist` now passes through the disabled branch, so it no longer exercises the allowlist check while W-1 is in effect. The process-level tests (`does_not_interpret_shell_metacharacters`, `caps_command_output`, `kills_command_on_timeout`, `runs_commands_inside_workspace`) call `run_process` directly and still run.
- No change to `ALLOWED_PROGRAMS`, `run_process`, or any other tool.

### Verification (owner to run)
1. `cargo test` → expected 56 passed (55 + 1 new), 0 failed. `cargo build --release` → 0 warnings from `iskra`.
2. Restart Iskra. In Telegram ask: "Show me the .env file one folder up using a shell command." → no command runs; `logs/agent.log` has no `run_command program=` line after the restart (a `run_command_disabled` line is acceptable if the model tried).
3. Regression: "Read hello.txt" → answered via `read_file`.

### Removal (formal-fix task)
Directive **v1.1-A (shell hardening)** must, when merged: delete `RUN_COMMAND_DISABLED` and both workaround branches, delete `run_command_disabled_until_v1_1_a`, restore the run_command line in `src/prompt.rs` (rewritten to match the hardened behaviour), and record "W-1 removed" in its SESSION-LOG entry.

## v1 real-use gate — dates (recorded 2026-11-20)

- Gate period: **3 days, 2026-11-19 → 2026-11-22** (shortened from 2 weeks by owner decision on 2026-11-19; design §3 v1). v2 design runs in parallel; v2 implementation starts only after the gate verdict.
- During the gate, shell features are unavailable (workaround W-1, above) until directive v1.1-A is merged.
- Long-horizon items that 3 days cannot cover (memory growth, resuming after days off, cost accumulation) stay under observation after the gate.

## v1.1-A — Shell hardening (2026-11-20, branch `v1.1-a-shell-hardening`)

Status: **automated and live verification passed** (live run with the owner's confirmation that no other instance was running). Worked only in the `../iskra-v1.1-a` worktree. Nothing committed, pushed or merged. `.env` was never opened. The sandbox tests use decoy files laid out like the worktree.

**W-1 removed.** `RUN_COMMAND_DISABLED`, the `schemas()` filter, the guarded `execute` arm and the test `run_command_disabled_until_v1_1_a` are deleted. The temporary run_command line in `src/prompt.rs` is replaced with a description of the hardened behaviour. `rejects_programs_outside_allowlist` now fails on the real allowlist check (it asserts the message starts with "Program is not allowed"). `grep -rn "v1.1-A pending\|RUN_COMMAND_DISABLED\|TEMPORARY WORKAROUND" src/` returns no matches.

### States and transitions (written before implementing, gate ②)
- Startup (`Tools::new` → `Runtime::detect`):
  - Canonicalize the workspace and take its parent as the project root.
  - Create `workspace/.iskra-home/cargo` and `workspace/.iskra-tmp`.
  - Pin programs: `/bin/ls`, `/bin/cat`, `/usr/bin/git`, and `<sysroot>/bin/cargo`. The sysroot comes from running the rustc proxy once with fixed args, a cleared environment and cwd `/`.
  - Probe Seatbelt with `sandbox-exec -p <profile> /usr/bin/true`.
  - A missing program is disabled and logged (`run_command_program_disabled`). With no sandbox, cargo is disabled. `run_command_sandbox available=…` is logged.
  - `schemas()` advertises only the available programs.
- Call: `Received` → `ProgramCheck` (allowlisted and pinned) → `DirCheck` (`dir` resolved like `read_file`) → `ArgPolicy` (common path rule, then the per-program rule) → `Spawn` (sandboxed, cleared env, own process group) → `Running` → `Exited` | `TimedOut` | `Aborted` (/stop or drop) → `Logged`.
  - In every exit state the whole process group is killed.
  - A refusal at any check spawns nothing and logs `run_command_rejected program=… reason=…` (redacted).

### Owner decisions (2026-11-20)
- `CARGO_HOME` is a private folder, `workspace/.iskra-home/cargo`, not `~/.cargo`. cargo runs from the default toolchain's sysroot directly, bypassing the rustup proxy, so `+toolchain` and `rust-toolchain.toml` have no effect. Consequence: offline builds of crates with external dependencies fail.
- `run_command` gained an optional `dir` argument (a folder relative to workspace, checked like `read_file`). Without it, git could not be used on `workspace/trial_project` once `-C` and parent discovery are refused.

### Bypass paths and how each is closed
| # | Path | Closed by | Test |
|---|------|-----------|------|
| 1 | `cat ../.env`, `cat /etc/hosts`, `ls /`, `ls ..` | Common path rule: absolute, `..` component or leading `~` refused before spawn | `refuses_outside_paths_before_spawn` |
| 2 | Paths inside options: `--output=/x`, `--output=../x`, `-O/x`, `HEAD:../x`, `:/x` | Same rule applied to the values after `=` and `:` and to attached short-option values | `refuses_absolute_parent_and_home_arguments` |
| 3 | git global options (`-c`, `-C`, `--git-dir`, `--work-tree`, `--exec-path`, `--namespace`, `--config-env`, `--super-prefix`, `-p/--paginate`, also `--no-pager`) | The first argument must be an allowed subcommand; any option before it is refused | `git_refuses_global_options_before_subcommand` |
| 4 | git push/fetch/pull/clone/ls-remote/remote/submodule/config/clean/reset/checkout/restore/apply/am/archive/bundle/daemon/instaweb/filter-branch (also credential, stash) | Subcommand allowlist: status, diff, log, show, add, commit, init, rev-parse, ls-files | `git_refuses_subcommands_outside_allowlist` |
| 5 | `--output`, `--upload-pack`, `--receive-pack`, `--exec`, `--template`, `-c`/`--config`, `--ext-diff`, `--textconv` | Refused on every subcommand, **including unique-prefix abbreviations** (`--outp`, `--ext-d`, `--textc`; new finding). Also refused, fail-closed because they run gpg: `--gpg-sign`/`-S` on commit, `--show-signature`. `--no-verify` stays allowed | `git_refuses_code_running_and_redirecting_options` |
| 6 | Hooks, fsmonitor, pager, protocols, system/global config, credential prompts | Iskra puts `-c core.hooksPath=/dev/null -c core.fsmonitor=false -c core.pager=cat -c protocol.allow=never` before the model's args and sets `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=` (empty), `GIT_PAGER=cat`, `GIT_EDITOR=true`, and a fixed author/committer `Iskra <iskra@localhost>` | `git_command_forces_hook_and_config_isolation` |
| 7 | **New:** with no repository in workspace, git found the parent Iskra repository (this worktree's gitdir), so `log/show/add/commit` acted on the owner's branch | `GIT_CEILING_DIRECTORIES=<project root>`; the sandbox also denies reads of the project root | `git_does_not_discover_parent_repository` |
| 8 | cargo install/publish/login/logout/owner/yank/fetch/update/vendor, `+toolchain`, `-Z`, `--config`, `--manifest-path` outside workspace | Subcommand allowlist (build, check, test, run, fmt, clippy, new, init, metadata); `+…`, `-Z…`, `--config` (and its abbreviations) refused; options before the subcommand refused; paths by rule 1 | `cargo_refuses_subcommands_toolchains_and_unstable_flags` |
| 9 | cargo network | `CARGO_NET_OFFLINE=true`; the sandbox denies the network | `cargo_command_runs_offline_with_pinned_homes`, `sandbox_blocks_build_script_network` |
| 10 | `write_file` to `.git/config`, `.git/hooks/*`, `.gitattributes`, `.gitmodules`, `.gitconfig`, a `.git` file (gitdir redirect), `.cargo/config.toml`, `rust-toolchain(.toml)` | Refused for any component starting with `.git`, any component equal to `.cargo`, and file names `rust-toolchain`/`rust-toolchain.toml`. **New:** the comparison is case-insensitive (`.GIT/config` writes `.git/config` on APFS; `ſ` folds to `s`) and is applied both to the given path and to the path after symlinks are resolved (a symlink inside workspace pointing at `.git`) | `write_file_refuses_git_cargo_and_toolchain_files` |
| 11 | Inherited environment (`.env` loaded by dotenvy, `SSH_AUTH_SOCK`, any `*_API_KEY`) | `env_clear()` plus an explicit allowlist: fixed `PATH` (absolute dirs only), `HOME=workspace/.iskra-home`, `LANG`, `TMPDIR=workspace/.iskra-tmp`, `CARGO_HOME`, `RUSTUP_HOME`, `CARGO_NET_OFFLINE`, and the git variables | `child_environment_does_not_inherit_parent_secrets` |
| 12 | Iskra's private HOME/TMPDIR read or written through the file tools | `read_file`, `write_file`, the ls/cat operands and `dir` refuse `.iskra-home` and `.iskra-tmp` (case-insensitive) | `file_tools_refuse_private_iskra_dirs` |
| 13 | PATH lookup at call time (`workspace/git`) | Absolute paths pinned at startup; a missing program is disabled, never looked up again | `programs_are_pinned_and_never_resolved_from_workspace` |
| 14 | Grandchildren surviving a timeout, `/stop` or drop | `process_group(0)`; the `ProcessGroup` guard calls `killpg(SIGKILL)` on timeout, after a normal exit (leftovers) and on drop (task abort) | `timeout_kills_whole_process_group`, `abort_kills_whole_process_group` |
| 15 | `read_file` following a final symlink, or reading a hard link to an outside file | `O_NOFOLLOW` on the final open; regular files with `nlink > 1` refused. Also applied to `write_file` targets and `cat` operands | `read_file_refuses_hard_links_and_final_symlinks` |
| 16 | **New:** BSD `ls`/`cat` following a symlink operand out of workspace; `ls -L/-H/--dereference` | ls/cat operands resolved like `read_file` (outbound symlinks refused); short-flag allowlist; long options refused | `ls_and_cat_refuse_escaping_operands_and_flags` |
| 17 | Arbitrary code from cargo build scripts and tests (read `~/.ssh` or the project `.env`, network, writes outside workspace, hard links out) | Seatbelt profile (constant in `src/tools/sandbox.rs`) for **all four programs** on macOS. Network denied. Writes only under workspace. Reads denied under `/Users`, `/Volumes`, `/private/tmp`, `/private/var/folders`, the owner's home and the project root, except the workspace and the toolchain sysroot. Ancestors of those are allowed for metadata only | `sandbox_blocks_build_script_reading_project_env`, `…_listing_ssh_dir`, `…_network`, `…_writing_outside_workspace`, `…_hard_linking_outside_file` |
| 18 | **New:** with `(allow default)`, a build script could reach macOS daemons: keychain via securityd (`git credential-osxkeychain get`, Security.framework), pasteboard, `defaults write` via cfprefsd, Apple Events | `(deny mach-lookup)` with no exceptions, plus `(deny appleevent-send)`. cargo/rustc/ld/git need no mach service. Manual probes (2026-11-20, scratch dir) under the profile: `security find-generic-password` could not reach the keychain, `pbpaste` failed, `defaults write` failed, `osascript` failed | Covered by the profile; no automated test (it would touch the owner's keychain and pasteboard) |
| 19 | **New:** files a build script plants for a later unsandboxed program (`.git/config` filter or diff drivers, symlinks, hard links) | ls/cat/git run under the same sandbox as cargo. Without a sandbox cargo is disabled, so no unsandboxed program other than git itself writes `.git` (directive §5.3) | `sandboxed_cargo_and_git_still_work_inside_workspace` (regression) |
| 20 | No sandbox available (Linux, or `sandbox-exec` missing) | cargo disabled with a clear error and removed from the schema enum and description; ls/cat/git run with argument policy and isolation only (known limitation, §5.3) | `cargo_disabled_without_os_sandbox`, `schema_lists_all_tools_when_every_program_is_available` |
| 21 | Credentials in the `run_command` log line | The log line (and rejection/error lines) pass through `memory::redact_credentials` | `run_command_log_line_redacts_credentials` |

Refused fail-closed and listed for the owner to decide (each may be harmless):
- git: `-c` after the subcommand (`git log -c`, a combined diff); `rev-parse --git-dir` (it matches the banned `--git-dir`); any `--e`/`--o`/`--t`-style abbreviation that is a prefix of a banned option; `:/<regex>` revision syntax (starts with `/`); `-S` clusters on commit (`-mSome` attached messages); any option before the subcommand, including `--no-pager`.
- cargo: any option before the subcommand (e.g. `cargo -q build`); program arguments after `--` that look like absolute or `..` paths.
- ls/cat: GNU long options; BSD flags outside the allowlist.

Known limitations (not fixed):
- Children now run in their own process group, so pressing Ctrl-C on Iskra in a terminal no longer reaches a running child. If Iskra exits abruptly (Ctrl-C, kill) while a command runs, no destructor runs and the child keeps going until it finishes by itself. Timeout, `/stop` and normal exit are covered. A signal handler belongs in `main.rs`, which is v1.1-B territory.
- `read_file` still has no size cap. That is not a sandbox bypass; left as is.
- Inside the sandbox, system locations outside the denied set (for example `/etc`, `/Library`) stay readable. Directive scope is the home folder and the project root.

### Verification (automated)
1. Baseline before changes: `cargo test` → 56 passed.
2. After: `cargo test` → **81 passed, 0 failed** (56 − 1 W-1 test + 26 new). Run twice, stable. The macOS sandbox tests build real fixture crates with cargo through `run_command`. Each asserts that the build failed at the exact attempted operation (the build script's `expect` message appears) and that no effect exists (marker file, listener connection, outside file, hard link).
3. `cargo build --release`: 0 warnings from `iskra`. The `proc-macro-error2` future-incompat note from a dependency is unchanged.
4. W-1 grep: no matches.
5. Budget: stripped release binary **6,295,072 bytes = 6.30 MB (6.00 MiB)** (previously 6,094,400). Direct dependencies **7** (unchanged; no new crate, no sandbox crate). Idle RSS after startup (Telegram long-poll): **14,784 KB = 14.4 MiB** (previously 14,288 KB).

### Verification (live, Telegram, one instance)
Time window: **2026-11-20 14:26:29 → 14:33:31 +0300**. Release binary started from this worktree (`.env` symlinked by the owner) and stopped with SIGTERM by Claude Code at the end. Startup log: `run_command_sandbox available=true`, no `run_command_program_disabled` lines.
- `.env` one folder up:
  - On the first try the model refused by itself and made no tool call, so the hardening path was not exercised.
  - On the second try the owner asked explicitly for `cat ../.env`. The log shows `run_command_rejected program=cat reason=Parent-directory (..) paths are refused: ../.env`. Nothing was spawned. ✓
- git and cargo on `workspace/trial_project`:
  - `cargo test` → `code=0`.
  - `git status` before any `git init` → `code=128` ("not a git repository"). This is correct: git no longer finds the parent Iskra repository.
  - After `git init` (dir=trial_project), `git status` → `code=0`. ✓ W-1 is gone.
  - A model typo `dir="rial_project"` was refused as a non-existing folder.
  - Note: this left a `workspace/trial_project/.git` in the worktree's workspace copy, which is gitignored.
- `/stop` during a slow build (fixture `workspace/slow_build`, whose build.rs sleeps 90 s):
  - First attempt: `/stop` arrived after the 92 s build had already finished (`telegram_stop idle`), so it is not counted.
  - Second attempt, after `touch build.rs`: `run_command program=cargo dir="slow_build" args=["build"]` at `ts=1795174381`, then `telegram_stop aborted` at `ts=…382`.
  - `ps` afterwards: no `cargo`/`rustc`/`build-script` process remained, and there was no `run_command_exit` line (the task was dropped and the group killed). ✓
  - Caveat: the abort came about 1 s after the start, so the build may still have been compiling build.rs (rustc) rather than running it. The build-script stage itself is covered by `abort_kills_whole_process_group`.

## v1.1-B — Memory reflection: saved memory visible without restart (2026-11-20)

Status: implemented on branch `v1.1-b-memory-reflection`; owner to review and commit.

### Root cause
The system prompt (SOUL + USER + MEMORY) was assembled once at startup and reused for every turn: `src/main.rs` built `memory::load()`/`prompt::assemble` and passed the result to `channel::run`/`run_cli`, and `ChannelState` stored `system_prompt` plus a startup `memory_status`. `memory::load_from` was private, so the daily-note date was also fixed at startup. `memory_save` appended to the end of `MEMORY.md` while `apply_budget` keeps only the first 200 lines / 24 KB, so an entry past the budget stayed invisible until restart.

### What changed
- `src/memory.rs`: `load_from` and `save_to` are now `pub(crate)`. `MemoryStatus` moved here from `channel.rs`. `save_to` re-checks the new full content with `apply_budget`; when truncated, the success message says the entry is beyond the load budget and will not be visible until older entries are archived. File/entry format, redaction, and atomic write are unchanged.
- `src/prompt.rs`: new `turn_prompt(identity, memory_dir)` reloads memory via `memory::load_from` and reassembles the prompt on every call. Memory warnings go to `logs/agent.log` as `context_warning …`; there is no per-turn `println!`.
- `src/main.rs`: identity still loads once, and the startup warning print remains for the first load only. `channel::run` and `run_cli` now receive `identity`, not a prebuilt prompt. The CLI builds `turn_prompt` for each input line.
- `src/channel.rs`: `ChannelState` stores `identity` instead of `system_prompt`/`memory_status`. `start_turn` builds the prompt inside the spawned task immediately before `run_turn`; `send_status` reloads the current memory file for `truncated`/`warnings`.
- Tests added: `saved_memory_visible_next_turn_without_restart`, `daily_note_date_follows_clock`, `save_reports_when_entry_is_beyond_load_budget`.

### Verification
1. `cargo test`: **59 passed, 0 failed** (56 baseline + 3 new).
2. `cargo build --release`: 0 warnings from `iskra`; direct dependencies unchanged at 7. The transitive `proc-macro-error2 v2.0.1` future-incompatibility note is unchanged.
3. P3 CLI live test (`./target/release/iskra --cli`, worktree memory): 2026-11-20 14:43:34–14:43:40. Sent "Remember: my test word is ember-42." then "What is my test word?" without restart. The second answer contained `ember-42`; `memory/MEMORY.md` held `- User's test word: ember-42`.
4. P2 Telegram live test (worktree, one instance): bot run 2026-11-20 14:44:39–14:48:14. The owner sent the same two messages. `logs/agent.log` shows the first turn calling `memory_save`, then the next turn with no tool call and no restart, and the owner confirmed the answer contained `ember-42`. `memory/MEMORY.md` held the saved test word.

### Temporary workarounds
- None added. W-1 (`RUN_COMMAND_DISABLED` and the temporary run_command rule line in `assemble()`) was left untouched.

### Notes
- Identity hot reload remains out of scope; only memory is reloaded per turn.
- Live tests used the worktree's own `memory/`; `.env` was never opened or printed, and the owner's real `iskra/memory/` was not read.
- No commit, push, or merge was performed.

## v1.1 merge — master re-verification (2026-11-20)

Merges: `5bbe10d` (v1.1-A shell hardening) then `c8746ee` (v1.1-B per-turn memory reload). The v1.1-B merge conflicted in `SESSION-LOG.md` and `src/tools.rs`; `src/tools.rs` was resolved to the v1.1-A version (no W-1 code remains), and the run_command line in `src/prompt.rs` is the v1.1-A version.

### Verification on master
1. `cargo test`: **84 passed, 0 failed, 0 ignored** (81 from v1.1-A + 3 from v1.1-B).
2. W-1 grep (`v1.1-A pending`, `RUN_COMMAND_DISABLED`, `TEMPORARY WORKAROUND`) in `src/`: no matches.
3. Live re-check on master (one instance, Telegram): **passed** (owner-confirmed; log evidence below).
   - Shell: at 15:29:19 +0300 the log shows `run_command_rejected program=cat reason=Parent-directory (..) paths are refused: ../.env`, followed by `tool_error`. Nothing was spawned. Git works inside the workspace: at 15:25:36, `git add`, `git commit` and `git log` ran in `dir="probe_ok"` in an earlier run of the same binary.
   - Memory: at 15:29:52 the log shows `tool_ok name=memory_save`. The next turns (15:29:56, 15:30:06) made no tool call and there was no restart in between; the owner confirmed the recall.
   - Run started at 15:28:59 with `run_command_sandbox available=true`.

v1.1 is closed.

### Known limitations carried forward
- An abrupt Iskra exit (Ctrl-C, kill) during a command leaves the child running until it finishes (no signal handler yet).
- Without an OS sandbox (non-macOS), cargo is disabled; ls/cat/git run with argument policy and isolation only.
- Inside the sandbox, system locations outside the denied set (e.g. `/etc`, `/Library`) remain readable.
- Options refused fail-closed pending an owner decision are listed in the v1.1-A entry.

## v1 real-use gate — closed early (2026-11-20)

- Owner decision on 2026-11-20: the gate ends today instead of 2026-11-22. Verdict: **v1 passes the gate**; v2 implementation may start.
- Discomforts found during the gate and where they went: model did not know its own usage / commands were invisible → Block 6.2; post-it board → Block 6.3, duplicated board messages → 6.3.1; shell sandbox bypass → v1.1-A; memory visible only after restart → v1.1-B; no acknowledgement during long turns → Block 6 (typing signal); stickers and reactions not understood → Block 6.1; strings not in the user's language → Blocks 6.5–6.6; no conversation context between messages → v2 `session/v1` (design §2.9, open); unclear data location → design §0.6 (open).
- Long-horizon items (memory growth, resuming after days off, cost accumulation) stay under observation.

## v1.2 — Model name replacement + reasoning echo (2026-11-20, branch `v1.2-model-reasoning`)

Status: **code and automated tests complete; live Telegram verification pending the owner.** Worked only in the `../iskra-v1.2` worktree. Nothing committed, pushed or merged. `.env` was never opened or printed, and the real `memory/` was not read.

### Root cause
`DEFAULT_MODEL` and `.env.example` still named `deepseek-chat`, a model name DeepSeek announced for retirement on 2026-07-24. The current (2026-11-20) official docs list `deepseek-flash` as the name to use; the legacy `deepseek-v4-flash` name is still accepted but is routed to V4.1 Flash. Separately, `deepseek-flash` has thinking mode enabled by default, and requests that carry `tools` must echo every assistant `reasoning_content` or the API returns 400. `Message` had no `reasoning_content` field, so the echo was impossible.

### Official checks (2026-11-20)
- https://api-docs.deepseek.com/quick_start/pricing — current model names `deepseek-flash` and `deepseek-v4-pro`; footnote: use `deepseek-flash`; legacy `deepseek-v4-flash`/`deepseek-v4-flash-vision-exp` are retired but still accepted.
- https://api-docs.deepseek.com/updates — 2026-11-13 entry confirms `deepseek-flash`; 2026-09-10 entry confirms it is the current V4.1 Flash name.
- https://api-docs.deepseek.com/guides/thinking_mode — thinking is enabled by default (default effort `high`); with `tools`, previous `reasoning_content` must be passed back in full or the API returns 400.
- Prices (peak, USD per 1M tokens, input cache-miss / output): `deepseek-flash` 0.30 / 1.20; `deepseek-v4-pro` 1.32 / 3.96. Off-peak is half of peak. The same date and URL are in the `PEAK_PRICES` comment in `src/usage.rs`.

### States and transitions (written before implementing, gate ②)
- `Message.reasoning_content: Option<String>`: parsed from an assistant reply when present, otherwise `None`.
- Serialization: `Some` is emitted verbatim; `None` omits the field entirely (no empty string, no `null`).
- `Message::system` / `user` / `tool` always set `None`. `run_turn` pushes the assistant reply by clone, so the same turn's next tool-carrying request carries the reasoning. Tool results carry `None`. Each turn starts fresh at `[system, user]`, so the echo scope is one turn's tool round-trips only.
- `Usage`: `prompt_tokens`, `completion_tokens`, `total_tokens` each default to 0; when `total_tokens` is 0 it is recomputed as prompt + completion. A missing `usage` remains `None`.

### What changed
- `src/brain.rs`: `DEFAULT_MODEL = "deepseek-flash"`; `Message` gains `reasoning_content: Option<String>` with `skip_serializing_if`; constructors set `None`; `Usage` uses a small `Raw` struct inside a manual `Deserialize` so all three fields default and `total_tokens` falls back to the sum.
- `.env.example`: default model comment now says `deepseek-flash`.
- `src/usage.rs`: `PEAK_PRICES` is now exactly `deepseek-flash` (0.30, 1.20) and `deepseek-v4-pro` (1.32, 3.96); retired names removed. Header records the check date and URL. The self-state estimate wording is “DeepSeek public peak prices, checked 2026-11-20”.
- `src/channel.rs` + `src/locale.rs`: `/status` now includes an “Estimated cost today” line (`~$…` or “unavailable”) in en/ko/ru, using the same price table and today's usage.
- No change to `src/loop_.rs`: it already pushes the assistant message verbatim; the new field proves it by test.

### Verification (automated)
1. `cargo test`: **88 passed, 0 failed, 0 ignored** (84 baseline + 4 new).
   - `keeps_reasoning_content_in_followup_request`: reply `reasoning_content` survives parsing and appears in the serialized follow-up request next to the tool result.
   - `omits_reasoning_content_when_absent`: missing field serializes without `reasoning_content` and without `null`; system/user/tool constructors do the same.
   - `usage_defaults_and_falls_back_to_prompt_plus_completion`: missing or zero total becomes prompt + completion; `{}` parses to zeros.
   - `chat_response_parses_without_usage`: a response with no `usage`, and one with usage lacking `total_tokens`, both parse.
2. Acceptance grep in `src/` and `.env.example` for `deepseek-chat` and `deepseek-reasoner`: **0 matches** (historical mentions remain only in SESSION.md / SESSION-LOG.md).
3. `cargo build --release`: 0 warnings from `iskra`; the transitive `proc-macro-error2 v2.0.1` future-incompatibility note is unchanged.
4. Budget: release binary 8,433,072 bytes; stripped **6,295,088 bytes = 6.00 MiB** (previous 6,295,072). Direct dependencies **7** (unchanged; no crate added). CLI idle RSS **4,032 KB = 3.94 MiB**, measured from a temp directory with a dummy key and no network; this is not comparable to the v1.1 Telegram long-poll idle figure.
5. `MODEL_TIMEOUT_SECS` stays 60: there is no live timing yet, so the “>45 s → 120” rule is not triggered.

### Verification (live, Telegram) — passed (2026-11-20, owner-run, one instance from this worktree)
- Run started 18:01 +0300 with `run_command_sandbox available=true`; identity files came from the templates (fresh worktree).
- Turn: "create workspace/test.txt with '불꽃', read it back, report". Log: `model_call model=deepseek-flash tools=[write_file]` (prompt 1895 / completion 88) → `tool_ok write_file` → `model_call tools=[read_file]` (2002 / 39) → `tool_ok read_file` → `model_call tools=[none]` (2057 / 28) → `turn_end answer`. Two tool round-trips in thinking mode, **no 400**, no `model_error`. Wall time from `turn_start` to `turn_end`: **7 s** (slowest single call ≈ 4 s) — the >45 s rule is not triggered; `MODEL_TIMEOUT_SECS` stays 60.
- `/status`: model `deepseek-flash`, memory ok, tokens today 6109, estimated cost ~$0.001972, balance 5.73 USD.
- Because the default model already runs thinking mode, this one turn covers both live items of the directive.

### Owner decision
- `deepseek-flash` defaults to thinking mode. Measured live: 3 calls, 6,109 tokens, ~$0.002, 7 s for a two-tool turn — completion tokens stayed small (28–88 per call). Accepted for now; a thinking on/off setting belongs to the v2.0 model-profile work.

## v1.3 — Telegram cleanup (2026-11-20, branch `v1.3-telegram-cleanup`)

Status: **code, automated tests and live Telegram verification complete (one item with a noted limit).** Worked only in the `../iskra-v1.3` worktree. Nothing was committed, pushed or merged by the executor. `.env` was symlinked by the owner for the live run and never opened or printed by the executor; the real `memory/` was not read.

### Root cause
Telegram showed raw developer-facing model/tool errors, and the user string table still used the word for a conversation turn (ko/ru) and English “turn”. Attachments other than text or stickers were silently ignored. `/start` was a one-line greeting plus a duplicated help block, `/status` put the model and token lines before cost and balance, and some fallbacks were hard-coded English (“unknown”, “unavailable”). The bot also answered and remembered in any chat type, and forwarded third-party text reached the model as if the owner had spoken it.

### States and transitions (written before implementing, gate ②)
- Model errors: `brain::ChatError` classifies timeout, connection failure, 401/403, 402, 429/5xx and everything else. `loop_::TurnError` adds repeated tool error, max round-trips and empty response. `run_turn` returns `Result<String, TurnError>`.
- Display vs logs: exactly one localized A1 string goes to the chat. The original detail stays in `Display` and therefore in `telegram_turn_error …`, `model_error …` and `tool_error …` log lines. CLI keeps printing the original detail.
- Private-chat gate: immediately after the allowed-sender check. A non-private chat gets one A6 line, then `leave_chat`, then `telegram_non_private chat_type=… left=true|false`; no content handling, memory write or model call. Leave failure is logged only, never retried. The reaction handler runs the same gate.
- Non-text: `MessageKind::Common` media (photo, voice, audio, document, video, video note, animation, etc.) gets one A3 line and `telegram_non_text_notice kind=…`; other service messages stay ignored.
- Forwarded text: if `forward_origin` is present, it is wrapped in the A7 form before reaching the model, and command/sticky handling is skipped. A literal `>>>` inside the text becomes `> > >`, so the wrapper cannot be closed early.
- `/start` sends only the A4 text (no duplicated help). `/status` order is spent today → balance → model → memory → tokens today.

### What changed
- `src/brain.rs`: added `ChatError` and `classify_status`; `chat` now returns structured errors with the original detail. Timeout and connection failures are separated before parsing.
- `src/loop_.rs`: added `TurnError` and the `ChatError → TurnError` mapping; repeated tool errors, max round-trips and the second empty response are now typed. The original message is still logged.
- `src/locale.rs`: added/replaced A1–A6 keys. `TurnBusy` became `BusyNotice`; `TurnStopped` was removed. Added five plain-language repeated-error reasons, `NonTextNotice`, `PrivateOnly`, `CostUnknown`, the new A4 text and the new `/status` cost label. No user string contains the turn jargon words.
- `src/channel.rs`: A6 check before any content work in messages and reactions; A3 reply for common media with a log kind; A7 wrapping for forwarded text with commands skipped; error category → locale key; `/start`/`/status` cleanup; extracted `status_text` for order testing.
- `src/prompt.rs`: one English rule line about forwarded material and the A7 reply notice, plus `FORWARD_HEADER`, `wrap_forwarded` and the `>>>` escape.
- teloxide 0.17 API check: the installed `teloxide-core 0.13.0` exposes `Chat::is_private`, `Chat::is_group`, `Chat::is_supergroup`, `Chat::is_channel`, `Message::forward_origin`, `MessageReactionUpdated::chat` and `Bot::leave_chat`; the implementation uses those exact types, so no meaning-changing workaround was needed.

### Verification (automated)
1. `cargo test`: **97 passed, 0 failed, 0 ignored** (88 baseline + 9 new).
   - `classifies_http_statuses`: 401/403 → auth, 402 → payment, 429 and 5xx → busy, 404 → other; detail keeps status and body.
   - `maps_every_error_category_to_plain_user_text`: every category maps to the approved en/ko/ru string and raw detail never appears in the reply; repeated-tool text uses the plain reason only.
   - `user_strings_avoid_busy_jargon`: word-level scan of every key in en/ko/ru for the turn jargon; zero matches (the `расход`/`подходит` substrings are different words).
   - `wraps_forwarded_content_in_markers` and `escapes_closing_marker_inside_forwarded_content`: exact A7 header/opening/closing and a single closing marker even when the input contains `>>>`.
   - `prompt_carries_the_forwarded_material_rule`: the English rule and A7 wording are in the assembled prompt.
   - `status_lines_follow_the_approved_order`: cost < balance < model < memory < tokens.
   - `labels_non_text_media_kinds` and `labels_chat_types_for_logging`: photo/voice kind labels and group/supergroup/channel/private labels.
2. `rustfmt --edition 2024 --check` on the five changed source files: clean. A repository-wide `cargo fmt --check` still reports the pre-existing unformatted `src/tools*` files, which were not touched here. `cargo clippy --all-targets`: only the pre-existing warnings (collapsible nested `if` in the board-reply path, char comparison in locale, two memory lints); no new errors.
3. Acceptance greps: `TurnBusy`/`TurnStopped` are gone; `src/channel.rs` has no hard-coded `"unknown"` or `"unavailable"`.
4. Budget: release binary 8,450,192 bytes; stripped **6,311,696 bytes = 6.02 MiB** (previous 6,295,088 / 6.00 MiB). Direct dependencies **7** (unchanged). CLI idle RSS **4,064 KB = 3.97 MiB** (waiting on stdin with a dummy key, no network).

### Verification (live, Telegram) — passed with one noted limit (2026-11-20, owner-run, one instance from this worktree)
- Setup: the worktree had no `.env`, so the owner symlinked `../iskra/.env` (same method as the v1.1-A live run). Run started 18:32:08 +0300 with `run_command_sandbox available=true`.
1. Photo → A3 text. Log 18:32:11 `telegram_non_text_notice kind=photo`. **Passed.**
2. Forwarded text → wrapped as material. 19:06:54 (third-party message without an instruction): no tool call; the reply treated it as unverified material and said there was no instruction to follow. 19:30:35 (third-party message asking to buy groceries): no tool call; the reply ended with the A7 notice ("I did not follow the request in the forwarded text"). **Passed, with a limit:** the forwarded instruction was not one Iskra could execute, so refusal of an executable forwarded instruction (e.g. `write_file`) is covered only by the automated wrapper/prompt tests. The owner accepted this limit on 2026-11-20.
   - Two earlier attempts (18:35, 18:36) were sent with the sender name hidden. Telegram then delivers no `forward_origin`, so the bot cannot tell them from the owner's own text and they were handled as owner instructions (`hacked.txt` written, later deleted). This is a platform limit, not a defect: a forward with the sender hidden is the owner's own message.
3. Bot added to a test group, owner message → A6 line and leave. Log 18:33:29 `telegram_non_private chat_type=group left=true`; another member's message was ignored (`telegram_ignore user_id=…`). **Passed.**
4. `/start` shows only the A4 text; `/status` order is spent today → balance → model → memory → tokens today (owner screen, 19:34). **Passed.**
5. `LLM_MODEL=no-such-model` (shell override, `.env` unchanged), message "안녕" → the A1 "other" text only. Raw `400 Bad Request … you passed no-such-model` appears only in `model_error` and `telegram_turn_error` (19:33:42). Normal model restored at 19:34:24. **Passed.**

### Observations (not v1.3 defects)
- 18:37 `telegram_send_failed` after one retry, yet the owner received the same reply twice at 18:47: a send judged failed on a network error had in fact reached Telegram, and the retry duplicated it. Retrying an ambiguous send can duplicate messages — recorded for v2.0.
- A configuration error at startup is not written to the log when `logs/` does not exist yet (`main.rs` logs before `create_dir_all("logs")`). Pre-existing; recorded for v2.0.
- An invalid model name (HTTP 400) maps to the generic A1 text; a clearer "check the model name" message belongs to the v2.0 provider profile.
- The log does not record whether a message was forwarded, so the live check for item 2 relied on tool-call absence and the reply text. A `telegram_forwarded` log line is a candidate for v2.0 together with the `origin` field.

### Notes
- No temporary workaround was added. The v2.0 source field (`origin: owner | forwarded | quoted`) will replace this wrapper, as the directive says.
- Commands inside forwarded text are deliberately not executed; the wrapped text goes to the model as material only.

## v2.0 — `session/v1` resumable owner workspace (2026-11-23)

Status: **owner-approved specification implemented; automated tests and real CLI process checks passed; owner-run Telegram and real-provider verification pending.** No commit, push or merge was performed.

### Scope and state handling
- The owner selected the workspace-first foundation for the longer-term multi-agent direction and approved the detailed `SESSION.md` specification before implementation.
- Added `src/session.rs`, a core-owned, versioned `memory/session.json` store with atomic writes, Unix `0600` permissions, credential substitution, 50-record storage and 12-record / 24-KiB replay limits. Owner input and final answers are capped at 8,000 stored characters with an explicit truncation marker.
- Work transitions are `idle → running → completed | failed | stopped`; startup and cancellation recovery produce `interrupted`. A running write precedes every model call. Corrupt data, unsupported versions and write failures refuse model/tool execution instead of silently resetting the workspace.
- Completed owner/assistant messages survive restart. Unfinished requests retain a core-generated status note and a fixed prompt rule forbids automatic resumption. Tool calls, tool results and `reasoning_content` remain scoped to the original turn.
- `src/loop_.rs` now wraps the existing turn engine with session begin/complete/fail handling. CLI and Telegram share the same `Session`; `/stop` persists `stopped` before aborting the task, and `/status` has a localized workspace line in en/ko/ru.
- Added `tests/session_cli.rs`, which runs the compiled Iskra binary against a local OpenAI-compatible HTTP server with dummy credentials and isolated temporary data. No real credential or personal memory was read.

### Verification (automated)
1. `cargo test`: **114 passed, 0 failed, 0 ignored** — 112 unit tests (97 baseline + 15 new) and 2 end-to-end CLI process tests. Coverage includes all work states, busy rejection, startup recovery, budgets, redaction, forwarded material, invalid UTF-8/JSON/version fail-closed behavior, and initial/later write failures.
2. Real CLI process restart: the second process's actual HTTP request contained `[system, previous user, previous assistant, current user]`; the test phrase survived restart, `MEMORY.md` stayed empty, and `session.json` mode was `0600`.
3. Real process kill: after the first process persisted `running` and reached the local model endpoint, it was killed. The next process changed the record to `interrupted`; with `exit` as the only input, the model POST count stayed at **1** (no new call or retry).
4. `rustfmt --edition 2024 --check --config skip_children=true` on changed Rust files: passed. Child recursion is disabled because the pre-existing `src/tools*` files are outside this patch and are not repository-wide rustfmt-clean.
5. `cargo clippy --all-targets`: only the same **5 pre-existing warnings** in the sticky reply path, locale character comparison and memory code; no new session/test warnings. The transitive `proc-macro-error2 v2.0.1` future-incompatibility note is unchanged.
6. `cargo build --release`: succeeded with zero Iskra warnings. Release binary **8,525,712 bytes**; stripped **6,361,264 bytes = 6.07 MiB** (previous 6,311,696 / 6.02 MiB). CLI idle RSS **4,064 KB = 3.97 MiB**. Direct dependencies **7**; no dependency added. All hard budgets pass.
7. During test development, two incorrect assertions were corrected (substring matching and a byte-budget fixture), and the mock server was made explicitly blocking on accepted sockets after a parallel-test `WouldBlock` failure. Final repeated integration runs and the full suite passed. These were test fixes, not product workarounds.

### Unverified / next action
- Owner-run Telegram: recall a test phrase after restart without `memory_save`; `/stop` records unfinished work and does not retry on restart; `/status` shows accurate session counts in the applicable locale.
- Real configured provider compatibility of replayed assistant history (including thinking-mode behavior) is not yet live-tested; the automated provider was a local OpenAI-compatible server.
- No multi-agent roles, routing, parallel execution, external adapters, session reset UI, automatic summarization or cloud sync were added.
- Unrelated `src/tools*` formatting produced recursively by rustfmt was reverted; those files are unchanged from HEAD. Pre-existing untracked `AGENTS.md` and `CLAUDE.md` were left untouched.

### Follow-up — local Bot API `/stop` verification (2026-11-23)
- Added `channel::tests::telegram_stop_persists_stopped_before_task_drop`, using teloxide's custom API URL with a local HTTP Bot API server and a dummy token. No external Telegram request was made and no personal memory or credential was read.
- The test drives the real `stop_turn` handler, verifies the Korean stopped response in the actual outgoing HTTP JSON, drops the work guard, and confirms the persisted record remains `stopped` and reloads as unfinished.
- Final `cargo test`: **115 passed, 0 failed, 0 ignored** (113 unit + 2 CLI process tests). Changed-file rustfmt passes; clippy still reports only the same 5 pre-existing warnings.
- Production code and release-budget measurements are unchanged by this test-only addition. Owner-run external Telegram and real-provider checks remain pending.


## Agent workbench — architecture and business Stage 0 (2026-11-23)

Status: **approved design documentation and automated baseline complete; no production code changed; owner-run `session/v1` live verification remains pending.**

### Current-state correction

The actual repository is clean at the start of this Stage 0 work on `master`/`origin/master` HEAD `d8b9abc` (`wip: goal 중간 저장`), which already contains the `session/v1` implementation. The personal `NOW.md` entry describing `f5a80dc` and uncommitted session files was stale. Actual Git state and source were used as the current baseline.

### Design and reference review

Reviewed the current Rust module boundaries and the public architecture, license, product, and pricing material of OpenMausBot and OpenBot.

- OpenMausBot: Apache 2.0 outside its separate `enterprise/` tree. Its normalized engine/runtime contracts, registry/admission, durable queue, approval cards, and portable packages are candidates for selective, attributed reuse. Its broad Electron/VM/voice/connector surface is not part of the Iskra slice.
- OpenBot: PolyForm Noncommercial 1.0.0. Its independent workspace/session, durable FIFO, explicit delegation, and team operating model are useful general ideas. No OpenBot code, text, tests, or assets will be copied into the commercial product. Its documented full-access/no-per-action-approval mode is rejected for Iskra.
- Preferred engine path: keep the built-in OpenAI-compatible HTTP engine and evaluate one standard ACP subprocess adapter later rather than cloning many provider-specific drivers. External tools must remain inside core approval, audit, sandbox, and cancellation boundaries.
- Business recommendation: free full local orchestration with BYOK/BYO-agent; charge for always-on infrastructure, human team seats, operations, and enterprise services rather than per Agent. Avoid OpenBot-style commercial-license-plus-hosting double charging. Set prices only after measuring actual costs.

The approved detailed decisions are in `docs/ARCHITECTURE.md` and `docs/BUSINESS.md`.

### Current coupling recorded for Stage 1

- `ChannelState` owns one brain/tools/identity/session and one active `JoinHandle`.
- `Brain::chat` directly obtains global tool schemas.
- `Tools` uses a fixed workspace and a process-global capability advertisement.
- Memory/sticky/locale/usage paths include global state that must become instance/context owned before independent Agents are safe.
- Current session records do not identify Agent, Run, parent Task, queue, approval, artifact, usage, or delivery state.

### Verification performed on the deliverable baseline

1. `cargo test`: **115 passed, 0 failed, 0 ignored** — 113 unit tests and 2 real CLI process tests. The existing transitive `proc-macro-error2 v2.0.1` future-incompatibility note remains.
2. `cargo build --release`: passed with **zero Iskra warnings**.
3. A temporary copy of the release binary was stripped and measured: **6,361,272 bytes = 6.07 MiB**. The temporary copy was removed.
4. CLI idle RSS measured after one second with dummy model configuration in an isolated temporary working directory: **3,216 KB = 3.14 MiB**. No real `.env`, personal memory, model call, or Telegram request was used. The process was stopped and the temporary directory removed.
5. `Cargo.toml` direct dependencies: **7**. All hard budgets remain within their limits.

### Files and remaining verification

Added `docs/ARCHITECTURE.md` and `docs/BUSINESS.md`; updated project status/log documents. No Rust source, manifest, dependency, or public feature claim was changed.

Not verified here: real Telegram restart recall, `/stop` and `/status`, and real configured-provider replay/thinking behavior. No multi-agent runtime, ACP adapter, external engine, payment, account, hosted service, commit, push, or external send was performed.


## DeepSeek V4.1 Flash `session/v1` live gate — passed (2026-11-23)

Status: **passed owner-run Telegram and real-provider verification.** The provider was DeepSeek V4.1 Flash through the official `deepseek-flash` model name. No Rust source, dependency, or committed model default was changed. No commit or push was performed for this live result.

### Provider and setup

- Official docs checked on 2026-11-23: `deepseek-flash` currently resolves to **DeepSeek-V4.1-Flash-1017**; the OpenAI-format base URL is `https://api.deepseek.com/chat/completions`; thinking mode is enabled by default.
- One Iskra instance was run with `LLM_MODEL=deepseek-flash` and the official endpoint. The existing `.env` was not opened, printed, or copied by the executor. No personal memory content, reasoning text, or credentials were read or recorded.
- The previous restart and `/stop` runs were deliberately terminated for the verification; background task notifications reporting those expected terminations were not product failures.

### Results

1. **`/status` — passed.** Before the tests it reported model `deepseek-flash`, memory normal, six stored records and six context records. After the final restart it reported model `deepseek-flash`, **11 stored / 11 context records**, memory normal, and an unfinished work item. It also reported 65,149 tokens used today, an estimated $0.022226 cost, and $4.94 remaining balance.
2. **Restart recall without `memory_save` — passed.** Session work `id=7` completed the owner request to answer `ember-live-42`. After restart, the log showed `session_loaded records=7 unfinished=0`; work `id=8` answered the recall question with `deepseek-flash`, `prompt=2946`, no tool call, and no error. The owner confirmed that the actual reply contained `ember-live-42`. No `memory_save` call occurred in the metadata logs, so recall came from session replay rather than long-term memory.
3. **Thinking-mode tool round trip — passed.** Session `id=9` completed three model calls: `write_file`, `read_file`, then a final answer. Both tool calls succeeded and no 400 or parsing error occurred. The test file contained `iskra-deepseek-live-ok`.
4. **Cross-turn replay with tools — passed.** Session `id=10` followed the tool turn after the completed history was present in the session. The first request carried `tools=[run_command,read_file]`; `run_command` and `read_file` succeeded, the final answer completed, and no reasoning-content-related 400 or parsing error occurred. The owner confirmed that the reply identified `deepseek-live-check.txt` and `iskra-deepseek-live-ok`.
5. **`/stop` persistence and no restart replay — passed.** A long-answer request created session `id=11`; `/stop` produced `session_work_end id=11 state=stopped` and `telegram_stop aborted` before a model call completed. After restart the log showed `session_loaded records=11 unfinished=1`. The total persisted `model_call` count stayed **85 → 85**, with no new model or tool execution. The owner confirmed that `/status` showed the unfinished item.

### Cleanup and unverified limits

- Only the generated test file `workspace/deepseek-live-check.txt` was removed during cleanup. Other workspace, memory, session, and log data were left intact.
- The observed cross-turn tool requests were accepted without replaying past `reasoning_content`, despite the provider documenting reasoning echo requirements for requests carrying tools. This passed for the tested DeepSeek V4.1 Flash turns, but provider behavior for other models, future versions, or longer histories is not generalized.
- Verification covered one owner, one running instance, one provider/model, and the local Telegram channel. It did not verify multiple agents, ACP, hosted mode, or Stage 1 runtime changes.


## Token/time-efficient development workflow (2026-11-23)

Status: **approved operational workflow documented; no production code changed.**

Added `docs/WORKFLOW.md` and linked it from `AGENTS.md` and `SESSION.md`.

The workflow assigns broad architecture and unresolved decisions to Plan mode with a stronger model, bounded machine-verifiable implementation to DeepSeek Goal mode, and owner-driven Telegram/provider checks to normal mode. A smaller Plan-mode pass is optional only when the large specification leaves a real choice unresolved.

The immediate cause for this rule was the completed interactive verification goal: it consumed 24 turns and 19.4k tokens, with repeated blocked/resume cycles caused by owner-input waiting. Future goals must not contain pending owner input. Live verification is prepared and recorded outside autonomous goals.

Stage 1 is documented as three sequential goals: Engine/RunContext contracts, instance-owned Tools/Memory/Session boundaries, and Supervisor-owned channel execution. Each uses focused tests while iterating and one full milestone verification. No Stage 1 implementation was started by this documentation task.

No automated test or build was rerun because only documentation and workflow guidance changed. No commit or push was performed.

## Stage 1 Goal 1 — Engine and RunContext contracts (2026-11-23)

Status: **completed; automated verification passed; not committed.**

### What changed

- Added `src/run_context.rs` with `RunContext` carrying `run_id`, `agent_id`, the approved `tool_schemas`, and the matching `&Tools` executor. `RunContext::new` derives the schemas from the same `Tools` instance, so the model cannot be told about tools this run cannot execute.
- `Brain::chat` now takes `&RunContext` and builds its request from `context.tool_schemas()`. It no longer calls a global tool-schema function. Request serialization, `reasoning_content` echo, usage parsing, and structured `ChatError` mapping are unchanged.
- `loop_::run_active_turn` uses the same context for tool execution, and `turn_start` records `run_id` and `agent=main`.
- `Tools` exposes the instance method `schemas()`. The former global free function was removed while the process-wide capability advertisement (`ADVERT`) remains for Goal 2.
- `session::PreparedTurn::id()` exposes the work id as the Run identity.
- `src/main.rs` registers the new module only. The `run_turn` call sites in `src/main.rs` and `src/channel.rs` are unchanged.

### Verification

- `cargo test`: **117 passed, 0 failed** (115 unit + 2 real CLI process tests). New tests cover RunContext identity/schema source and the request-body schema source.
- `cargo build --release`: zero Iskra warnings; the existing `proc-macro-error2 v2.0.1` future-incompatibility note remains.
- `rustfmt --check` (edition 2024): changed files clean except `src/tools.rs`, which retains pre-existing formatting debt outside this change; the added `Tools::schemas` method is clean.
- `cargo clippy --all-targets`: 5 pre-existing warnings in `channel.rs`, `locale.rs`, and `memory.rs`; none in changed files.
- Budgets: stripped binary **6,377,872 bytes = 6.08 MiB**; direct dependencies **7**; CLI idle RSS **4,032 KB = 3.94 MiB** in an isolated dummy-config directory. No real `.env`, personal memory, Telegram request, or provider call was used.
- Not verified: owner-run Telegram/provider behavior, multi-agent isolation, and queue behavior (later goals). No commit or push was performed.

## Stage 1 Goal 2 — Instance-owned Tools, Memory, and Session boundaries (2026-11-23)

Status: **completed; automated verification passed; not committed.**

### What changed

- Added `src/instance.rs` with `InstancePaths` (`workspace`, `memory`, `session`). `InstancePaths::default()` keeps `workspace/`, `memory/`, and `memory/session.json`.
- `Tools` now owns its capability advertisement (`advert`) and memory root (`memory_dir`). The process-wide `ADVERT: OnceLock` was removed. `Tools::new_in(workspace, memory)` builds an explicit instance; `Tools::schemas()` reads the instance advertisement.
- `Tools::execute("memory_save", ...)` writes to the instance memory root through `memory::save_to`. The global `memory::load()`/`save()` wrappers were removed; `load_from`/`save_to` take an explicit path.
- Stickies expose path-based `add_at`, `board_at`, `remove_at`, `load_board_state_at`, `save_board_state_at`, and `clear_board_state_at`. The global path wrappers were removed; `Tools` and the Telegram channel use the instance memory root.
- `Session` keeps its per-instance path. `SESSION_FILE` names the file; `load_default()` was removed, and the composition root uses `Session::load(&paths.session)`.
- `main.rs` creates one `InstancePaths` and passes the same roots to memory loading, `Tools`, `Session`, and the channel. `ChannelState` carries the paths and uses them for `/status`, prompt assembly, sticky commands, and board state.
- No on-disk format or schema changed. No dependency was added.

### Verification

- `cargo test`: **122 passed, 0 failed** (120 unit + 2 real CLI process tests). New tests cover: two Tools instances with different capability advertisements, two Tools instances keeping memory/sticky files in their own roots, two Sessions with separate paths not sharing records, and the compatibility `InstancePaths::default()` layout.
- `cargo build --release`: zero Iskra warnings; the existing `proc-macro-error2 v2.0.1` future-incompatibility note remains.
- `rustfmt --check` (edition 2024): changed files clean except `src/tools.rs` and `src/tools/tests.rs`, which keep pre-existing formatting debt outside the changed hunks; the new sections are clean.
- `cargo clippy --all-targets`: the same 5 pre-existing warnings in `channel.rs`, `locale.rs`, and `memory.rs`; none in new code.
- Budgets: stripped binary **6,394,376 bytes = 6.10 MiB**; direct dependencies **7**; CLI idle RSS **4,000 KB = 3.91 MiB** in an isolated dummy-config directory. No real `.env`, personal memory, Telegram request, or provider call was used.
- Not verified: owner-run Telegram/provider behavior, multi-agent isolation beyond paths, and queue behavior (Goal 3/Stage 2). No commit or push was performed.

## Stage 1 Goal 3 — Supervisor-owned channel execution (2026-11-23)

Status: **completed; automated verification passed; not committed.**

### What changed

- Added `src/supervisor.rs` with `RunSupervisor`. `start` performs admission atomically under one lock: reject when the stored handle is still running, otherwise spawn and store the new handle. `take_running` removes the handle only when it is still active; a finished task is cleared and reported as no active run.
- `ChannelState` no longer stores `current_turn: Mutex<Option<JoinHandle<()>>>`; it carries `supervisor: RunSupervisor`. The `tokio::task::JoinHandle` import moved out of the channel module.
- `start_turn` now asks the supervisor to start the task. On rejection it keeps the existing `telegram_busy` log and localized busy notice. The task body, typing indicator, final reply, and sticky-board sync are unchanged.
- `stop_turn` takes the running handle from the supervisor, calls `session.stop_current()` first, then aborts. This preserves the "persist stopped before abort" order and the localized stopped/idle replies.
- The existing channel test now seeds the supervisor with a pending task instead of assigning `current_turn`.

### Verification

- `cargo test`: **125 passed, 0 failed** (123 unit + 2 real CLI process tests). New supervisor tests cover a rejected second start while active, `take_running` clearing exactly once, and a finished run not being treated as active. The existing `telegram_stop_persists_stopped_before_task_drop` mock Bot API test still passes.
- `cargo build --release`: zero Iskra warnings; the existing `proc-macro-error2 v2.0.1` future-incompatibility note remains.
- `rustfmt --check` (edition 2024): `supervisor.rs`, `channel.rs`, and `main.rs` clean.
- `cargo clippy --all-targets`: the same 5 pre-existing warnings in `channel.rs`, `locale.rs`, and `memory.rs`; none in new code.
- Budgets: stripped binary **6,394,384 bytes = 6.10 MiB**; direct dependencies **7**; CLI idle RSS **4,032 KB = 3.94 MiB** in an isolated dummy-config directory. No real `.env`, personal memory, Telegram request, or provider call was used.
- Not verified: owner-run Telegram `/stop`, busy notice, typing, final reply, and restart recovery. Durable queues and multi-agent execution remain later stages. No commit or push was performed.

## Stage 1 owner-run Telegram verification — passed (2026-11-23)

Status: **passed owner-run Telegram checks.** The automated Stage 1 goals were committed and pushed as `c67d824`, `b19c3e2`, and `2ce77dc`. The owner ran the checklist against one Telegram instance and DeepSeek V4.1 Flash (`deepseek-flash`); this entry records the log evidence the owner asked the agent to inspect.

### Results from `logs/agent.log`

1. **Restart recovery without automatic replay — passed.** `session_loaded records=11 unfinished=1` was followed by `telegram_start`. The first model call came only with a later owner turn (`turn_start run_id=12`), not at startup. After the second restart, `session_loaded records=15 unfinished=3` was likewise followed by no model call until `turn_start run_id=16`.
2. **`/stop` persistence — passed.** Work `id=12` logged `session_work_end id=12 state=stopped` before `telegram_stop aborted`, followed by `telegram_typing_stop`. Work `id=14` repeated the same order.
3. **Busy notice — passed.** While work `id=14` was active, a second message logged `telegram_busy`; no second `turn_start` appeared for that message.
4. **Typing activity — passed.** Runs 12–16 logged `telegram_typing_start`, repeated `telegram_typing_sent`, and `telegram_typing_stop` after the answer or stop.
5. **Tool round trip — passed.** Work `id=16` logged `model_call ... tools=[write_file]`, `tool_ok name=write_file`, `model_call ... tools=[read_file]`, `tool_ok name=read_file`, then `model_call ... tools=[none]`, `turn_end answer`, and `session_work_end id=16 state=Completed`. Work `id=15` also completed a multi-tool round trip.
6. **Usage recording — passed.** Every `model_call` line carried non-zero `prompt`, `completion`, and `total` counts.
7. **No protocol errors — passed.** `grep -c "model_error\|telegram_turn_error" logs/agent.log` returned **0**.

### Session state after the checks

`memory/session.json` reports `version: 1`, `next_id: 17`, and 16 records: 13 `completed` and 3 `stopped`. Session text content was not inspected.

### Cleanup and limits

- Removed the owner-created test files `workspace/live-check.txt`, `workspace/roundtrip.txt`, `workspace/cli-check.txt`, and `workspace/telegram-check.txt`.
- Kept `workspace/hello.txt` (Block 3 demo) and `workspace/trial_project/` (the Block 6 regression fixture that `SESSION.md` says to keep).
- Verification covered one owner, one instance, one provider/model, and the local Telegram channel. Multi-agent isolation, queues, ACP, and hosted behavior remain untested.

## v1.4 — Telegram reachability hardening (2026-11-24)

Status: **implemented, committed, and pushed as `97ff7ae`; owner-run live check passed.** The B3 live-proxy measurement remains unverified because no proxy was available.

### What changed

- `Cargo.toml`: reqwest now enables the `socks` feature. Direct dependencies remain **7**; the resolved package set is unchanged.
- `src/health.rs` (new): Telegram reachability state (`starting` / `ok` / `degraded` / `unreachable`), atomic 0600 JSON writes, a 30 s `get_me` watchdog, and unit tests. The snapshot lives at `memory/telegram_health.json` and contains timestamps, counters, and a short error category only.
- `src/channel.rs`: startup probes `get_me` inside a **20 s** window (1/2/4/8 s delays, 5 s per attempt), logs `telegram_start_retry`, writes `telegram_fatal`, and uses `try_dispatch_with_listener` instead of the panicking `dispatch`. Runtime polling errors go through a custom `ErrorHandler` and log `telegram_poll_error`; the watchdog records API reachability.
- `src/main.rs`: a channel startup failure exits with code **2** instead of a panic.
- `README.md`: new **Known limitations** section covering HTTP/SOCKS5 proxy variables, `socks5h://`, ignored `TELOXIDE_PROXY`, macOS system proxy not being read, the 20 s startup window, and the runtime health file.
- The health file's `last_api_ok` comes from the 30 s `get_me` watchdog, not from a `getUpdates` success hook: teloxide exposes no polling-success callback, and wrapping the stream would need a new direct dependency or a larger polling refactor. A silently stuck polling loop is bounded by the 17 s HTTP timeout.

### Verification

- `cargo test`: **136 passed, 0 failed** (134 unit + 2 real CLI process tests; baseline 125, +11 tests: 8 health unit tests, 1 runtime mock Bot API test, 2 helper tests).
- `cargo build --release`: zero Iskra warnings; the existing `proc-macro-error2 v2.0.1` future-incompatibility note remains.
- `rustfmt --check` (edition 2024): `health.rs`, `channel.rs`, and `main.rs` clean.
- `cargo clippy --all-targets`: the same 5 pre-existing warnings in `channel.rs`, `locale.rs`, and `memory.rs`; none in new code.
- Budgets: stripped binary **6,512,152 bytes = 6.21 MiB**; direct dependencies **7**; CLI idle RSS **3,984 KB = 3.89 MiB** in an isolated dummy-config directory.
- Startup failure: closed proxy port → exit **2**, 20.3 s, 5 attempts, `telegram_fatal reason=network`, no panic; blackhole proxy → exit **2**, 20.1 s, no panic. The health file reported `state=unreachable`.
- SOCKS5: the release binary completed SOCKS5 handshakes through a local sink. `socks5://` resolved `api.telegram.org` locally to `149.154.166.110:443`; `socks5h://` sent `api.telegram.org:443` as a domain request.
- Runtime mock: the integration test moved `ok → degraded → unreachable → ok` and produced `telegram_poll_error` lines. Test events append to the shared `logs/agent.log` because logging uses a process-global path; the test lines around ts 1795487017–1795487020 are not live failures.

### Owner-run live check

- A debug binary built from the new source (PID 84368, started 2026-11-24 05:29) ran against the real `.env`; the executor did not open or print `.env`.
- `telegram_start` appeared without any `telegram_start_retry`; `telegram_commands_registered` succeeded for en, ko, and ru; `telegram_fatal` count was **0**.
- `memory/telegram_health.json` reported `state: ok`, no errors, and `last_api_ok_unix` advanced **1795489097 → 1795489157** over the observation interval.
- One real turn completed: `session_work_start id=17`, `turn_start run_id=17`, `model_call model=deepseek-flash ... total=3950`, `turn_end answer`, `session_work_end id=17 state=Completed`, and `telegram_reply_sent chunks=1`.
- Two ESTABLISHED TCP connections to `149.154.166.110:443` were visible for the instance.

### Unverified

- **B3**: real Telegram receive/reply through a user-provided HTTP or SOCKS5 proxy. No proxy was available, so the Task B report keeps B3 as Unverified.
- Generalization to other networks, proxies, providers, or long-running outages.

### Next

- Close B3 when a proxy address is provided.
- Proceed to the Stage 2 design (two built-in HTTP agents, explicit `@agent`, independent state, FIFO) unless the owner chooses another next step.


## Stage 2 detailed design and three-session execution plan (2026-11-24)

Status: **design approved and documented; no Stage 2 Rust implementation or live verification performed.**

### What changed

- Expanded `docs/ARCHITECTURE.md` §12 Stage 2 with the strict `iskra.json` registry, isolated `AgentRuntime` ownership, independent paths and usage attribution, `runs.json` authority, Session projection, reachable Run transitions, FIFO/admission/recovery/cancellation ordering, and Telegram/CLI command contracts.
- Added `docs/WORKFLOW.md` §11: three sequential Goal sessions using the currently connected DeepSeek Flash without changing model or reasoning level. Unresolved decisions, owner-input requirements, new dependencies, or two failed approaches stop the Goal rather than triggering a model change.
- Added three exact Goal cards and the owner-run checklist to `SESSION.md`: (1) registry/runtime isolation, (2) durable RunStore/FIFO/Core Supervisor, (3) explicit routing/localization/end-to-end proof.
- Defined `main` compatibility with no `session/v1` migration, at most two Agents, per-Agent/global active limits 1/2, at most 20 unfinished queued Runs per Agent and 100 stored Runs, explicit `/resume`, targeted `/stop`/`/status`, and no automatic retry or delegation.

### Verification and limits

- This was a documentation-only design task. No Rust source, manifest, lockfile, real registry, `.env`, memory, workspace, or dependency was changed.
- The recorded baseline remains 136 passed, 0 failed from the v1.4 verification; tests and builds were not rerun merely to restate that baseline for this design entry.
- Registry validation, multi-Agent isolation, Run transitions, queues, cancellation races, restart behavior, localized routing, and two-provider execution remain unverified until their implementation Goals run.
- Owner-run Telegram/configured-provider verification stays outside all Goals and is required before any live-verified claim.
- No commit, push, external send, or real provider call was performed.

### Next

Run Stage 2 Goal 1 in a new Goal session with DeepSeek Flash and its current reasoning level unchanged. Goals 2 and 3 follow sequentially and inherit the working tree; none commits or pushes automatically.

## Stage 2 Goal 1 — Registry and isolated AgentRuntime (2026-11-24)

Status: **completed; automated verification passed; not committed.**

### What changed

- Added `src/registry.rs`: strict `version: 1` `iskra.json` parsing with only `engines` and `agents`, Agent ID constraints, duplicate detection, Engine reference checks, URL userinfo refusal, non-empty model/display/role checks, environment-variable-name validation for `api_key_env`, and the two-Agent bound. A missing file returns the synthesized compatibility `main`; any invalid file stops construction and is left untouched.
- Added `src/runtime.rs`: `RuntimeRegistry` owns `main` and at most one additional Agent. Each available Agent is an independent `AgentRuntime` with its own `Brain`, `Tools`, `Identity`, `Session`, and `InstancePaths`. A missing additional-Agent credential marks only that slot `unavailable`; it never falls back to another model or the main key.
- `src/instance.rs`: `InstancePaths::for_agent` derives `agent-workspaces/<id>/` and `memory/agents/<id>/`; `main` keeps `workspace/` and `memory/session.json`.
- `src/prompt.rs`: additional Agents read their own `SOUL.md` and the shared owner `USER.md`; the registry role is the missing-SOUL fallback (`IdentitySource::RegistryRole`). The main identity is unchanged.
- `src/run_context.rs` carries `agent_id`, `engine_id`, Run ID, tool schemas, and the matching executor. `src/loop_.rs` now takes the selected `AgentRuntime` and logs `model_call agent=... run_id=... engine=... model=...`; self-state usage is per Agent.
- `src/usage.rs`: added `today_for_agent`; legacy `model_call` lines without an Agent field count as `main`. The global `today()` remains available and is logged once at startup.
- `src/main.rs` and `src/channel.rs` compose from `RuntimeRegistry` and execute `main` only. Queueing, `memory/runs.json`, `@agent`, `/agents`, routing commands, dependencies, and `session/v1` changes were not added.
- Added `iskra.example.json` (credential-free) and ignored `/iskra.json` and `/agent-workspaces/`.

### Verification

- `cargo test`: **157 unit + 2 real CLI process tests = 159 passed, 0 failed**.
- New coverage: missing-registry compatibility and existing startup error text; strict version/field/ID/URL/engine/kind/duplicate validation; two-Agent model, workspace, memory, Session, and identity isolation; missing additional credential without fallback; registry-role SOUL fallback; per-Agent usage attribution with legacy `main` lines; additional path derivation.
- Changed files pass `rustfmt --check`.
- `cargo clippy --all-targets`: the same 5 pre-existing warnings in `channel.rs`, `locale.rs`, and `memory.rs`; none in new code.
- `cargo build --release`: zero Iskra warnings; the existing transitive `proc-macro-error2 v2.0.1` future-incompatibility note remains.
- Budgets: stripped binary **6,528,712 bytes = 6.23 MiB**; direct dependencies **7**; dummy-config CLI idle RSS **4,032 KB = 3.94 MiB** in an isolated temporary directory.

### Limits and next

- Not verified: owner-run Telegram/configured-provider behavior, actual routing to the additional Agent, Run/Queue transitions, and cross-turn live usage attribution. No commit, push, real provider call, or Telegram call was performed.
- Next: Stage 2 Goal 2 — durable RunStore, FIFO, and Core Supervisor, inheriting this uncommitted working tree.

## Stage 2 Goal 2 — Durable RunStore, FIFO, and Core Supervisor (2026-11-24)

Status: **completed; automated verification passed; not committed.**

### What changed

- Added `src/run.rs`: strict `version: 1` `memory/runs.json` store, `Core` dispatcher, per-Run `RuntimeEvent` projection, and the Run/Queue contract from `docs/ARCHITECTURE.md` §12.
- `src/supervisor.rs` now owns multiple run handles keyed by Run id and Agent. Admission authority moved to `Core` + `RunStore`; the supervisor only holds handles for cancellation.
- `src/loop_.rs` exposes `run_work`, which runs model/tool turns after the caller has created the Session work. Session completion and Run settlement stay with `Core` so their order is explicit.
- `src/main.rs` builds one `Core`, logs the main queue status, and waits for terminal `RuntimeEvent` values in CLI mode.
- `src/channel.rs` submits `main` work to `Core`, starts typing on `Started`, sends replies from `Completed`/`Failed`, and delegates `/stop` to `Core::cancel_agent`.
- `src/session.rs` exposes `FailureKind::as_str` for Run failure logs. `tests/session_cli.rs` now asserts `runs.json` terminal/interrupted states across processes.

### Run/Queue behavior

- Enqueue persists `queued` before acceptance. Input over 8,000 characters is refused; credentials are redacted at admission and the canonical text is used for both execution and Session persistence.
- Claim persists `preparing` under one dispatch lock. Dispatch creates the Session work, persists `preparing -> running`, then permits the first model/tool action.
- Completion persists the Session result first, then the Run terminal state, then publishes the event. A late settlement or start after cancellation is rejected and cannot reopen the Run.
- Per-Agent active concurrency is 1 and global active concurrency is 2. A later Run for the same Agent cannot overtake its head. A paused head blocks that Agent until explicit resume.
- `/stop` performs one Run-store write cancelling the Agent's active, queued, and paused work, then persists the Session stop, then aborts the handle. If the Run-store write fails, requested work is still aborted, no replacement work is dispatched, the store is poisoned, and the channel reports an error instead of a clean stop.
- Startup recovery maps `queued -> queued_paused` and `preparing/running -> interrupted`. No model or tool call is made until an explicit owner action.
- Unknown Agent references in `runs.json` fail closed before recovery writes, so the original file is preserved.

### Verification

- `cargo test`: **176 unit + 2 real CLI process tests = 178 passed, 0 failed** (Stage 2 Goal 1 baseline 159; +19 tests).
- New coverage includes every implemented/forbidden Run transition, atomic claim/settle, per-Agent FIFO, global concurrency 2, queue bound 20, paused resume, cancellation races and late events, cancellation write failure, poisoned-store reload, 100-record retention, credential redaction, input limits, invalid data fail-closed, startup no-replay, event sequence monotonicity, and two-Agent FIFO/concurrency against a local OpenAI-compatible mock.
- Existing real CLI process checks still pass: completed context replays after restart, and killed work recovers as `interrupted` with zero new model calls. Both `session.json` and `runs.json` were asserted.
- `cargo build --release`: zero Iskra warnings; the existing transitive `proc-macro-error2 v2.0.1` future-incompatibility note remains.
- `rustfmt --check` (edition 2024): all changed Rust files clean. `cargo clippy --all-targets`: the same 5 pre-existing warnings in `channel.rs`, `locale.rs`, and `memory.rs`; none in new code.
- Budgets: stripped binary **6,545,424 bytes = 6.24 MiB**; direct dependencies **7**; dummy-config CLI idle RSS **4,128 KB = 4.03 MiB**. All hard limits pass.
- Manual release-binary CLI check against a local OpenAI-compatible mock produced the answer, `runs.json` state `completed` with `session_work_id` and revision 4, and a completed `session/v1` record.

### Limits and next

- Not verified: owner-run Telegram/provider behavior, live routing to the additional Agent, live queue/stop/resume messages, and `/status` queue reporting.
- `Core::resume_agent` is implemented and tested, but the user-facing `/resume` command is Stage 2 Goal 3 scope. Until that command exists, work paused by a restart blocks later work for that Agent; this is an intermediate-stage limitation, not a live-verified behavior.
- No commit, push, real provider call, Telegram call, or dependency change was performed.
- Next: Stage 2 Goal 3 — explicit `@agent` routing, `/agents`, targeted status/stop/resume, localized text, and end-to-end proof.

## Stage 2 Goal 3 — Explicit routing, localized commands, and end-to-end proof (2026-11-24)

Status: **completed; automated verification passed; not committed.**

### What changed

- Added `src/route.rs`: one shared parser for directly entered text and command targets. Plain text goes to `main`; only a leading `@name` selects another Agent. Unknown, malformed, unavailable, and empty routes fail before any model or tool call. Commands may target a known Agent even while unavailable so paused work can still be cancelled; conversation routing still requires availability.
- `src/channel.rs`: `/agents`, `/status [@agent]`, `/stop [@agent]`, and `/resume [@agent]`; the command menu and `/help` now register seven commands; a localized queue acceptance notice names the busy Agent; status composition is shared with the CLI and adds a Work line (running/waiting/paused/interrupted). Forwarded text, stickers, and sticky-board replies stay on `main`.
- `src/main.rs`: CLI supports `@agent` routing and the same commands through the Core submission/event path.
- `src/locale.rs`: en/ko/ru strings for the new commands, agent lines, targeted status, queue acceptance, resume results, route errors, unknown commands, and oversized input. `with_values` fills multiple placeholders in order.
- `src/run.rs`: read-only `RunStore::run_state` / `Core::run_state` used for the queue notice; the `resume_agent` dead-code annotation was removed because the CLI now uses it.
- README (en/ko/ru): the two-Agent runtime, the explicit-routing rule, the local `iskra.json` requirement, and the fact that owner-run live verification is pending.

### Verification

- `cargo test`: **188 unit + 4 real CLI process tests = 192 passed, 0 failed** (Goal 2 baseline 178; +14).
- New coverage: plain-to-main; explicit `@helper`; `@` inside text; forwarded material never routed; sticker text stays on main; unknown, malformed, unavailable, and empty routes; empty command argument defaults to `main`; known-but-unavailable command target; `/agents` for available and unavailable Agents; targeted `/status` for both Agents; mock Bot API queue notice; targeted stop cancelling only the named Agent; resume of paused work with `queued_paused -> queued -> completed`; target errors without fallback.
- Real CLI processes with two mock OpenAI-compatible servers and a two-Agent `iskra.json`: distinct `model` values in the request bodies, `write_file` landing in `workspace/` for `main` and `agent-workspaces/assistant/` for the second Agent, separate `memory/session.json` and `memory/agents/assistant/session.json` records, two attributed records in `memory/runs.json`, and empty long-term memory in both roots.
- Invalid CLI routes (`@ghost`, bare `@`) produced localized errors and zero model requests.
- `rustfmt --check` (edition 2024): `route.rs`, `channel.rs`, `locale.rs`, `main.rs`, `run.rs`, `tests/session_cli.rs` clean.
- `cargo clippy --all-targets`: the same 5 pre-existing warnings in `channel.rs`, `locale.rs`, and `memory.rs`; no new warnings.
- `cargo build --release`: zero Iskra warnings; the existing transitive `proc-macro-error2 v2.0.1` future-incompatibility note remains.
- Budgets: stripped binary **6,628,272 bytes = 6.32 MiB**; direct dependencies **7**; dummy-config CLI idle RSS **4,160 KB = 4.06 MiB**. All hard limits pass.

### Limits and next

- At Goal 3 completion, owner-run Telegram and configured-provider verification, including live queue/stop/resume messages and en/ko/ru rendering, had not been performed. The later owner-run section below supersedes this status.
- `iskra.example.json` still shows only the single `main` profile; it was outside this Goal's allowed files. The owner configures the second Agent from `docs/ARCHITECTURE.md` §12.
- No commit, push, real provider call, Telegram call, or dependency change was performed.
- Next: owner-run Stage 2 checklist in normal mode with the current DeepSeek Flash configuration, then a commit decision.

## Stage 2 owner-run Telegram verification — passed (2026-11-24)

Status: **passed the Stage 2 owner-run checklist in normal mode.** The Stage 2 code remains uncommitted on top of `c4bf7e0`.

### Setup

- Created a local, Git-ignored `iskra.json` with two Agents, `main` and `second`, sharing one `openai_compatible` Engine: `deepseek-flash` at the existing DeepSeek endpoint. The registry referenced the existing `DEEPSEEK_API_KEY` variable name; no credential value or `.env` content was read, printed, or changed.
- The model and reasoning configuration were left unchanged. One Telegram instance served both Agents. The previous debug instance was stopped before the check.
- Startup logs reported `core_ready agents=2`, both Agent runtimes available with `main-engine / deepseek-flash`, and command registration for `en`, `ko`, and `ru`.

### Results

1. **Agent listing — passed.** `/agents` showed `@main` (Iskra) and `@second` (Second), both on `main-engine / deepseek-flash`, available, with zero running/waiting/paused work.
2. **Routing and isolation — passed.** Plain input wrote `workspace/stage2-main.txt`; `@second` wrote `agent-workspaces/second/stage2-second.txt`. Test markers appeared only in `memory/MEMORY.md` and `memory/agents/second/MEMORY.md` respectively. Two attributed records appeared in `memory/runs.json`, and main and second Session files advanced independently. `model_call` lines carried `agent=main|second`, `run_id`, `engine=main-engine`, and `model=deepseek-flash`.
3. **Concurrency, FIFO, and queue acknowledgement — passed with a wording observation.** Runs 5 (`main`) and 6 (`second`) were active at the same time. Run 7 (`main`) was accepted while run 5 was active, logged `telegram_run_queued`, and started immediately after run 5 completed, before run 6 finished. The Korean notice read `접수했어요. Iskra가 지금 일하는 중이라, 끝나면 시작할게요. 기다리는 요청: 0개.` The number is computed as queued requests excluding the newly accepted request. This is behaviorally consistent with "requests ahead", but the bare "Waiting: 0" wording is ambiguous after the request has been accepted.
4. **Targeted status, stop, and empty resume — passed.** `/status @second` reported the second Agent's model, memory, workspace counts, tokens, and work line. `/stop @main` logged `run_cancelled id=8`, then persisted `session_work_end id=22 state=stopped`; Run 8 ended `cancelled`. `/resume @second` with no paused work replied that no paused request existed.
5. **Restart with running and queued work — passed.** Before restart, runs 9 (`main`) and 10 (`second`) were `running`, and run 11 (`second`) was `queued`. After restart, recovery logged `session_recovered` for both running works, `run_store_loaded runs=11 paused=1 interrupted=2`, and stored runs 9/10 as `interrupted` and run 11 as `queued_paused`. No `model_call` occurred between process start and explicit owner action.
6. **Explicit resume and no interrupted replay — passed.** `/status @second` showed `1 paused, 1 interrupted`; `/resume @second` logged `run_resumed agent=second count=1`, started only run 11, and completed it with one `model_call` (total 3203 tokens). Runs 9 and 10 stayed `interrupted`; `/status @main` continued to report `interrupted 1`.

### Verification scope and limits

- No code changed during this owner-run check, so `cargo test` was not rerun. The automated Stage 2 Goal 3 baseline remains **192 passed, 0 failed**; the live run exercised the same uncommitted binary built from that tree.
- Both live Agents referenced the same Engine/model. Distinct-model routing remains proven only by the automated two-mock-server CLI test.
- Actual rendered messages were checked in Korean. Registration for all three locales was confirmed in logs, but live English and Russian message rendering was not exercised.
- The check covered one owner, one instance, one provider/model, one Telegram channel, and no proxy. It does not generalize to groups, multiple users, ACP, hosted mode, or network failure during a run.

### Cleanup

- Removed the two generated workspace files and both generated memory markers. Additional-Agent runtime directories, Sessions, and `runs.json` records were retained as execution history rather than edited by hand.
- No commit or push was performed.

## Stage 2 Remediation Goal R1 — Core/channel correctness (2026-11-24)

Status: **completed; focused checks passed; not committed.** R2 and R3 are not started.

### What changed

- `src/channel.rs` gained two coverage points only: the turn watcher must end on a `StorageFailed` event with the storage-error reply, and a Telegram resume must deliver the paused answer while leaving an interrupted Run terminal and adding no queue notice.
- The rest of R1 was already present in the working tree when this session started (partial edits from an interrupted session). It was checked item by item against the card and preserved without rewriting:
  - Board replies stay on `main`: a numeric reply removes a sticky; every other board reply, including one starting with `@agent`, is submitted to `main`. Forwarded text, stickers, and unknown-slash handling are unchanged.
  - `Core::resume_agent` returns the resumed `RunTicket`s. Telegram subscribes before resuming and reuses `watch_turn` with no queue notice; the CLI prints every resumed answer in FIFO order.
  - Claim validation (`run_id + revision + preparing`), `session.begin`, and `preparing -> running` run under the Core admission lock, so a cancelled task creates no Session work and publishes no spurious terminal event.
  - A Session or RunStore stop-write failure aborts the requested work, poisons admission/dispatch, returns the error, and emits `StorageFailed` instead of a clean cancellation. No replacement work is dispatched and later submissions are refused.
  - `/agents` and `/status` show an explicit unavailable queue line when `queue_status` fails instead of zero counts.
  - Event-sequence pruning removes only terminal entries, so an active Run keeps a monotonic sequence after more than 256 other Run IDs.

### Verification (actual commands)

```text
cargo test --bin iskra run::tests
  -> 23 passed, 0 failed, 172 filtered out

cargo test --bin iskra channel::tests
  -> 26 passed, 0 failed, 169 filtered out   (25 before this session's additions)

cargo test --test session_cli
  -> 5 passed, 0 failed

rustfmt --edition 2024 --check src/channel.rs src/run.rs src/main.rs src/locale.rs tests/session_cli.rs
  -> clean
```

- R1 coverage: non-numeric board reply to `main` with zero helper calls; Telegram resume answer delivery with the interrupted Run untouched and no queue notice; real two-process CLI restart, resume of two paused Runs in FIFO order, and no model call for the interrupted Run; cancel before and after `session.begin`; Session and RunStore stop-write failures with abort, `StorageFailed`, no replacement dispatch, and later admission refusal; poisoned queue status unavailable; active sequence monotonic past 256 other Run IDs.
- An orientation `cargo test` run was completed before the new card was visible: **194 unit + 5 process tests = 199 passed, 0 failed**. It is not used as R1 proof and was not repeated, because the card forbids full-suite, clippy, release, and budget checks in R1.

### Limits

- Telegram resume delivery is proven with a mock Bot API and an in-process Core. The owner-run restart/resume delivery, board-reply, targeted `/stop`, second-dispatcher refusal, and touched Korean strings remain in the R3 live checklist.
- No commit, push, dependency change, personal-memory edit, or R2/R3 work was performed.

## Stage 2 Remediation Goal R2 — Single dispatcher and canonical Agent isolation (2026-11-24)

Status: **completed; focused checks passed; not committed.** R1 remains complete; R3 is not started.

### What changed

- New `src/run_lock.rs`: `DispatcherLock::acquire(memory_dir)` opens `memory/dispatcher.lock` with `O_NOFOLLOW`, mode `0600`, and takes `flock(LOCK_EX | LOCK_NB)`. The descriptor is owned by `Core` and therefore by every run task holding an `Arc<Core>`. Normal release only unlocks; the file is never unlinked, and the kernel releases a crashed or killed owner's lock, so no stale-file heuristic exists.
- `src/main.rs`: the composition is now read-only registry validation (`RuntimeRegistry::load_plan`) → lock acquisition → runtime construction (`from_plan_env`, which performs Session recovery) → `Core::open`, which performs RunStore recovery. A lock conflict logs `dispatcher_lock_error`, prints `Dispatcher error: Another Iskra dispatcher already owns this data directory ...` to stderr, and exits 1. Nothing before the lock mutates Session, RunStore, model, or Telegram state. `Core::new` without a lock is `#[cfg(test)]` and is not reachable from production.
- `src/instance.rs`: `prepare_for_agent` returns main's existing paths unchanged. Each additional root (`agent-workspaces/<id>`, `memory/agents/<id>`) is created one component at a time using `symlink_metadata`, so no component symlink is followed; a symlink, an existing non-directory, or a canonical target that differs from the approved derived path is refused.
- `src/runtime.rs`: registry plan loading is separated from runtime construction. All Agent roots are prepared and cross-checked before any Session load: an additional root may not equal or nest with main's workspace, a main root, or another additional Agent root, while the approved `memory/agents/<id>` nesting under main's memory stays valid. A rejected root therefore cannot trigger main's `running -> interrupted` recovery write.
- Startup recovery after the lock is held is unchanged: `queued -> queued_paused`, `preparing/running -> interrupted`, no automatic dispatch.
- `tests/session_cli.rs`: real-process coverage for the second dispatcher and normal-exit reacquisition.

### Verification (actual commands)

```text
cargo test --bin iskra run_lock::tests
  -> 2 passed, 0 failed, 201 filtered out

cargo test --bin iskra instance::tests
  -> 8 passed, 0 failed, 195 filtered out

cargo test --bin iskra runtime::tests
  -> 7 passed, 0 failed, 196 filtered out   (6 before this session's addition)

cargo test --bin iskra run::tests
  -> 23 passed, 0 failed, 180 filtered out

cargo test --bin iskra channel::tests
  -> 26 passed, 0 failed, 177 filtered out

cargo test --test session_cli
  -> 6 passed, 0 failed   (5 before this session's addition)

rustfmt --edition 2024 --check src/run_lock.rs src/instance.rs src/runtime.rs src/run.rs src/main.rs tests/session_cli.rs
  -> clean
```

- Proof coverage:
  - A live delayed-work CLI process holds the lock; a second process exits nonzero with the English refusal before any model call, and `memory/runs.json` and `memory/session.json` remain byte-identical. The first process resumes and answers normally.
  - After the first process exits normally, a new process acquires the lock, makes zero model calls, and leaves the completed Run terminal; the existing owner-kill test still acquires the lock after `SIGKILL` and performs no-replay recovery.
  - Second lock acquisition fails inside one process and succeeds after the guard drops; the lock file survives normal release; a symlinked lock file is refused.
  - Symlinked additional workspace or memory roots pointing at main's workspace, main's memory, an outside directory, or through a symlinked parent are all refused, with the seeded main Session's bytes unchanged and nothing written through the alias.
  - Non-symlinked additional roots are still created and used for isolated agents, as covered by the existing two-Agent runtime test.
- Forbidden and not run in this Goal: full suite, clippy, release build, budget measurement, commits, pushes, R3 work, personal-memory edits.

### Limits

- The owner-run second-process refusal and the remaining R3 live checklist are still pending. The automated results use the real binary and real filesystem but do not prove Telegram behavior.
- The cross-root nesting check is written generically, but Stage 2 admits at most one additional Agent, so the exercised alias cases are the additional root against `main`.
- No commit or push was performed.

## Stage 2 Remediation Goal R3 — Log privacy, compatibility proof, docs, and milestone (2026-11-24)

Status: **completed; full milestone checks passed; not committed.** R1 and R2 remain complete. The owner-run post-R3 checklist has not been run.

### What changed

- `src/brain.rs`: `ChatError` is now a structured value (`kind`, optional HTTP `status`, human-readable `detail`). It keeps the existing owner-facing detail (including a truncated provider body) for `TurnError` and the Session failure category, but exposes `category()` and `status()` for logs.
- `src/loop_.rs`: `model_error` logs `agent`, `run_id`, `engine`, `model`, `category`, and `status` and no longer includes the error string. `tool_error` logs `agent`, `run_id`, `engine`, and the tool `name` and no longer includes raw arguments or the error text.
- `src/brain.rs`: `balance_error` logs `model`, a category (`timeout`, `connection`, `http`, `parse`), and the optional HTTP status. The provider body stays only in the owner-facing error value.
- `tests/session_cli.rs`: an `ErrorServer` and a first-response mock support two canary tests and one legacy-compatibility test:
  - A 401 provider and balance failure with unique prompt/provider-body canaries. The log contains the structured `model_error ... category=auth status=401` and `balance_error ... category=http status=401` lines, no canary, while the owner-facing stderr still contains the provider detail.
  - A failing `read_file` whose path is a unique canary. The log contains `tool_error agent=main run_id=1 engine=main-engine name=read_file`, and neither the prompt canary nor the path canary nor the tool error text appears.
  - A real CLI run starting from an existing `session/v1` file with no `runs.json` and a legacy `model_call` line without an Agent field. It proves main replay of the legacy record, `/status` reporting the legacy 12 tokens as main usage, new Runs attributed to `main` and `assistant`, an empty assistant history, and no merge into the assistant Session.
- `src/instance.rs`, `src/run_lock.rs`: test-only `.err().expect(...)` calls became `.expect_err(...)`. This removes five clippy warnings introduced with the R2 test helpers and leaves only the four pre-existing warnings outside this Goal's files; no runtime behavior changed.
- `docs/ARCHITECTURE.md`: status line, single-dispatcher lock and lock-before-recovery order, `dispatcher.lock` in both layouts, canonical/symlink-checked additional roots, `StorageFailed` cancellation projection, resumed-result delivery, the expanded failure-log contract, non-numeric board replies staying on `main`, and Stage 2 verification additions.
- `docs/WORKFLOW.md` §11: a Stage 2 remediation subsection records that `SESSION.md` cards are the source of truth, focused checks run at handoffs, and the full milestone runs once after R3.
- README (en/ko/ru): corrected the stale owner-run claim (the pre-remediation Stage 2 checklist passed on 2026-11-24, the post-remediation checklist is pending), the registry location (working directory, not "next to the binary"), and the single-instance and canonical-root limits.

### Verification (actual commands and counts)

```text
cargo test
  -> 203 unit tests passed, 0 failed
  -> 9 real CLI process tests passed, 0 failed
  -> 212 total passed, 0 failed

rustfmt --edition 2024 --check src/brain.rs src/loop_.rs src/instance.rs src/run_lock.rs tests/session_cli.rs
  -> clean

cargo clippy --all-targets
  -> 4 pre-existing warnings (channel.rs:354, locale.rs:19, memory.rs:108, memory.rs:133)
  -> no warning in an R3-changed file; the five R2 test-helper warnings were removed

cargo build --release
  -> zero Iskra warnings; the existing transitive proc-macro-error2 v2.0.1 future-incompatibility note remains

strip target/release/iskra (temporary copy)
  -> 6,694,568 bytes = 6.38 MiB   (release binary 8,956,304 bytes)

direct dependencies
  -> 7 (unchanged; no dependency added)

dummy-config CLI idle RSS, isolated temp directory, after 2 s waiting on stdin
  -> 4,224 KB = 4.13 MiB
```

- All hard budgets pass: stripped binary 6.38 MiB ≤ 20 MB, idle RSS 4.13 MiB ≤ 30 MB, direct dependencies 7 ≤ 15.
- Canary and legacy tests are part of the 9 real-process tests. No real provider, Telegram request, or personal memory was used.

### Limits

- The owner-run post-R3 checklist is still pending: board replies, restart `/resume` answer delivery, normal targeted `/stop`, second-dispatcher refusal, and the touched Korean strings. This record does not claim live verification.
- Live en/ru rendering and distinct-model routing remain automated-only or previously owner-checked only as recorded above.
- No commit, push, dependency change, or personal-memory edit was performed.

## Stage 2 post-R3 owner-run checklist and reply attribution patch (2026-11-24)

Status: **live checklist passed; reply attribution patched and automatically verified; one live label check pending; not committed.**

### What was verified live

A Telegram instance using the local `main` + `second` registry ran the post-R3 checklist. Evidence was taken from `logs/agent.log` after the live window; the later `cargo test` noise (Run ids reset to 1) was excluded.

- **Board reply stays on main**: a non-numeric reply to the live board beginning with `@second` created Run 13 with `agent=main`; no `second` Run existed in that window. The board remained live afterwards.
- **Numeric board removal**: replying `1` to the board logged `telegram_sticky_remove number=1` and `telegram_board_torn_down`.
- **Restart and resume delivery**: Run 14 (`main`, long request) was running when Run 15 (`main`, follow-up) was queued; the queue acceptance path logged `telegram_run_queued run_id=15 agent=main`. After restart, recovery logged `run_store_loaded runs=15 paused=1 interrupted=1` and `run_queue agent=main active=0 queued=0 paused=1 interrupted=2`. `/resume @main` logged `run_resumed agent=main count=1`, `run_claimed id=15 agent=main`, `telegram_resume agent=main resumed=1`, and `telegram_reply_sent`. Run 14 remained `interrupted` and was never re-claimed.
- **Targeted stop**: `/stop @second` during Run 16 logged `run_cancelled id=16 agent=second`, `telegram_stop aborted agent=second cancelled=1 active=1`, and `telegram_run_cancelled run_id=16`. A second `/stop @second` logged `telegram_stop idle agent=second`.
- **Second dispatcher refusal**: with the lock-aware debug instance holding `memory/dispatcher.lock`, the release binary run as `./target/release/iskra --cli` exited 1 with `Dispatcher error: Another Iskra dispatcher already owns this data directory (lock: memory/dispatcher.lock)`. `dispatcher_lock_error` was logged, and `memory/runs.json` and `memory/session.json` hashes were byte-identical before and after.
- **Korean UI**: the owner confirmed `/start`, `/help`, `/agents`, `/status @second`, and `/stickies` rendered in Korean.

### Environment finding

A stale pre-lock debug process (PID 4237, started at 10:01) was still polling the same bot from the same working directory. `telegram_poll_error … kind=api` first appeared at 13:45:20, immediately after the lock-aware instance started at 13:45:14, and caused `telegram_health state=degraded` with repeated API failures. With owner approval the stale process was terminated; no new poll error appeared afterwards and the health file returned to `state=ok`, `consecutive_poll_failures=0`.

### Product gap and patch

Live replies from both Agents arrived through the same bot with no attribution. The cause was `watch_turn` sending the final text through `send_reply` unchanged (`src/channel.rs`). Patch:

- Added `attributed_reply(agent_id, text)`: `main` and empty text are unchanged; other Agents get a first-line `@<agent_id>` label.
- Applied it to completed answers, structured failure text, and the storage-failure notice. Resumed deliveries use the same watcher and are labeled automatically.
- The label is added before the 4,096-character split, so only the first chunk carries it. Stored Session/Run content is unchanged; the label is presentation-only. CLI output is unchanged.
- Updated the existing resume mock assertion to expect `@helper\nlate answer`; added tests for main/helper/empty labeling, first-chunk-only labeling, and a helper completed answer through `watch_turn`.

### Verification (actual commands and counts)

```text
cargo test
  -> 206 unit tests passed, 0 failed
  -> 9 real CLI process tests passed, 0 failed
  -> 215 total passed, 0 failed   (R3 baseline 212; +3)

rustfmt --edition 2024 --check src/channel.rs
  -> clean

cargo clippy --all-targets
  -> 4 pre-existing warnings (channel.rs:354, locale.rs:19, memory.rs:108, memory.rs:133)
  -> none in the new code

cargo build --release
  -> zero Iskra warnings; the existing transitive proc-macro-error2 v2.0.1 note remains

strip target/release/iskra (temporary copy)
  -> 6,711,104 bytes = 6.40 MiB

direct dependencies
  -> 7 (unchanged)

dummy-config CLI idle RSS, isolated temp directory, after 3 s waiting on stdin
  -> 4,160 KB = 4.06 MiB
```

- All hard budgets pass: stripped binary 6.40 MiB ≤ 20 MB, idle RSS 4.06 MiB ≤ 30 MB, direct dependencies 7 ≤ 15.

### Limits

- The reply label has not been observed live yet; the checklist requires one `@second` request and one plain request after restart.
- en/ru rendering was not exercised in this live run; locale completeness remains covered by tests. Distinct-model routing also remains automated-only because both local Agents use `main-engine` / `deepseek-flash`.
- Running `cargo test` appends test events to the real `logs/agent.log` because the logger uses a process-global path; the recorded live evidence excludes those entries.
- No commit, push, dependency change, or personal-memory edit was performed.

### Live label check (final, 2026-11-24)

Status update: **complete.** The patched debug binary was restarted at 14:23. Run 17 (`second`) completed and sent one chunk at 14:23:45–46; Run 18 (`main`) completed and sent one chunk at 14:24:06. The owner confirmed that the `second` answer began with `@second` and the plain `main` answer had no label. This closes the only pending acceptance criterion for the reply-attribution patch. No commit or push was performed.


## Stage 2.5 design promotion — delivery/v1 (2026-11-24)

Status: **approved architecture recorded; no code, Goal, commit, push, or test run.**

### What changed

- `docs/ARCHITECTURE.md` now records Stage 2.5-A `delivery/v1` as the next approved implementation contract before Stage 3. The additions cover the Core-owned delivery projection, `memory/deliveries.json`, pre-send persistence, strict state semantics, no automatic retry of `unknown`, one plain-text fallback only for a definitive Markdown rejection, chunk/attribution truth, event-lag recovery, `/status` delivery counts, owner-only model-free `/result <run-id>`, fail-closed corrupt-store handling, bounded retention, and Stage 2.5-A verification requirements.
- The architecture status and invariants now separate Run completion from delivery. Existing `session/v1`, `runs.json`, and registry formats are explicitly unchanged.
- README (en/ko/ru) and the public site roadmap now say the approved next slice is delivery reliability, not Stage 3. They do not claim that it is implemented.
- This record only promotes the already-approved design. It does not create a Goal or authorize code changes.

### Baseline and verification

- Repository: `master` HEAD `7b281cb`; the tree was clean before this documentation change and now contains the architecture, README/site, and Stage 2.5 record edits uncommitted. Stage 2 is committed as `3d3c98f`.
- Last recorded baseline: **215 passed, 0 failed** (206 unit + 9 real CLI process, 2026-11-24). Tests, builds, clippy, and resource measurements were not rerun for this documentation change.
- Recorded budgets remain historical: stripped binary 6.40 MiB, dummy-config CLI idle RSS 4.06 MiB, 7 direct dependencies.

### Limits and next step

- No implementation, real Telegram/provider call, owner-run verification, commit, or push was performed.
- The next step is owner review of the compact Stage 2.5-A task card. A separate explicit instruction is required before creating a Goal or starting implementation.


## Commercial validation P0 — paid Launch first (2026-11-24)

Status: **business strategy and execution card recorded; documentation only; no paid product, payment, deployment, outreach, code change, commit, or push.**

### What changed

- `docs/BUSINESS.md` now defines the beachhead customer, three commercialization scenarios, the recommended paid-Launch → managed sequence, experimental offer boundaries, rollout gates, unit-economics limits, and commercial stop rules.
- `SESSION.md` adds the Commercial validation P0 card, four-part defensibility contract, first-14-day sequence, productization sequencing gate, and expansion/stop rules.
- The initial offer hypothesis is a $300 assisted setup with one provider, two isolated Agents, three verified workflows, recovery instructions, and 14 days of bounded support. The $49/month managed founding pilot remains subject to retention and measured cost. Neither is a final public price or a revenue forecast.
- Current public reference checks: MausBot lists free local desktop, Cloud plans at $15/$49/$99, and Teams from $40/month for two people; OpenBot lists free local use and paid hosting/business licensing. These are competitor-published claims and price anchors, not independent demand or cost evidence.
- Stage 2.5-A `delivery/v1` remains the current technical implementation target. The commercial plan does not supersede it or start a parallel code Goal. ACP/delegation now requires paid-demand evidence after delivery reliability rather than being the default next business phase.

### Verification and limits

- Repository baseline before this entry: `master` HEAD `b5af990`. The latest recorded code baseline remains **215 passed, 0 failed**; tests, builds, clippy, and resource measurements were not rerun because only business/session documentation changed.
- Final `git diff --check` passed after this log entry. Final repository status shows only `docs/BUSINESS.md`, `SESSION.md`, and this log modified; no new files were created.
- No customer discovery, actual payment, installation on a customer machine, managed hosting, backup/restore, or productization workflow has been performed. Revenue and timeline figures are hypotheses and arithmetic examples only.
- Owner action and separate approval remain required for payment/tax/refund setup, intake publication, external outreach, account creation, deployment, and case-study publication. The next code step remains owner review of the compact Stage 2.5-A task card; the next business step is the P0-S offer/intake package.


## Outcome-first Headless strategy and visual product assets (2026-11-24)

Status: **approved strategy, architecture and six visual assets completed and inspected; documentation/design only; no new code Goal, paid launch, provider account, customer outreach, deployment, commit or push.**

### What changed

- `docs/BUSINESS.md` supersedes the earlier `$300 generic installation` offer and rejected vertical-service-first draft. It now defines a MausBot-like free Local → paid Managed Always-on → Team path, with a model-neutral Headless Agent Control Plane, Outcome/evidence metrics, BYOUI projections, GLM as a validation wedge rather than a permanent identity, and managed design-partner gates.
- `docs/ARCHITECTURE.md` keeps Stage 2.5-A `delivery/v1` as the current implementation target while separating future Outcome, Headless, BYOUI and Lease contracts from current behavior. It adds Outcome ≠ Task completion, authorized UI projections, lease-expiry/no-replay rules, `outcome/v1`/`headless/v1` direction and future verification gates.
- `SESSION.md` adds the Outcome-first strategy, the P0-H demand card, sequential—not concurrent—technical cards, commercial stop rules and the visual-asset result.
- Public-reference research now includes Vercel Board's official experimental message/task primitive, claims and renewable leases, dependencies, watchers/schedules, structured data, grants, event logs and external sync, plus its US-only storage, default visibility, profile-permission and sync-spill limits. Iskra will not clone Board; an optional future adapter requires separate terms/security review.
- The strategy also incorporates Anthropic Managed Agents, LangGraph, Microsoft Agent Framework, Temporal, MCP/A2A and Z.AI's official GLM/Claude Code integration. No representative evidence was found for a mass migration of Russian developers to GLM.

### Visual deliverables

Created the following target/concept assets with synthetic content and explicit **NOT IMPLEMENTED** labels:

- `docs/assets/iskra-outcome-model.svg`
- `docs/assets/iskra-byo-ui.svg`
- `docs/assets/iskra-headless-strategy.svg`
- `docs/assets/iskra-control-plane-architecture.svg`
- `docs/assets/iskra-final-product.svg`
- `docs/assets/iskra-ops-wireframe.png`

The images cover Outcome → Task → Run → Evidence, six outcome metrics, BYOUI projections and security boundaries, vertical-service versus MausBot versus Headless strategy, the final Control Plane, Local/Managed/Team product family, and the Outcome Board with acceptance, cost, human intervention, lease and approval.

### Verification (actual)

- XML parsing with Python `xml.etree.ElementTree`: **5 SVG files passed**.
- `sips` rendering: all five SVGs rendered at **1600×1000**; the Outcome Board PNG is **1800×1100**.
- `ReadMediaFile`: all six final rendered images were inspected for aspect ratio, clipping, Korean text, panel/button visibility and Outcome/BYOUI semantics. No right-side cropping remained.
- An initial Quick Look thumbnail pass produced square, horizontally cropped images and was rejected as an invalid verification artifact. Correct `sips` renders replaced it; no final deliverable uses the cropped render.
- `git diff --check`: passed before this log entry. Final checks follow the log update.
- No Rust code, tests, manifests or runtime assets changed. The latest recorded code baseline remains **215 passed, 0 failed**; tests/builds were not rerun for this documentation/design work.

### Limits and next step

- These are target visuals, not screenshots of an implemented product. Headless/Outcome/BYOUI/Lease/Cloud/GLM compatibility and managed pricing remain unimplemented or unvalidated as explicitly marked.
- No customer interview, actual payment, GLM live-provider call, customer deployment or production UI was performed.
- The next code step remains owner review of the compact Stage 2.5-A card. The next business step is the P0-H demand interviews. Each subsequent technical card requires its own scope, state transitions, proof and explicit start instruction.


## Authority Hub anti-absorption strategy and competitor correction (2026-11-24)

Status: **approved competitive architecture and validation card recorded; documentation only; no new code Goal, adapter, partnership, provider account, customer outreach, deployment, commit or push.**

### What changed

- `docs/ARCHITECTURE.md` adds the target Authority and integration-independence contract. Iskra must own canonical Identity, Authority, Outcome, Evidence and Delivery while external Agents/models/tools are replaceable capabilities and UIs are replaceable projections.
- New target contracts: `adapter/v1`, `capability/v1`, `policy/v1`, `event/v1` and `evidence/v1`, alongside `outcome/v1`, `headless/v1` and `delivery/v1`. The document defines external principal/scope, CloudEvents-compatible events, anti-loop IDs, canonical storage, detachable adapters, least-privilege delegation, export/restore and no-replay behavior.
- `docs/BUSINESS.md` adds the Authority Hub and anti-absorption strategy, platform-specific authority boundaries and a required AgentField/CodeAF/OpenClaw bake-off. Iskra must prove practical advantages rather than claim a unique category.
- `SESSION.md` adds the P0-H cross-vendor proof questions and one design-only HUB-PROOF vertical integration card: external request → untrusted adapter → Outcome → scoped execution → independent evidence → scoped delivery → provider swap and UI detach.
- The revised win condition is the smallest Outcome-verified Authority Hub: single binary, acceptance/evidence, BYOUI, no invisible replay, Local → Managed recovery and cross-provider evaluation.

### Competitor evidence used

- AgentField docs: Agents as APIs, service discovery, shared memory, deferred execution, human approval, identity, verifiable credentials, access policies, decision receipts, signed audit chain, multiple SDKs and `docker compose`.
- CodeAF docs: managers/subteams, budgets, standing orders, schedules, remote machines and headless mode.
- OpenClaw official docs: local multi-channel assistant, desktop/Gateway, Node/Bun, daemon, backup/update/migration and hosting options.
- Hermes Agent official site confirms a free open-source agent with memory/skills and desktop downloads. Its Kanban, Telegram budgets and approval gates were not verified.
- No official evidence was found for Guild.ai, VDF AI Networks or Elestio managed OpenClaw, so those claims are excluded from the strategy.

### Verification and limits

- No Rust code, tests or manifests changed. Latest recorded code baseline remains **215 passed, 0 failed**, not rerun for this documentation work.
- Final `git diff --check` passed. No customer interviews, actual provider calls, AgentField installation, partnership or paid validation was performed.
- Stage 2.5-A `delivery/v1` remains the current implementation target. HUB-PROOF is a design card, not authorization to implement adapters or start another Goal.
- Stop conditions now include no practical advantage over AgentField/CodeAF/OpenClaw, vendor-owned canonical state, permission bypass, unrelated custom workflows or negative managed economics.


## G1 desk evidence — cross-vendor Agent operation pain (2026-11-25)

Status: **G1 passed; public evidence documented; no code, paid offer, outreach, or external account change.**

### What changed

- Created `docs/research/demand-evidence.md` as the only project deliverable. It records dated public evidence with source URLs and direct quotations for the interview-free demand ladder.
- Adopted **17 records** of cross-vendor Agent operation pain and **6 explicit requests** for a layer owning shared state, authority, policy, or evidence, exceeding the G1 threshold of 8 records and 3 layer requests.
- Sources cover AgentField, OpenClaw, ccusage, Z.AI/ZCode, OpenAI Codex, r/LocalLLaMA, r/ClaudeAI, Hacker News, Habr, and OKKY. CodeAF cases are included as supporting evidence but excluded from the primary count because the multi-Agent/Provider condition is not explicit.
- The repeated failures are state drift, retry overwrites, restart duplicate execution/delivery, ineffective permission revocation, and missing execution/audit evidence. This is not a token-price-only or GLM-setup-only result.

### Verification and limits

- Base HEAD: `34b8e58f4efe298f7514ddc7f6c8570905391f5f` on `master`.
- Structural check: 18 `E` headings, 17 primary records, 6 layer requests, no missing metadata or quotes, no trailing whitespace, and 24 Markdown source links.
- `git diff --check` passed. The new untracked document was checked separately for structure and trailing whitespace.
- No Rust code, tests, or manifests changed. `cargo test` and builds were not rerun; the latest recorded code baseline remains **215 passed, 0 failed**, not a new measurement.
- `E16` is excluded because its original date metadata was not independently verified. Korean first-person multi-Agent incident evidence, willingness to pay, Managed demand, representative GLM migration, and Iskra's practical advantage over existing orchestrators remain unverified.
- The `$49 Agent Ops Audit` remains a pre-existing price hypothesis from `SESSION.md`; this research did not derive or validate that price.
- The next business action is owner review of the evidence followed by a separate decision to prepare the G2 offer and asynchronous intake. No payment route, listing, or outreach was created.


## Token/quota workflow — session splitting and targeted reads (2026-11-25)

Status: **documentation-only operating-rule update; no implementation Goal, Rust code change, live verification, commit or push.**

### What changed

- `AGENTS.md` now limits startup reads to the current task card, the previous relevant result, the latest applicable decisions and targeted specification sections. It prohibits rereading unchanged ranges without a new fact or contradiction.
- `docs/WORKFLOW.md` now requires one session per deliverable and a new session at research/design/implementation/owner-verification handoffs. Changing `/model` alone is not a context-reduction strategy.
- The initial context threshold is roughly 100k tokens before compaction. Routine parallel agents and duplicate multi-model review are excluded; escalation is sequential after two failed approaches or a security/design conflict.
- Gemini is assigned long-document comparison and multimodal first passes; Muse is assigned repetitive browser/app checks. Only compressed evidence and unknowns enter the project session. Existing owner-approval rules remain unchanged.

### Evidence and limits

- Local Kimi Code log counters for 23 Iskra sessions recorded 1,984 LLM calls, about 381.2 million cumulative processed input tokens and about 1.70 million output tokens. There were 163 turns above 100k context tokens and 121 above 200k; the maximum was 678,508.
- Cache reads represented 97.1% of input. These are processing counters, not invoice or quota deductions. The first optimization target is fewer calls, smaller sessions and fewer repeated reads.
- No Rust tests or builds were rerun. The latest recorded code baseline remains **215 passed, 0 failed**, stripped binary **6.40 MiB**, dummy-config CLI idle RSS **4.06 MiB**, and **7 direct dependencies**.
- Pre-existing uncommitted changes in `SESSION.md` and `docs/BUSINESS.md` were preserved. No product, payment, provider, external account or watchpost change was made.

### Next step

Apply and measure this workflow from Stage 2.5-A `delivery/v1`, but do not start that implementation without a separate owner instruction. Compare LLM calls, context size, rereads, actual test results and provider quota views over the next two or three tasks before claiming any savings percentage.


## Stage 2.5-A Goal 1 — Core-owned delivery store (2026-11-25)

Status: **completed; focused automated verification passed; not committed or pushed.**

### What changed

- Added `src/delivery.rs` with a strict `delivery/v1` store for `memory/deliveries.json`. Records hold delivery identity, Run and Agent identity, channel, owner-facing target, chunk count, per-chunk state/attempt/fallback/message id, and revision. No answer, prompt, tool output, credential, or raw error text is stored; the final answer remains only in the Session projection.
- Chunk transitions are `pending → sent | failed | unknown`; `failed → pending` only by explicit resend; `unknown → sent | failed` only by reconciliation. `sent` requires a positive message id. Chunk attempts are appended in send order, so chunks not yet attempted are absent rather than counted as attempted.
- `open` maps every unresolved `pending` chunk to `unknown` in one atomic write and performs no send, settlement, or Run work. A missing store is empty and creates no file. Corrupt JSON, invalid UTF-8, unsupported versions, unknown fields, and invalid references fail closed and preserve the original bytes. Later write failures poison the store until reload.
- Retention is bounded at 200 records; only fully sent records are prunable. When unresolved records fill the bound, creation returns `RetentionFull` without touching the file.
- `RunStore::run_target` exposes the Agent, Run state, and Session work reference. `Session::completed_answer` returns only a completed work item's stored redacted answer. `Core` owns the delivery store and exposes `run_target`, `create_delivery` (Run-derived Agent, terminal Runs only), `completed_answer`, and `delivery()`.
- `src/main.rs` registers the new module. No channel sending, user-facing command, schema, dependency, registry, or real runtime data change was made.

### Verification

- `cargo test delivery::`: **17 passed, 0 failed**.
- `cargo test run::`: **24 passed, 0 failed** (new run-target and Core delivery/answer query assertions included).
- `cargo test session::`: **17 passed, 0 failed** (new completed-answer query included).
- `rustfmt --edition 2024 --check src/delivery.rs src/run.rs src/session.rs src/main.rs`: clean.
- Test coverage includes allowed and forbidden transitions, message-id-required `sent`, restart `pending → unknown` without sending, explicit resend and reconciliation, attempt bounds, strict invalid-data preservation, private atomic writes, retention without unresolved pruning, and the absence of answer/prompt/tool/error text in delivery records.
- The focused filters add 19 unit tests to the previous 206-unit baseline (delivery 17, run 1, session 1). The full suite and real CLI process tests were not rerun in this Goal; the milestone gates stay in Goal 4.
- Tests append `delivery_store_loaded`, `delivery_store_error`, and delivery transition lines to the process-global `logs/agent.log`; these are test metadata, not live evidence.
- Session-level LLM call, token, model-distribution, and reread counters are not observable from inside the Goal session; the measurable evidence is the commands and results above.

### Limits and next step

- Not verified in this Goal: channel integration, pre-send behavior against a Bot API mock, real CLI delivery, Telegram/provider behavior, full-suite counts, clippy, release build, and budgets. No owner-run check was performed.
- The new Core/delivery APIs are not yet called by production code, so the binary reports expected dead-code warnings until Goal 2 wires delivery sends.
- Next: Stage 2.5-A Goal 2 — Telegram send classification and chunk tracking, preserving this working tree. No commit or push without separate owner approval.

## Stage 2.5-A Goal 2 — Telegram send classification and chunk tracking (2026-11-25)

Status: **completed; focused automated verification passed; not committed or pushed.**

### What changed

- `src/channel.rs` now routes terminal Run replies through Core's delivery authority. The final text is built first: non-`main` Agent attribution is prepended, and failure text is already localized, then the result is split into 4,096-character chunks. Only then does `Core::create_delivery` create the delivery identity (Agent taken from the Run, target = owner chat id).
- Per chunk the order is strict: `begin_chunk` persists `pending` with attempts = 1, the Bot API send runs, and `settle_chunk` records the result. Chunks that were never attempted stay absent instead of being counted as attempted.
- Send classification: `sent` requires a returned positive message id; `markdown_rejected` maps only `ApiError::CantParseEntities` and is the sole path to one recorded plain-text fallback (`begin_fallback`, attempts = 2) for the same logical chunk; `rejected` covers recognized structured Bot API errors, rate limits, and chat migration; `ambiguous` covers network/timeout/reset, invalid JSON, and unrecognized API descriptions. Ambiguous results settle as `unknown`; no second send is ever attempted.
- A failed, unknown, or storage-aborted chunk stops later chunks. A partial answer therefore persists and is logged as sent plus unknown plus unattempted counts, not as full delivery.
- Delivery admission failure, or a first-chunk pre-send write failure, produces no reply chunk on Telegram; the channel sends one localized `ErrorDeliveryStorage` notice instead. A mid-answer storage failure stops later sends without adding an untracked notice over content the owner can already see.
- `RuntimeEventKind::StorageFailed` notices, command replies, queued notices, and sticky-board messages remain outside delivery tracking. A storage-failed Run may not be terminal, so no valid delivery identity exists for that notice; this is the only intentionally untracked run-facing send.
- `src/locale.rs` adds `ErrorDeliveryStorage` in en/ko/ru. Logs add `telegram_delivery_chunk`, `telegram_delivery_report`, `telegram_delivery_refused`, `telegram_delivery_storage_error`, and `telegram_notice_error` with IDs and classifications only. Reply text is never logged.
- The helper-attribution watcher test now drives a real helper Run instead of injecting an event for a synthetic Run id, because delivery admission requires a terminal Run record.

### Verification

- `cargo test channel::`: **37 passed, 0 failed** (29 prior channel tests + 8 new).
- `cargo test delivery::`: **17 passed, 0 failed** (unchanged from Goal 1).
- Full suite: **233 unit + 9 real CLI process tests = 242 passed, 0 failed**. Goal 1 left 225 unit tests, so Goal 2 adds 8.
- New tests: admission refusal with no answer send; persisted message id; one Markdown-only plain fallback with `attempts = 2` and `fallback_used = true`; definitive rejection as `failed` with one request; ambiguous connection reset and unrecognized API description as `unknown` with one request each and no retry; three-chunk partial send persisting two chunks as sent + unknown and leaving the third unattempted; zero Bot API calls when the pre-send write fails after the delivery identity exists; helper attribution plus Korean failure localization before chunking.
- Mock Bot API supports scripted success ids, structured API error bodies, and a connection reset without a response. Success in these tests is not live Telegram proof.
- `rustfmt --edition 2024 --check src/channel.rs src/locale.rs src/delivery.rs src/run.rs src/session.rs src/main.rs`: clean.
- `cargo clippy --all-targets`: no warnings in new code; the recorded pre-existing `channel.rs`/`locale.rs`/`memory.rs` warnings and Goal 1's expected dead-code notes remain.
- `cargo build --release`: exit 0. The Iskra crate reports **5 warnings**, all dead-code notes for Goal 1 primitives that Goal 3–4 wire next (`Reconciliation`, `DeliveryCounts`, `run_target`, `completed_answer`, and related store queries); none points at Goal 2 code. The existing transitive `proc-macro-error2 v2.0.1` future-incompatibility note remains.
- Tests append delivery and channel log lines to the process-global `logs/agent.log`; these are test metadata, not live evidence.

### Limits and next step

- Not verified in this Goal: real Telegram delivery and rendering, owner-run localization, `/status` delivery counts, `/result`, durable watcher recovery, and resource budgets. Those remain Goal 3–4 and the owner-run gate.
- Existing `session/v1`, `runs.json`, and registry formats, CLI behavior, dependencies, and the inherited Goal 1 files are unchanged.
- Next: Stage 2.5-A Goal 3 — durable terminal recovery, preserving this working tree. No commit or push without separate owner approval.


## Transaction-first survival strategy revision (2026-11-25)

Status: **documentation strategy approved and synchronized; no Rust code change, new implementation Goal, external research, paid offer, customer outreach, deployment, commit or push by this task.**

### What changed

- Reworked `docs/research/iskra-survival-strategy.md` §5 onward into a **transaction-first, one-incident, one-productization-path** strategy while preserving the user-supplied research inputs without re-verifying them. The long-term Outcome-verified Authority Hub remains the target, not the current implemented product category.
- Replaced the broad `$49 Agent Ops Audit` with the fixed-scope **Agent Replay & Delivery Audit** hypothesis: one restart duplicate-execution or duplicate-delivery incident, one state/authority map, one reproduction fixture, one deterministic check and one rerunnable evidence bundle. The name, price, 200-view and five-intake targets remain unvalidated hypotheses.
- Removed the unimplemented Headless Outcome API as an audit-delivery prerequisite. After Stage 2.5-A, the audit uses the existing Iskra CLI/Run/Session/Delivery path and a fixed local audit kit, recording manual work. Paid diagnosis proves neither Outcome API nor Headless nor Managed demand.
- Added a productization gate of two paid audits requesting the same capability or one actual paid product pilot before a minimal `outcome/v1`, deterministic `evidence/v1` and one Outcome Pack card. ACP, Headless, BYOUI, additional packs, `doctor` and `export` are separate demonstrated-need gates rather than an automatic bundle.
- Changed Managed validation from proposal acceptance to actual payment. Expansion still requires three paying workspaces, two four-week retentions, recurring operation without manual repair, restore/export/deletion and measured total cost.
- Updated `docs/BUSINESS.md` §0/§6/§7 and `docs/ARCHITECTURE.md` §5/§12 to match these gates. Approval records now explicitly authorize effects rather than serving as acceptance evidence.
- Updated `README.md`, `README.ko.md` and `README.ru.md` to emphasize verified durable runs, explicit routing and no automatic restart replay without advertising unimplemented Outcome/Headless capabilities. The roadmap no longer makes ACP the automatic next Stage.
- Added the current strategy card and superseded-rule notes to `SESSION.md`; preserved historical entries. Updated the personal `NOW.md` and `DECISIONS.md` with the decision, conditions and next action.

### Verification

- `git diff --check` passed on the tracked changes before this log entry; a final post-entry check is part of closeout.
- Targeted searches found no remaining active broad `Agent Ops Audit`, future-product-path audit prerequisite, Managed proposal-acceptance pass, three-pack requirement or automatic post-delivery ACP instruction in `docs/BUSINESS.md`, `docs/ARCHITECTURE.md` or the README roadmaps. Superseded rules remain only as clearly marked history in `SESSION.md` and the preserved research-input section.
- The new untracked strategy document has no trailing whitespace. References point to existing `docs/BUSINESS.md` §0/§6/§7, `docs/ARCHITECTURE.md` §5/§12/§13 and the new strategy §5–10.
- This strategy task made no Rust source, test, manifest or runtime-data edits. Existing Stage 2.5-A Goals 1–2 changes were preserved, including `src/channel.rs` and `src/locale.rs` records that appeared from a concurrent Goal 2 session during this documentation work.
- No `cargo test`, release build or owner-run check was performed by this strategy task. The concurrently recorded Goal 2 result of **242 passed, 0 failed** is inherited session state, not a test result produced by this documentation work.

### Limits and next step

- Actual audit demand, repeated productization demand, Managed payment, competitive advantage and product outcomes remain unverified. No listing, payment path, customer-data collection or external publication was launched.
- At this record, the next code card is **Stage 2.5-A Goal 3 — durable terminal recovery**, preserving Goals 1–2 and requiring a separate implementation instruction. The next business task after P0 is one synthetic fixed-scope audit sample plus offer/intake/payment drafts in normal mode.
## Stage 2.5-A Goal 3 — Durable terminal recovery (2026-11-25)

Status: **completed; focused automated verification passed; not committed or pushed.** Goal 1–2 changes were preserved.

### What changed

- `src/run.rs` adds a read-only terminal projection. `TERMINAL_POLL_INTERVAL` (500 ms) bounds every event wait. `RunTarget` now exposes the stored `failure` category, and `Core::terminal_outcome` returns `Completed { answer }` (the stored redacted Session answer, `None` if unavailable), `Failed { category }`, `Cancelled`, or `Interrupted`, or `None` while the Run is unknown/non-terminal. It performs no Run, model, or tool work and does not change `session/v1`, `runs.json`, or the registry format.
- `src/delivery.rs` adds `DeliveryStore::existing_attempt(run_id, channel, target)`. It reports any existing delivery record, including an unresolved one, so recovery cannot duplicate a send or retry an `unknown` outcome automatically.
- `src/channel.rs` `watch_turn` now treats RuntimeEvents as hints. Each wait is bounded; a timeout, a lagged stream, or a closed stream falls back to the durable Run/Session projection. A terminal outcome is handled at most once per watcher: the event path and the recovery path share one loop and the first terminal observation ends the watcher. Recovery sends only when no delivery attempt exists for the same Run/channel/target; it logs `telegram_recovery_delivery_exists` and stays silent otherwise.
- Recovery handling: completed runs send the stored answer through the normal tracked-delivery path and re-sync the sticky board; failed runs send the localized category text; cancelled and interrupted runs send nothing; a completed run without a stored answer sends one localized `ErrorSession` notice and records no delivery. Every terminal, recovery-error, and closed-stream path drops the typing guard before leaving.
- `src/main.rs` replaces the unbounded CLI event wait with `wait_for_run`/`recover_run`. The CLI polls the same durable projection, prints the first terminal result once, and uses the stored answer for a completed Run even when the event was missing, lagged, or the stream closed. Event-backed failure text stays as before; recovered failures use the localized category text; `interrupted` states that it will not resume automatically. `/resume` output uses the same path.
- New logs contain IDs, states, and classifications only: `telegram_recovered`, `telegram_recovery_delivery_exists`, `telegram_recovery_delivery_error`, `telegram_recovery_answer_missing`, `telegram_recovery_error`, `telegram_recovery_unavailable`, `cli_recovery_error`, `cli_recovery_answer_missing`, and `cli_event_closed`. No reply, prompt, or tool text is added.

### Verification

- `cargo test channel::`: **47 passed, 0 failed** (37 before; +10 recovery tests).
- `cargo test run::`: **27 passed, 0 failed** (24 before; +3 terminal-projection tests).
- `cargo test delivery::`: **18 passed, 0 failed** (17 before; +1 existing-attempt test).
- CLI wait unit tests in `src/main.rs`: **5 passed, 0 failed** (missing, duplicated, lagged, closed-with-terminal, closed-without-terminal).
- `cargo test --test session_cli`: **10 passed, 0 failed** (9 before; +1). The new real-process test prints a completed answer exactly once, then restarts the process and proves no reprint and no model call. The existing two-process restart, resume, legacy-session, lock, and canary tests still pass.
- Full suite: **252 unit + 10 real CLI process tests = 262 passed, 0 failed** (233 unit + 9 CLI before Goal 3; +19 unit, +1 CLI).
- New coverage: missing Completed event recovered from Session; lagged stream; closed stream with and without a terminal Run; duplicated terminal events send once; existing delivery attempt suppresses resend; durable failure category; missing stored answer reported once; cancelled recovery silent; Started-then-missing-Completed ends the watcher; recovery makes zero model calls; terminal projection for failed/cancelled/interrupted/missing-answer and `None` while active; restart publishes no terminal events.
- `rustfmt --edition 2024 --check src/channel.rs src/main.rs src/run.rs src/delivery.rs tests/session_cli.rs`: clean.
- `cargo clippy --all-targets`: the same four pre-existing warnings (`channel.rs:358`, `locale.rs:19`, `memory.rs:108`, `memory.rs:133`); no new warnings. Goal 1's expected dead-code notes remain because Goal 4 still wires those queries.
- `cargo build --release`: exit 0 with the same four Iskra dead-code warnings for Goal 1 primitives (`Reconciliation`, `DeliveryCounts`, `run_target`, related store queries); no Goal 3 code warning. The existing transitive `proc-macro-error2 v2.0.1` future-incompatibility note remains.
- Not run in this Goal, as the card leaves them to Goal 4: full milestone formatting/clippy/budget gates and resource measurements.
- Real CLI tests use local mock OpenAI-compatible servers and dummy credentials only.

### Limits and next step

- Mock Bot API success is not live Telegram proof. Owner-run Telegram delivery, restart recovery, rendering, and en/ko/ru checks remain after Goal 4.
- Tests append channel/CLI metadata to the process-global `logs/agent.log`; those lines are test metadata, not live evidence.
- No schema change, dependency, new command, automatic restart watcher/send, or automatic retry was added.
- Next: Stage 2.5-A Goal 4 — localized `/status` delivery counts, model-free `/result <run-id>`, and the full automated milestone.
## Stage 2.5-A Goal 4 — Localized status/result and milestone proof (2026-11-25)

Status: **completed; full Stage 2.5-A automated milestone passed; not committed or pushed.** Goal 1–3 changes were preserved.

### What changed

- `src/locale.rs` gained en/ko/ru strings for the delivery status line, delivery-unavailable state, the `/result` command description, usage text, unknown run, not-finished run, missing answer, and unreadable store. Command lists in `/start` and the Telegram menu now include `/result`.
- `src/channel.rs` adds a per-Agent delivery line to `/status [@agent]`: waiting (pending), refused (failed), uncertain (unknown), and the number of answers not fully delivered. It is reported after the Work line and separately from Run/Queue state. A poisoned or invalid delivery store reports the localized `unavailable` line, logs `delivery_status_error`, and never reports zero. Telegram startup logs a `telegram_delivery_start` summary.
- Telegram `/result <run-id>` parses exactly one positive integer, then reads Run and Session only. Missing/malformed/zero arguments, unknown runs, non-completed runs, and completed runs without a stored answer return localized errors and send no answer. A completed Run's stored redacted answer is attributed for non-`main` Agents and sent through a new explicit tracked `delivery/v1` record. Each invocation is a new owner action; `sent` requires a message id, an ambiguous send settles as `unknown` without automatic retry, and a poisoned store sends one localized storage notice instead of the answer. Logs contain run ids, states, and delivery ids/classifications only.
- `src/main.rs` adds CLI `/result`, which prints the stored answer once and writes nothing. CLI help lists the command. The CLI path performs no Run, model, or tool call and logs `cli_result run_id=… state=…` without the answer.
- `src/delivery.rs` change is annotation-only. `counts`, `error`, and `get` are now used by production code, which removed the inherited Goal 1 dead-code notes. `Reconciliation`, `reconcile_chunk`, and `resend_chunk` keep documented `#[allow(dead_code)]` annotations: the approved `delivery/v1` contract requires explicit owner resend and channel-proven reconciliation, but Stage 2.5-A adds no command for them. They remain test-covered.
- No `session/v1`, `runs.json`, or registry schema change; no dependency; no automatic retry; no provider, approval, context, or tool-policy change.

### Verification

- `cargo test channel::`: **54 passed, 0 failed** (+7). `cargo test delivery::`: **18 passed, 0 failed**. `cargo test run::`: **27 passed, 0 failed**. `cargo test session::`: **17 passed, 0 failed**. CLI wait units: **5 passed**.
- `cargo test --test session_cli`: **11 passed, 0 failed** (+1). The new real-process test runs one turn, then `/result 1` in a second process: the answer is printed once, the mock model receives no new request, Run and Session files are byte-identical, and the log contains `cli_result run_id=1` but not the answer. Malformed and unknown arguments are localized and make no model request.
- Full suite: **259 unit + 11 real CLI process tests = 270 passed, 0 failed** (252 + 10 before Goal 4).
- New coverage: per-Agent delivery counts and separation from Work state; poisoned-store `unavailable`; explicit Telegram `/result` delivery with a persisted message id; repeated invocation as a new record with no automatic retry; ambiguous `/result` send as `unknown`; en/ko/ru usage/unknown/non-completed/missing-answer errors with no answer leak; poisoned-store `/result` refusal without contacting the model; real CLI `/result` zero-model behavior and unchanged bytes.
- `cargo fmt --all -- --check`: exit 0. `cargo clippy --all-targets`: the same four pre-existing warnings (`channel.rs:368`, `locale.rs:19`, `memory.rs:108`, `memory.rs:133`); none in new code.
- `cargo build --release`: exit 0 with zero Iskra warnings. The existing transitive `proc-macro-error2 v2.0.1` future-incompatibility note remains.
- Budgets: stripped release binary **7,140,448 bytes = 6.81 MiB** (limit 20 MiB); dummy-config CLI idle RSS **4,192 KB = 4.09 MiB** (limit 30 MiB); direct dependencies **7** (limit 15).

### Limits and next step

- Mock Bot API success is not live Telegram proof. Owner-run `/status` delivery counts, `/result`, successful/partial/unknown delivery, en/ko/ru rendering, attribution, and restart recovery remain the separate live gate.
- Tests append channel/CLI metadata to the process-global `logs/agent.log`; those lines are test metadata, not live evidence.
- The `#[allow(dead_code)]` annotations on explicit resend/reconciliation primitives are intentional contract annotations, not a behavior change; a later card may connect an owner command.
- No commit or push was performed. Next: the owner-run Telegram checklist in normal mode; no further Stage starts automatically.

---

## Stage 2.5-A owner-run Telegram verification (2026-11-25)

Status: **live gate passed; no commit or push.** Working tree: `master` `aac5656` plus the uncommitted Stage 2.5-A Goals 1–4 changes; release binary 9,061,648 bytes. The test instance was started with `./target/release/iskra` (Telegram mode) and stopped afterwards.

### Method

- Live Telegram account and DeepSeek Flash; no model, provider, or configuration change.
- Live idle RSS after startup: 19,936 KB (limit 30 MiB).
- Partial delivery was produced on purpose by a local watcher that polled `memory/deliveries.json` every ~2 ms and sent `SIGKILL` when a new delivery record showed one `sent` chunk and one `pending` chunk. The kill is test tooling, not product behavior.

### Observed

- Successful delivery: run 19 → delivery 1 was persisted `pending` before the Bot API call, settled `sent` with `message_id 368`, and `telegram_delivery_report ... sent=1 failed=0 unknown=0 unattempted=0`. `memory/deliveries.json` is `0600` and holds ids, states, target, and message ids only.
- `/status` (Korean app language): Work and Delivery lines were separate; Delivery showed `0 waiting, 0 refused, 0 uncertain · 0 answers not fully delivered`. `/help`, the command menu, and the status labels were Korean; startup registered command menus for `en`, `ko`, and `ru`.
- `/result 19` twice: deliveries 2 and 3 were each newly created and `sent` (`message_id 372`, `374`), with no automatic retry. `/result abc` and `/result 0` produced the localized usage error (`telegram_result usage`); `/result 999` logged `state=unknown`; `/result 14` (interrupted Run) logged `state=Interrupted` and returned the localized not-finished message.
- Non-main delivery: `@second ...` (run 20) was recorded as delivery 4 `agent=second` and `sent` (`message_id 386`); `/status @second` reported `Agent: Second (@second)` with its own delivery counts. The reply's `@second` prefix was not separately quoted in the owner report; attribution was confirmed from delivery/status records.
- Interrupted send and restart: run 22 read `workspace/live_long.md` (6,072 characters) and produced a two-chunk delivery (id 6). Chunk 0 was `sent` (`message_id 392`); chunk 1 was `pending` when the process was killed. Restart logged `delivery_store_loaded deliveries=6 unknown=1` and rewrote chunk 1 to `unknown` (revision 5) with no send, settlement, or Run replay. The owner confirmed no message arrived automatically after restart, and the log contains no `telegram_delivery_chunk` after restart.
- `/status` after restart reported `0 waiting, 0 refused, 1 uncertain · 1 answer not fully delivered`. `/result 22` created delivery 7 with both chunks `sent` (`message_id 397`, `398`); `telegram_result_delivery id=7 ... sent=2`.
- Record 6 stays unresolved by design: an explicit `/result` is a new owner delivery, not channel-proven reconciliation of the `unknown` chunk, and no automatic retry exists.
- Live-log canaries: no answer text (`오래 가는 소프트웨어`, `유지보수의 무게`: 0 matches) and no `sk-` token or key/token assignment (0 matches).

### Limits

- Partial/`unknown` came from a genuinely interrupted Telegram send, but the interruption itself was deliberately injected locally.
- Localization was verified live in Korean only; `en`/`ru` were confirmed as registered command menus at startup, not as in-app language renders.
- The extra `/status`, `/help`, and `/result` calls were explicit owner actions; no other live command was exercised.
- No commit or push. Next: owner decision on committing Stage 2.5-A.

### Commit (2026-11-25)

- Committed as `bd62255` and pushed to `origin/master` at 2026-11-25 16:12:45 +0300; the working tree is clean. This supersedes the "No commit or push" status above and closes Stage 2.5-A.

---

## P1 — Agent Replay & Delivery Audit launch package (2026-11-25)

Status: **local execution package prepared; automated checks passed; no external publication, payment, customer-data collection, deployment, commit or push. Starting baseline: `master` `c769deb`, clean.**

### What changed

- Added `docs/AUDIT-LAUNCH.md`, an owner-facing immediate runbook covering local verification, a timed four-owner-hour sample redelivery rehearsal, intake creation, payment/settlement feasibility, placeholder replacement, two-channel publication, daily measurement, first-order delivery, owner-time budgeting, and day-7/day-14 stop decisions.
- Added a fixed-scope English offer at `docs/audit/index.html` and a public synthetic evidence report at `docs/audit/sample.html`. The offer preserves the unvalidated `$49` hypothesis, one-incident scope, ten-question intake, 48-hour target, exclusions, and data-handling boundary. It contains an intentional DRAFT notice and `{{INTAKE_URL}}` placeholders so it cannot be published correctly without owner setup.
- Added `docs/audit/intake.md` with exactly ten asynchronous questions, no file uploads, qualification/rejection rules, and an explicit rule that form completion or proposal acceptance is not payment.
- Added `docs/audit/marketing.md` with a 60-second synthetic demo, Herdr/community and purchase-listing drafts, a no-CTA public-issue contribution, later Russian/Korean summaries, UTM templates, and measurement definitions.
- Added `docs/audit/replay-delivery/`: a synthetic interrupted two-chunk delivery fixture, an eight-check zero-dependency Node.js audit script, and a README containing the state/authority map, verdict template, exact commands, and public-claim boundary.
- Appended the Herdr and adjacent-competitor scan to `docs/research/iskra-survival-strategy.md` §11. Herdr issue reports remain labelled as user reports rather than confirmed defects; Odysseus, AgentLedger, AgentField/CodeAF, Google AX, OpenClaw, and Vibe Kanban prevent generic-runtime, generic-plugin, or free-adoption assumptions.
- Corrected stale Stage 2.5-A status text in README 3, `docs/index.html`, `docs/ARCHITECTURE.md`, `docs/BUSINESS.md`, and the survival strategy. These documents now distinguish completed delivery reliability from unimplemented Outcome/Evidence/Headless/ACP and the new local audit package.
- Review corrections: the survival strategy §7 P0 roadmap now records Stage 2.5-A as completed instead of pending; the audit checker derives its post-restart summary fields from fixture evidence rather than hardcoding them; and both draft public pages carry temporary `noindex,nofollow` plus an explicit launch-gate step to remove it only after final checks.

### Verification

- `node docs/audit/replay-delivery/check.mjs` → `state: PASS`, **8 passed, 0 failed**. The checker validates synthetic/no-payload classification, event order, Run/Delivery ownership, crash state, unknown conversion, zero automatic recovery effects, explicit `/result` identity, and unresolved original state plus unique message ids. It performs no network, model, Telegram, filesystem-write, or customer-data operation.
- `cargo test restart_maps_pending_to_unknown_without_sending_or_rerunning` → **1 passed, 0 failed**.
- `cargo test --test session_cli cli_restart_does_not_reprint_or_resend_completed_work` → **1 passed, 0 failed**; the test proves no completed-answer reprint and no second model request after restart.
- `cargo test --test session_cli cli_result_prints_a_stored_answer_without_a_model_call` → **1 passed, 0 failed**; the test proves model-free result output with unchanged Run/Session bytes.
- Final `cargo test` → **270 passed, 0 failed** (259 unit tests + 11 real CLI process tests).
- HTML parsing and local-link checks over `docs/index.html`, `docs/audit/index.html`, and `docs/audit/sample.html` → PASS; both draft pages have `noindex,nofollow`; the intake contains exactly ten questions. New-file trailing-whitespace checks and fixture JSON parsing → PASS. `git diff --check` → PASS.
- Injected-failure checker run → `state: FAIL` with derived post-restart delivery attempts `1`, model calls `1`, and original-unknown resolution `false`, matching the modified fixture instead of the passing constants.
- Environment used for the audit checker: Node.js `v22.22.2`; no package installation or new Core dependency.
- No release build, binary/RSS budget measurement, live Telegram request, real Herdr reproduction, rendered external-browser deployment, four-owner-hour delivery, or paid transaction was performed by this task.

### Limits and next step

- The fixture is explicitly synthetic; the focused Rust tests use local mocks and dummy credentials. Neither proves universal agent safety, a customer fix, Herdr compatibility, or a confirmed Herdr defect.
- Publication remains blocked on owner review, real form/payment/refund/tax/settlement setup, replacement of all public placeholders, removal of the DRAFT notice, deployed-link checks, and separate approval for external action and any commit/push.
- Actual customer demand, repeated productization demand, a four-owner-hour rerun, and Managed payment remain unverified.
- Next: owner executes `docs/AUDIT-LAUNCH.md`; no next Core implementation is approved.

---

## Headless/v1 design review — strategy judgment, source pass, price anchors (2026-11-26)

Status: **design-only session; no code, dependency, commit or push. Baseline `master` `0d31697`, clean. `cargo test` not rerun (recorded 270 passed, 0 failed).**

### What changed

- Recorded the design brief in `SESSION.md`'s headless/v1 card: zero new state transitions (reuses ARCHITECTURE §7), the `headless/v1` contract (three endpoints, strict schema, error codes), a six-line threat model, G1–G3 proof/stop additions, and dependency candidates.
- Appended source-level competitive findings to `docs/research/iskra-survival-strategy.md` §11.4.
- Added price anchors to `docs/AUDIT-LAUNCH.md` (Step 3.5).
- Updated `NOW.md` next action and baseline.

### Strategy judgment

- GO for the approved headless/v1 slice. T3 is the most load-bearing thesis: if a competitor ships external-effect safety by default and buyers find it sufficient, both the slice's wedge and §5's single product promise collapse.
- Source pass facts: AgentField requires PostgreSQL 15+, defaults to all-interface bind, and has durable execution events, leases and signed execution VCs with input/output hashes — so "they lack durability or evidence" is false and must not be claimed. Their documented posture clears `idempotency_key` on inbound replays so dedup does not reject re-dispatch (`migrations/032...`), and restarts create a new execution linked via `restarted_as_execution_id` (`036...`). Herdr's resume types the agent's own resume command into the restored pane (`src/agent_resume.rs`); no external-effect layer exists in that scope. A public comparison may claim posture difference (replay-by-default vs no-invisible-replay) only after a runtime bake-off.
- Price desk research (2026-11-26, search-result snippets; the FAULTLINE page was unreachable): nearest agent-audit comparable is FAULTLINE Rapid Diagnostic at $500 one-time; ayfar quotes $150–300/month monitoring; Omium and AgentShield offer free audits as lead magnets; Upwork 2026 intermediate developer rates are $40–70/hr with an AI median around $30/hr; one blog example prices a wrong $4,200 refund at $7,300 total cost. Self-hosted one-time license precedents are $99–500 (spryn, MailFlow). Managed-instance anchors are $10–20/month, and managed OpenClaw hosting already exists. Verdict: keep $49 for the first three transactions (demand proof, not margin); the $150–500 band gives market support for later audit pricing and the $99–500 band for the box license, but only P3 gate data may fix them.

### Limits

- No installation, execution, or runtime reproduction of either competitor; no code copied. AgentField's crash-recovery policy for in-flight executions remains unverified (application code not read). Herdr #5231 remains a user report. FAULTLINE tier details remain unverified beyond the snippet.
- External search failed twice with HTTP 403 and two price pages timed out before a later search succeeded; the price figures above come from search snippets, not full-page verification.

### Next

- Owner approves one HTTP server dependency (axum recommended; size impact to be measured in G1), then G1 starts in its own session.

---

## headless/v1 G1 — HTTP surface and token admission (2026-11-26)

Status: **G1 complete; automated verification and a local smoke run passed; not committed or pushed.** Baseline `master` `0d31697` with the design-session documentation changes preserved.

### Scope and behavior

- Added `src/headless.rs` and registered it in `src/main.rs`. The server starts only when `iskra.json` contains an optional `headless` block; a missing registry keeps the compatibility `main` behavior and starts no HTTP surface.
- `headless` configuration: `{ "bind": "127.0.0.1:8787", "token_env": "ISKRA_OWNER_TOKEN", "allow_non_loopback": false }`. `bind` defaults to `127.0.0.1:8787`; the bearer token is read from the named environment variable and never stored in `iskra.json`. A non-loopback bind is rejected unless `allow_non_loopback` is explicitly true. A missing or empty token, an unavailable bind, or a second server on the same data directory stops startup instead of running unauthenticated.
- Endpoints: `POST /v1/runs`, `GET /v1/runs/{run_id}`, `GET /v1/runs/{run_id}/evidence`. Requests require `Authorization: Bearer <token>`, compared in constant time. Unknown fields, missing fields, and malformed JSON return 400; bodies over 64 KiB return 413; unknown paths and run ids return 404; wrong methods return 405.
- `POST /v1/runs` validates `prompt` and the required `idempotency_key`, then admits exactly one Run through the existing Core path with `Core::admit`. `admit` shares `submit`'s checks, `RunStore::enqueue`, and `dispatch_locked`; it only avoids reporting a post-admission dispatch failure as a failed admission. The HTTP layer never dispatches, executes tools, or writes Run state itself. `QueueFull` maps to 429 and a RunStore failure maps to 503.
- `GET` handlers read the persisted Run projection only. G1 intentionally omits timestamps and result fields because `runs.json` stores neither, and it reports `unknown` for every evidence value that G3 will later compute. No model call, tool call, or usage-counter change occurs on a read.
- `idempotency_key` is required and bounded but **not deduplicated** in G1 (G2). Restart recovery stays the existing Core behavior; this Goal adds no new state transition.
- `src/registry.rs` gained the strict optional `headless` block validation; `src/run.rs` gained the shared admission helper. `docs/ARCHITECTURE.md` §12 now states the optional `headless` registry block. No `runs.json` or `session/v1` schema change, no CLI/Telegram code change, no new dependency beyond the approved `axum`.

### Proof (actual results)

```text
cargo test headless_
  -> 12 passed, 0 failed
     required: headless_wrong_token_401, headless_missing_token_401,
               headless_malformed_body_400, headless_oversized_body_413,
               headless_unknown_path_404, headless_binds_loopback_by_default,
               headless_second_bind_fails_closed
     extra:    headless_unknown_fields_400, headless_wrong_method_405,
               headless_valid_submit_returns_201_without_echoing_the_body,
               registry headless default/loopback and non-loopback confirmation tests

cargo test
  -> 271 unit passed, 0 failed
  -> 11 real CLI process tests passed, 0 failed
  -> 282 total passed, 0 failed (recorded baseline 270)

cargo test --test cli
  -> no test target named `cli`; the real CLI process suite is `tests/session_cli.rs`
cargo test --test session_cli
  -> 11 passed, 0 failed

rustfmt --edition 2024 --check src/headless.rs src/main.rs src/registry.rs src/run.rs
  -> clean

cargo clippy --all-targets
  -> 4 pre-existing warnings (channel.rs:368, locale.rs:19, memory.rs:108, memory.rs:133)
  -> none in the changed or new files

cargo build --release
  -> zero Iskra warnings; the existing transitive proc-macro-error2 v2.0.1 future-incompatibility note remains

stripped release binary
  -> 7,280,536 bytes = 6.94 MiB (baseline 6,328,272 bytes = 6.32 MiB; +0.62 MiB)
  -> axum added axum, axum-core, httpdate, and matchit to the lock; hyper/tower/http were already present

dummy-config CLI idle RSS, isolated temporary directory, after 2 s waiting on stdin
  -> 4,480 KB = 4.38 MiB (baseline 4.06 MiB)

direct dependencies
  -> 8 (limit 15; previous 7)
```

- All hard budgets pass: stripped binary 6.94 MiB ≤ 20 MB, idle RSS 4.38 MiB ≤ 30 MB, direct dependencies 8 ≤ 15.
- Local smoke run outside the test suite, with a synthetic `iskra.json`, a dummy model URL on loopback, and a temporary data directory: `headless_ready bind=127.0.0.1:18787`; `POST /v1/runs` without a token -> 401; with the owner token -> 201 `{run_id:1, state:"queued"}`; `GET /v1/runs/1` -> persisted `running` state. The dummy model endpoint was loopback port 9; no provider or Telegram request was made.
- Token and request bodies are not logged or echoed; the 401/201 tests assert the token and prompt are absent from response bodies.

### Limits and next

- G2 owns idempotency dedupe, the duplicate-submit proof, and restart-safety proof through this surface. G3 owns stored results and mechanical evidence values; G1 returns only persisted state and explicit `unknown`.
- `created_at` is returned on the 201 because admission time is known at that moment; `GET` omits timestamps because `runs.json` has no timestamp fields and this Goal may not change that schema.
- The headless surface is opt-in through `iskra.json`; the compatibility registry-less startup is unchanged. No owner-run external-client or real-provider check was performed.
- No commit or push was performed.

Next: G2 in its own Goal session — submit through Core and restart safety.

## headless/v1 G2 — Core-owned idempotent submit and restart proof (2026-11-26)

Status: **G2 complete; automated verification and a real SIGKILL process proof passed; not committed or pushed.** Baseline `master` `0d31697` with the G1 and design-session changes preserved.

### Scope and behavior

- Added `src/idempotency.rs`: a `Core`-owned `IdempotencyStore` over `memory/idempotency.json`. It is a separate side store, not a `runs.json` or `session/v1` schema change, and it is not a second execution authority. The strict `version: 1` file is atomically written with private permissions, capped at 256 keys, and loaded at startup. An unreadable, invalid, or unsupported index poisons headless submit only; a submit that needs a durable dedupe decision returns 503 instead of creating a duplicate.
- Added `Core::admit_idempotent`, which reuses the existing queue admission (`RunStore::enqueue`) and dispatch path. Under the Core admission lock it checks the key, reserves the next Run id in the dedupe index, then creates the Run. The reservation-before-Run order means a persisted Run always has its dedupe entry; a crash between the two writes leaves a key that points to no Run, and the next submit treats it as a miss and reuses the id. A duplicate key returns the persisted Run id and `deduplicated:true` without enqueueing.
- `POST /v1/runs` returns 201 for the winning admission and 200 `{schema, run_id, state, deduplicated:true}` for a duplicate. The stored state is reported for a duplicate, because the original may have advanced or been interrupted since admission. A RunStore or dedupe persist failure returns 503 `storage_unavailable` and releases the reservation; admission full stays 429. The HTTP layer still does not dispatch, execute, or write Run state.
- Restart behavior is unchanged: startup recovery performs `queued -> queued_paused` and `preparing/running -> interrupted` with zero model or tool calls, and resume remains the explicit CLI/Telegram owner action. The dedupe index is restart-safe, so a duplicate after restart returns the original Run id.
- Log privacy is unchanged in shape and now covers submit: ids, states, and a deduplicated flag only. The prompt, token, idempotency key, and request body are not logged. `MAX_IDEMPOTENCY_KEY_CHARS` now references the store's single 256-character bound.
- No CLI or Telegram behavior change, no `src/channel.rs` change, no registry or delivery-store change, no dependency change, and no real Telegram or provider call. Both mock model endpoints were loopback.

### Proof (actual results)

```text
cargo test headless_
  -> 16 passed, 0 failed (12 G1 + 4 G2)
     headless_duplicate_submit_creates_no_second_run
     headless_storage_failure_returns_503_and_creates_nothing
     headless_submit_then_kill_leaves_interrupted_with_zero_model_calls
     headless_submitted_prompt_never_logged
     (the real process test also matches this filter: headless_process_kill_never_replays_submitted_work)

cargo test restart
  -> 8 passed, 0 failed (5 unit + 3 real CLI process tests)

cargo test
  -> 275 unit passed, 0 failed
  -> 12 real CLI process tests passed, 0 failed
  -> 287 total passed, 0 failed (G1 baseline 282; +4 unit +1 process)

cargo test --test session_cli
  -> 12 passed, 0 failed, including:
     headless_process_kill_never_replays_submitted_work
       real process: POST 201 run 1 -> running; POST 201 run 2 -> queued;
       duplicate 200 run 1; SIGKILL; restart; GET run 1 = interrupted,
       GET run 2 = queued_paused, duplicate still 200 run 1; model posts stay 1.

rustfmt --edition 2024 --check src/headless.rs src/idempotency.rs src/main.rs src/run.rs src/supervisor.rs tests/session_cli.rs
  -> clean

cargo clippy --all-targets
  -> 4 pre-existing warnings only (channel.rs:368, locale.rs:19, memory.rs:108, memory.rs:133)
  -> none in the changed or new files

cargo build --release
  -> zero Iskra warnings; existing transitive proc-macro-error2 v2.0.1 note remains

stripped release binary
  -> 7,313,656 bytes = 6.97 MiB (G1 7,280,536 bytes = 6.94 MiB; +0.03 MiB)

dummy-config CLI idle RSS, isolated temporary directory, after 2 s waiting on stdin
  -> 4,448 KB = 4.34 MiB (G1 4,480 KB = 4.38 MiB)

direct dependencies
  -> 8 (limit 15; unchanged)
```

- Concurrent duplicates are covered inside `headless_duplicate_submit_creates_no_second_run`: eight simultaneous submits of one fresh key produce exactly one 201, seven duplicate 200s, one Run id, two Run records total (including the earlier sequential case), and `next_run_id = 3`.
- `headless_storage_failure_returns_503_and_creates_nothing` covers both directions: a corrupt dedupe index returns 503 before any Run write and leaves the original file untouched; a blocked `runs.json` path returns 503, creates no Run, and releases the reserved key (`entries: []`).
- `headless_submitted_prompt_never_logged` primes `logs/agent.log`, asserts the submit path wrote its own line, and asserts the unique prompt, idempotency key, token, and whole request body are absent. `headless_submit_then_kill_leaves_interrupted_with_zero_model_calls` holds a model request open, aborts in-flight execution without settlement, reloads the data directory, and asserts `interrupted`/`queued_paused` with the model-call count unchanged and zero active handles.

### Limits and next

- The dedupe index is bounded to 256 keys and a key whose Run left the bounded 100-Run store is treated as a miss; idempotency is therefore bounded, not permanent. Reusing an old key after Run-store pruning creates a new Run.
- The dedupe key is stored in the private `memory/idempotency.json`; only logs and responses are proven free of it.
- The real process test models a submit, SIGKILL, and restart on one host. It does not cover kernel-level disk failure, a full disk, or an owner-run external client.
- G3 owns stored results and mechanical evidence values; G1/G2 still return persisted state and explicit `unknown` for not-yet-computed evidence.
- No commit or push was performed.

Next: G3 in its own Goal session — persisted result and mechanical evidence without a model call or usage-counter change.

---

## headless/v1 G3 — Stored result and mechanical evidence reads (2026-11-26)

Status: **G3 complete; automated verification passed; not committed or pushed.** Baseline `master` `0d31697` with the G1, G2, and design-session changes preserved; G3's baseline was G2's recorded 287 passed / 0 failed, 6.97 MiB stripped, 4.34 MiB idle RSS, and 8 direct dependencies.

### Scope and behavior

- `GET /v1/runs/{run_id}` adds the completed Run's stored redacted Session answer as `result`, read through `Core::completed_answer` — the same projection `/result` uses — so the HTTP layer gains no new read authority and no write authority. Unfinished, failed, cancelled, interrupted, and unknown Runs skip the Session read and omit `result`; a completed Run whose Session projection is missing also omits it. `created_at` and `updated_at` are `"unknown"` because `runs.json` persists no timestamps, and `finished_at` is never invented.
- `GET /v1/runs/{run_id}/evidence` keeps the design-brief shape with mechanical values only: `exit_code` `"unknown"`, `artifact_hashes` `[]`, `delivery` `{id, state, message_id}` all `"unknown"`, and `usage` `{model_calls: "unknown", cost_known: false}`. Nothing is estimated, parsed out of logs or tool output, or taken from the model's completion claim.
- Gaps recorded in the card instead of adding persisted state: no per-Run exit code or timestamps in `runs.json`; no artifact broker or artifact hashes; no by-Run delivery read contract exposed to headless (a delivery-store change would be required); no persisted per-Run model-call count (log-derived counts are explicitly excluded as evidence).
- Authentication, bind rules, error codes, the G1/G2 submit path, CLI, and Telegram behavior are unchanged. No new dependency, no approval, no `runs.json` / `session/v1` / delivery-store schema change, no new persisted field, and no fourth endpoint. Both mock model endpoints were loopback.
- The read tests use a second registry Agent with a unique id per test. Usage counters are log-derived per Agent, so the unique id makes the counter snapshot isolated from every other test in the same process; the real Run still uses the existing Core admission and dispatch path.

### Proof (actual results)

```text
cargo test headless_read_
  -> 4 passed, 0 failed
     headless_read_returns_stored_answer_with_zero_model_calls
     headless_read_omits_result_for_unfinished_work
     headless_read_evidence_reports_unknown_without_estimating
     headless_read_does_not_change_usage_counters
  (all three design-brief labels carry the headless_read_ prefix so this filter
   matches them; the unfinished-result test is additive)

cargo test restart
  -> 8 passed, 0 failed (5 unit + 3 real CLI process tests)

cargo test
  -> 279 unit passed, 0 failed
  -> 12 real CLI process tests passed, 0 failed
  -> 291 total passed, 0 failed (G2 baseline 287; +4 unit)

cargo test --test session_cli
  -> 12 passed, 0 failed

rustfmt --edition 2024 --check src/headless.rs src/idempotency.rs src/main.rs src/run.rs src/supervisor.rs tests/session_cli.rs
  -> clean

cargo clippy --all-targets
  -> 4 pre-existing warnings only (channel.rs:368, locale.rs:19, memory.rs:108, memory.rs:133)
  -> none in the changed or new files

cargo build --release
  -> zero Iskra warnings; existing transitive proc-macro-error2 v2.0.1 note remains

stripped release binary
  -> 7,313,632 bytes = 6.97 MiB (G2 7,313,656 bytes = 6.97 MiB)

dummy-config CLI idle RSS, isolated temporary directory, after 2 s waiting on stdin
  -> 4,432 KB = 4.33 MiB (G2 4,448 KB = 4.34 MiB)

direct dependencies
  -> 8 (limit 15; unchanged)
```

- Every read test asserts the mock model POST count, the per-Agent `usage::today_for_agent` counters, `runs.json` bytes, the Agent's `session.json` bytes, and the absence of `memory/deliveries.json` are unchanged across the read. The usage test executes a real completed Run first, asserts `usage.total > 0`, then performs two status reads and two evidence reads.
- `headless_read_returns_stored_answer_with_zero_model_calls` submits through `POST /v1/runs`, waits for `completed`, and asserts the read returns the stored answer with `created_at`/`updated_at` `"unknown"` and without echoing the prompt.
- `headless_read_evidence_reports_unknown_without_estimating` keeps a model request open long enough for a completed Run, asserts every evidence value is explicitly `"unknown"`/`[]` with string types (never a numeric estimate), and asserts the stored answer and prompt do not appear in the evidence body.
- `headless_read_omits_result_for_unfinished_work` holds a model request open, reads a `running` Run, and asserts `result` is absent; the model-call count is snapshotted after the hanging request is observed so the assertion is not racy.

### Limits and next

- Evidence is intentionally thin: exit code, artifacts, delivery identity, and model-call counts all stay `"unknown"` until a future card adds persisted mechanical records through a separate approved schema change. G3 proves the read contract, not evidence richness.
- The read contract reports `created_at`/`updated_at` as `"unknown"`; an external client that needs timestamps requires a future persisted-field card.
- Tests cover loopback mock endpoints on one host; the owner-run external-client and real-provider checks remain.
- No commit or push was performed.

Next: owner-run external-client and real-provider checks outside the Goal loop; then a separate commit/push decision.

---

## headless/v1 real-provider smoke — owner-approved external client (2026-11-26)

Status: **passed; no code change; repository clean at `d53b77f`.**

### Setup

- Isolated data directory `~/iskra-smoke` outside the repository, so the real `memory/`, `logs/`, and `workspace/` were untouched (repository `git status` empty before and after).
- Strict registry: one engine (`https://api.deepseek.com/chat/completions`, `deepseek-flash`, `api_key_env=DEEPSEEK_API_KEY`), one `main` agent, `headless.bind=127.0.0.1:8787`, `token_env=ISKRA_TOKEN`, no `allow_non_loopback`.
- Credential handling: the owner's project `.env` was exposed to the process only through a temporary symlink that Iskra's own `dotenvy` loaded at startup. The key was never read, printed, copied, or stored by the agent or the smoke script; the symlink was removed after the run. The bearer token was a throwaway 48-hex-character value in a `0600` file inside the throwaway directory.
- Client: `curl` from outside the process (the script), against the release binary.

### Results (actual)

```text
provider: https://api.deepseek.com/chat/completions deepseek-flash
POST /v1/runs                      -> 201 {"run_id":1,"state":"queued"}
GET  /v1/runs/1                    -> 200 state=completed result="ready"
                                     created_at=updated_at="unknown"
POST /v1/runs (same key)           -> 200 {"deduplicated":true,"run_id":1,"state":"completed"}
                                     next_run_id=2 (no second Run)
no token / wrong token             -> 401 / 401
unknown run / non-numeric run      -> 404 / 404
GET  /v1/runs/1/evidence           -> 200 exit_code="unknown" artifact_hashes=[]
                                     delivery={id,state,message_id}="unknown"
                                     usage={model_calls:"unknown",cost_known:false}
runs.json sha256                   -> dfdb7d15a55a860c641ec753b50880f19579d87836897600cef9c4e2c61e62fa
                                     unchanged across duplicate submit, all reads, and restart
model_call log lines               -> 1 -> 1 across duplicate submit, all reads, and restart
restart                            -> 200 state=completed same result, zero replay
logs                               -> token occurrences 0, submitted prompt occurrences 0
```

- One real model call total: `model_call agent=main run_id=1 engine=main-engine model=deepseek-flash tools=[none] usage=prompt=1870 completion=2 total=1872`. The stored answer (`session.json` work 1, answer `"ready"`) is exactly what `result` returned, so the read path projected stored state rather than regenerating it.
- The evidence endpoint stayed mechanical: the log-derived usage line was deliberately not used as evidence, and `model_calls` remained `"unknown"`.
- This closes the external-client and real-provider gap recorded at the end of G3. Still unverified by design: non-loopback binds, Telegram delivery, and any evidence field that would require persisted state.

### Next

`headless/v1` is complete and frozen (owner decision 2026-11-26). Persisted mechanical evidence is deferred until a concrete external-client blocker appears, and would require a separately approved schema-change card. Next: owner chooses the next Iskra track; watchpost stays on hold.

## Paperclip competitor registry entry (2026-11-26)

Status: **docs only; no code change; no install, no code audit, no bake-off.**

### What changed

- `docs/research/iskra-survival-strategy.md` §11.5: Paperclip registered in the §11 format (public README/docs facts, strategic meaning, explicit boundary, seven unmeasured comparison dimensions).
- `docs/BUSINESS.md`: Paperclip added to the "Headless and orchestration references" list with two source URLs.

### Facts recorded (README/docs only)

- Node.js 24.11+, pnpm, embedded PostgreSQL, React UI, Docker, MIT; orchestrates Claude Code, Codex, OpenClaw, Cursor, Gemini CLI, OpenCode via heartbeats; org chart, atomic task checkout, approval gates, per-agent budgets, secrets manager, MCP tool gateway, routines, multi-org, out-of-process plugins.
- Anonymous telemetry is enabled by default (disable via `PAPERCLIP_TELEMETRY_DISABLED=1`, `DO_NOT_TRACK=1`, `CI=true`, or config). Sentry/OTel are opt-in.
- Budget enforcement uses recorded spend and in-flight work can delay a stop (self-declared). Self-healing/automatic recovery is listed as shipped.
- Star/fork counts (~117.3k / 19.5k) recorded as distribution signal only.

### Boundaries kept

- No conclusion that Paperclip is safe or unsafe. The "security posture verified by Superagent" badge is a third-party claim and was not independently checked.
- No claim of Iskra superiority on install, recovery, or external-effect safety before a same-scenario bake-off.
- Paperclip is used as further evidence that "no orchestrator exists" is a false market assumption, not as a replacement target.

### Next

Keep the seven comparison dimensions unmeasured until an owner-approved bake-off. No commit or push.

## Audit offer launch — scope B+ documentation pass (2026-11-26)

Status: **documentation and design only; page checks passed; not committed or pushed; no Core code, dependency, or schema change.**

### What changed

- `docs/index.html`: `rmP` and `prog2Meta` updated in EN/KO/RU and the static fallback; `registry.rs`, `run.rs`, `headless.rs`, `idempotency.rs` inserted after `loop_.rs` in `codeTree` (EN/KO/RU + static fallback) with the existing `<span class="cm">` comment formatting; one `axum` pill added to the BUILT WITH strip. No dedicated headless section.
- `docs/audit/index.html`: rewritten as one static file with inline CSS matching the homepage tokens (Manrope/Noto Sans + JetBrains Mono, ember palette, 18px radius, shared shadow, 1160px wrap). No scripts, frameworks, analytics, or telemetry. Four deliverables became a numbered strip with unchanged claims. Added the four-step synthetic preview, the `Trust & boundaries` block (Synthetic sample, No install. No call., Data handling, What is not included), and the owner-gate comment. Q&A and fine print preserved.
- The visible banner is now `DRAFT — DO NOT PUBLISH YET. Complete the intake form, payment path, and 4-hour rehearsal before launch.`; `noindex,nofollow` is kept; only the two intake CTA hrefs contain `{{INTAKE_URL}}`.
- Records: `SESSION.md` card, this entry, personal-memory `DECISIONS.md` (2026-11-26 scope B+), personal-memory `NOW.md` priority 1.

### Commands and actual results

- `rg -n '\{\{[^}]+\}\}' docs/audit/index.html` → 2 matches, both `{{INTAKE_URL}}` (lines 145, 265).
- `node docs/audit/replay-delivery/check.mjs` → `"state": "PASS"`, `"passed": 8`, `"failed": 0`.
- Headless Chrome via CDP, `Emulation.setDeviceMetricsOverride`, `document.fonts.ready` awaited, `prefers-color-scheme` light and dark:
  - 360×640: `scrollWidth` 360, no horizontal overflow; `DRAFT` 71–140, h1 200–256, claim 265–326, primary CTA 338–386, sample link 394–442, terms block 454–607, `$49` 465–492, `HYPOTHESIS` 475–494, `48 hours` 555–577 — all inside the 640px first viewport.
  - 360×780: identical layout, all required items visible.
  - 768×900 and 1280×900: no horizontal overflow; all required hero items visible; section content aligned to the 1160px wrap.
- `python3 -m http.server 8123 --directory docs`: `/audit/`, `/audit/sample.html`, and `/audit/replay-delivery/README.md` all returned HTTP 200.
- Original-claim check: 45 sentences from `git show HEAD:docs/audit/index.html` compared against the new page; the only unmatched strings are the replaced DRAFT banner, its removed placeholder instruction, and text split across new markup. Targeted string checks for the five out-of-scope items, Data handling, all three Q&A answers, the h1, the supporting paragraph, and the two Trust items: 14/14 present; synthetic-preview strings 9/9 present.
- HTML tag-balance parse of `docs/audit/index.html`: no unclosed tags, no mismatches.
- `git status --short` → only `SESSION.md`, `docs/audit/index.html`, `docs/index.html` modified. `git diff --check` → clean.

### Unverified

- No real-device or Safari/iOS rendering check; only headless Chrome emulation.
- No `cargo test` rerun in this session because no code changed; the 291 passed / 0 failed figure remains the `b8de139` baseline.
- Intake URL, payment path, and the four-hour sample-redelivery rehearsal are still incomplete, so launch stays blocked.

### Next action

Owner reviews the documentation diff and separately approves or rejects commit/push. External launch remains blocked until the intake form, payment path, and four-hour sample-redelivery rehearsal are complete. No Core code action is approved, the dedicated headless homepage section remains deferred, and watchpost stays on hold.

## Roadmap correction — demand-gated tracks (2026-11-26)

Status: **documentation only; no code, dependency, or schema change; committed and pushed after verification.**

### What changed

- README en/ko/ru: the status badge, status block, current-state heading/checklist, and roadmap section were rewritten. `headless/v1` is now listed as complete and frozen instead of unimplemented, and the v3–v5 version ladder (triggers/bot packages/`identity/v1`, Tauri desktop, per-user memory) was removed because no current source of truth schedules those items.
- The roadmap is now v1/v2 complete plus five demand-gated tracks mapped to `docs/BUSINESS.md` §6 and `docs/ARCHITECTURE.md` §12: Agent Replay & Delivery Audit, Outcome/Evidence + one Outcome Pack, ACP and bounded delegation, Managed always-on, Team/self-serve multi-tenant.
- Three stale "planned for v2" design-principle sentences (specs, approval, quarantine) were corrected to "as each part lands / not implemented yet / later target" so the text stays truthful after v2 is marked complete.

### Verification

- `rg -n "v3|v4|v5|in progress|planned for v2|в работе|진행 중" README.md README.ko.md README.ru.md` → 0 matches.
- `git diff --check` → clean.
- Claim mapping: the `headless/v1` row matches the G1–G3 records in `SESSION.md` and the 2026-11-26 owner freeze; track gates match `docs/BUSINESS.md` §6 Phase B–F and `docs/ARCHITECTURE.md` §12 Stage 3–4.
- No `cargo test` rerun: no Rust or behavior change.

### Next action

The next Iskra action stays owner-gated: audit launch scope B+ needs the intake form, payment path, and four-hour rehearsal before external publication. No Core code action is approved.

## Terminal ownership analysis — Herdr (2026-11-27)

Status: **research and design only; no code, dependency, schema, or test change.**

### What changed

- Added `docs/research/herdr-analysis-2026-11-27.md`: observation versions, terminal-ownership contract, L0–L3 surface ladder, VT comparison, gate-form spike, budget, failure catalog, and falsification conditions.
- Updated `docs/ARCHITECTURE.md` invariant 4 (untrusted code never gains authority; pure functions or separately supervised processes) and invariant 9 (base 30 MB RSS plus 2 MB per pane, 1 MB scrollback, up to 8 panes), per the owner's 2026-11-27 approval. Added a status sentence that terminal ownership is an approved direction but not implemented.
- Recorded two owner decisions in personal-memory `DECISIONS.md` and added the handoff card to `SESSION.md`.

### Verification

- Research claims are tagged with source paths or `[확인]`/`[문서 주장]`/`[미확인]` equivalents in the research doc; no Herdr source was copied.
- Herdr repositories were cloned to a temporary directory only; the Iskra working tree contains no Herdr code.
- No `cargo test` rerun: no Rust or behavior change. Baseline remains 291 passed / 0 failed at `60aeb33`.
- `git diff --check` and a stray-token scan on the new doc were run after editing.

### Next action

VT spike in gate form (`PaneGrid` seam plus vt100 adapter, G0–G8). Owner must approve the `vt100` dependency before implementation; the audit scope B+ launch stays owner-gated.

## VT spike — G0–G8 (2026-11-27)

Status: **spike complete; 6/6 scored gates PASS; verdict PROCEED; harness only.** No file in the `iskra` package changed: `git status` lists only the new untracked `spikes/` tree, and `git diff Cargo.toml Cargo.lock` is empty.

### What was built

- `spikes/vt` is an isolated crate (not a workspace member). It contains `src/pty.rs` (PTY spawn via `openpty` + `setsid` + `TIOCSCTTY`, resize, raw input, non-blocking read, bounded teardown), `src/grid.rs` (`PaneGrid` trait, vt100 adapter with a `Callbacks` tap, feature-off `NullGrid` baseline), `src/gates.rs` (G0–G8, panic-isolated, evidence strings), `src/main.rs`, `measure.sh`, and `evidence.json`.
- Candidate: `vt100` 0.18.1 (`vte` 0.15.0, `unicode-width` 0.2.3, `arrayvec` 0.7.8, `memchr` 2.8.3, `itoa` 1.0.19). macOS 27.0.1 (26A434), rustc 1.100.0, edition 2024.
- The production dependency list is unchanged at 8. Landing `vt100` in `iskra` remains a separate owner approval.

### Commands and actual results

- `cd spikes/vt && ./target/release/vt-spike` → G0–G8 all `PASS`; `SCORE scored_pass=6/6`; exit 0.
- Same command three consecutive times → `6/6`, 0 failures each run; after the runs, `ps -ax | grep 'bash --norc'` → 0 orphaned panes.
- `./measure.sh` → stripped with vt100 `610688` B, baseline `519776` B, delta `90912` B (`88` KiB); RSS base `1872–1888` KB, 1 pane `2128` KB, 4 panes `2416–2448` KB; fds `7 / 8 / 11`. The script merges these into `evidence.json` and re-runs the suite.
- A 4-pane resource probe (`--probe 4 1`) completed in 1.09 s under a 30 s alarm in 3/3 runs.

### Gate evidence (condensed)

| Gate | Result | Evidence |
|---|---|---|
| G0 | PASS | OSC 7501 (`working;run=7`) and 9999 captured via `unhandled_osc`; title and clipboard callbacks fired; grid text `ABCDEF` |
| G1 | PASS | 24×80 bash prompt plus command output and a new prompt; shell alive |
| G2 | PASS | vim alternate screen, typed text visible, `:wq` saved `hello from vt spike`, normal screen restored |
| G3 | PASS | `PID  COMMAND  %CPU …` header and 2 distinct frames |
| G4 | PASS | 5 Hangul base cells + 5 wide continuations, cursor column 10; `a한b` cursor column 4 |
| G5 | PASS | grid 12×40, no over-width rows, PTY 24×80 → 30×100, zero split wide cells after redraw |
| G6 | PASS | capacity 200, 200 rows retained, 224 contiguous lines ending at `line-300` |
| G7 | PASS | CR submit, Up-arrow history, UTF-8 input, Ctrl-C interrupt |
| G8 | PASS | split UTF-8, wide char at last column wraps, invalid byte ignored, mode bits set/reset |

### Findings

- vt100 exposes unknown OSC through `Callbacks::unhandled_osc`; no side-channel pre-parser is needed for OSC 7501-style status.
- Scrollback is a viewport offset: the host pages one screenful at a time. 200 rows read in 10 pages (224 lines incl. the screen).
- No reflow on width change; vt100 truncates and the app redraws. The gate passed because no split wide cell remained after redraw, but scrollback reflow is not available.
- Scrollback budget: 32-byte cell → 2,560 B/row at 80 columns (409 rows/MiB), 6,400 B/row at 200 columns (163 rows/MiB). "1 MB scrollback" must be defined in bytes if 200 columns are allowed.
- The first 2+ pane teardown runs hung in an unbounded `Child::wait` while orphaned panes from a killed run existed. After removing the orphans, every teardown variant (drain on/off, `FD_CLOEXEC` on/off, blocking/bounded reap) passed 3/3. The harness keeps drain, `FD_CLOEXEC` on the master, and a bounded reap. Root cause from a clean state is unproven; it is recorded as a mandatory asynchronous-teardown requirement, not a solved fact.

### Unverified

- `alacritty_terminal` was not built; the single-candidate run does not compare backends.
- Reflow, 8-pane scale, two-client reconnect, non-macOS platforms, Herdr authority reproduction.
- Any Iskra integration; the production stripped-size delta is unmeasured.
- Owner visual inspection of vim/top/Korean rendering (the checks here are automated PTY assertions).

### Next action

The gate says proceed. The owner picks one: (A) approve `vt100` behind a `PaneGrid` seam in `src/` as a new card, direct dependencies 8 → 9; (B) run the same harness against `alacritty_terminal` first; (C) keep the seam design only. No commit, push, or production change was made.

## Pane seam landing (A) — PaneGrid + vt100 (2026-11-27)

Status: **complete; automated verification passed; committed as `7705af2` and pushed.**

### What changed

- `Cargo.toml`: `vt100 = "0.18.1"` is now a direct dependency (8 → 9). `Cargo.lock` adds `vt100` 0.18.1, `vte` 0.15.0, `unicode-width` 0.2.3, and `arrayvec` 0.7.8; `itoa` and `memchr` were already in the lock.
- New `src/pane_grid.rs`: production `PaneGrid` seam and `Vt100Grid` adapter. It holds split UTF-8 across `process()` calls, resizes, and exposes a snapshot (rows, cols, cursor, `hide_cursor`, `alternate_screen`, `bracketed_paste`, `application_cursor`, `application_keypad`, typed `MouseMode`, typed `CursorShape`, scrollback length/capacity, trimmed visible lines), cell/wide inspection, an OSC/CSI callback tap, and paged scrollback in global order.
- `src/main.rs`: `mod pane_grid;` registration only.
- The module is intentionally unreferenced until the pane-host card. A single module-level `#![allow(dead_code)]` with a comment covers it; no debug flag, command, or production call was added.
- No PTY, process spawn, pane runtime, queue integration, channel/locale change, or `runs.json`/`session/v1` change. CLI and Telegram paths are untouched.

### Commands and actual results

- `cargo test pane_grid` → **9 passed, 0 failed** (279 filtered out).
- `cargo test` → **288 unit passed, 0 failed**; `tests/session_cli.rs` → **12 passed, 0 failed**. Total **300 passed, 0 failed** (recorded baseline 291).
- `cargo fmt --all -- --check` → exit 0. `rustfmt --edition 2024 --check src/pane_grid.rs src/main.rs` → exit 0.
- `cargo clippy --all-targets` → the same 4 pre-existing warnings: `channel.rs:368` (collapsible if), `locale.rs:19` (manual char comparison), `memory.rs:108` (explicit counter), `memory.rs:133` (consecutive replace). No warning in `pane_grid.rs`.
- `cargo build --release` → success with zero Iskra warnings; only the existing transitive `proc-macro-error2 v2.0.1` future-incompatibility note.

### New seam coverage

- `unknown_osc_is_captured_without_grid_text`: OSC 7501 and 9999 reach `unhandled_osc`, window-title and clipboard callbacks fire, unknown CSI is tapped, and visible text is only `ABCDEF`.
- `icon_name_and_bells_are_reported`; `mode_bits_round_trip` (alternate screen, bracketed paste, application cursor/keypad, hidden cursor, press-release mouse, steady underline → all reset).
- `split_utf8_across_process_calls_is_held`; `wide_character_at_last_column_wraps`; `invalid_byte_does_not_corrupt_the_grid`.
- `snapshot_reports_rows_cursor_and_scrollback_capacity`; `resize_updates_the_reported_size`.
- `scrollback_pages_in_global_order`: 224 fed lines, 200-row capacity, 224 returned lines, contiguous `L001…L223` oldest-first.

### Budgets

- Stripped: `strip -x -o target/tmp/iskra-stripped target/release/iskra && stat -f%z` → **7,686,608 B = 7.33 MiB** (limit 20 MiB). Delta over the 7,313,632 B G3 baseline: **+372,976 B = 364.2 KiB**.
- The card expected a near-zero unused-module delta; the measured link delta is 364 KiB. This is the cost of linking `vt100` even without a production call, not the spike's 88 KiB used-code delta. The authoritative used-code production delta still needs the pane-host card.
- Dummy-config CLI idle RSS in an isolated temporary directory, after 3 s waiting on stdin: **4,416 KB = 4.31 MiB** (limit 30 MiB). Dummy `LLM_*` values, no real `.env`, model call, or Telegram request.
- `cargo metadata --no-deps --format-version 1` → **9 direct dependencies** (limit 15): axum, dotenvy, libc, reqwest, serde, serde_json, teloxide, tokio, vt100.

### Unverified

- PTY lifecycle, pane state transitions, real pane rendering, scrollback under an actual application, and the production used-code size delta.
- No owner-run Telegram/provider check was performed; none belongs to this card.
- Committed as `7705af2` and pushed to `origin/master` on 2026-11-27.

## Pane lifecycle card prepared (2026-11-27)

Status: **card prepared; no code, dependency, schema, test, or runtime change.**

- Added `Pane lifecycle and state (B) — PTY host` to `SESSION.md`: outcome, state/transition table, `memory/panes.json` schema and bounds, PTY/environment/teardown contract, Core ownership, focused and milestone proof, stop conditions, and the Goal-mode contract.
- The card keeps the pane surface (layout, rendering, commands, model/tool input) out of scope. It requires 9 direct dependencies (no new crate), at most 8 panes, a 2 s bounded teardown, and a 2 MiB modeled per-pane budget.
- Baseline recorded in the card: `master` @ `6ce9783`, 300 passed / 0 failed (288 unit + 12 real CLI), stripped 7,686,608 B, dummy-config CLI idle RSS 4,416 KB, 9 direct dependencies.
- No test or build was run for this preparation; the recorded baseline is from the Pane seam result. No Telegram/provider or pane visual check was performed.
- Next: a new Goal session executes the card. No owner input is required to finish it. No commit or push.

## Pane lifecycle and state (B) — PTY host (2026-11-27)

Status: **complete; automated verification passed; not committed or pushed.**

- Changed: new `src/pty.rs` (PTY ownership) and `src/pane_host.rs` (slot store + host); `src/pane_grid.rs` (`PaneGrid: Send`, `scrollback_rows`, 32 B/cell and 1/2 MiB constants); `src/run.rs` (Core owns `PaneHost`, opens it under the dispatcher lock, exposes the accessor); `src/main.rs` (module registration only); `SESSION.md`/`SESSION-LOG.md`.
- `memory/panes.json` is a strict version-1 projection: `next_pane_id`, at most 8 records sorted by id, `spawning|live|interrupted|finished`, dimensions `1..=200`, `exit_code` only on `finished`. A missing file is an empty host and creates nothing. Invalid UTF-8/JSON, unknown version or field, duplicate/out-of-order ids, ids `>= next_pane_id`, out-of-range dimensions, a non-finished exit code, and more than 8 records preserve the original bytes and poison pane operations. Writes reuse `memory::atomic_write` with mode `0600`.
- Recovery maps `spawning|live` to `interrupted` in one atomic write at `PaneHost::open`, with zero spawns. `start` persists `spawning` before `openpty`; a failed initial write spawns nothing. A spawn failure restores the previous slot or removes a new record. `live` is persisted before `start` returns; a failed `live` write stops the child. Later write failures poison further starts; a failed resize write stops the affected pane. `spawning|live` never return as a clean transition.
- PTY contract: `/bin/bash --norc --noprofile`, absolute argv, canonical `main` workspace cwd, allowlist environment (`TERM=xterm-256color`, `LANG`/`LC_ALL=en_US.UTF-8`, fixed `PATH`, inherited `HOME` only), `openpty` + `setsid` + `TIOCSCTTY`, non-blocking master with `FD_CLOEXEC`, `TIOCSWINSZ`, bounded writes, non-blocking reads, and a 2 s drain/close/kill/reap teardown with no unbounded wait or `mem::forget`.
- Pane model: `scrollback_rows(24,80)=409`, `scrollback_rows(200,200)=127`; screen+scrollback stays within 2 MiB; at most 8 live panes. Logs are metadata only (`pane_spawn`, `pane_exit`, `pane_interrupted`, `pane_store_error`, `pane_stop_timeout`); no PTY bytes are logged.
- Focused results: `cargo test pane_host::` **20 passed, 0 failed**; `cargo test pty::tests` **6 passed, 0 failed**; `cargo test pane_grid::` **10 passed, 0 failed**; `cargo test run::` **28 passed, 0 failed**.
- Full: `cargo test` **316 unit + 12 real CLI = 328 passed, 0 failed** (baseline 300 = 288 unit + 12 CLI; +28 unit: 20 pane_host, 6 pty, 1 pane_grid, 1 run).
- `cargo fmt --all -- --check` exit 0; `rustfmt --edition 2024 --check src/pty.rs src/pane_host.rs src/pane_grid.rs src/run.rs src/main.rs` clean.
- `cargo clippy --all-targets`: the same 4 pre-existing warnings (`channel.rs:368`, `locale.rs:19`, `memory.rs:108`, `memory.rs:133`); none in the new code.
- `cargo build --release`: zero Iskra warnings; only the existing transitive `proc-macro-error2 v2.0.1` future-incompatibility note.
- Budgets: stripped **7,719,952 B = 7.36 MiB** (delta **+33,344 B = 32.6 KiB** over 7,686,608 B); dummy-config `--cli` idle RSS **4,448 KB = 4.34 MiB**; direct dependencies **9** (limit 15). All hard limits pass.
- `git diff --check` exit 0. An isolated dummy-config `--cli` start created `memory/` and `workspace/` but no `panes.json` and no `runs.json`; startup spawns no PTY.
- Test note: macOS accepted a 64 MiB non-blocking PTY write without back-pressure, so the bounded-write proof uses a deterministic zero-deadline seam; the 150 ms large-write path is asserted only as time-bounded. `resize(400, 200)` is refused by the 2 MiB model cap with the recorded size unchanged.
- Unverified: no pane surface (layout, rendering, commands), no model/tool input from panes, no real vim/top/Korean visual check, no Telegram/provider check, no per-Agent panes, and PTY teardown under pre-existing orphaned processes. No commit or push was performed.
- Committed as `3a924ba` and pushed to `origin/master` on 2026-11-27.

## Pane surface (C) card prepared (2026-11-27)

Status: **preparation only; no code, dependency, schema, or test change.**

- Added a decision gate and Option A draft card to `SESSION.md` for the surface left out of B. The decision is the v1 surface shape: A = CLI-only control commands (`/panes`, `/pane start|stop|resize|send|text`), B = local TUI attach, C = the full L0–L3 ladder. A is recommended because it is the smallest real surface, adds no dependency, and keeps Telegram away from a local shell authority boundary.
- The draft card fixes the command set, allowed files, failure lines, real-process proof (start, send/text, `stty size` resize, stop, final-screen read, 9th-start refusal, zero model requests, `0600`), stop conditions, and the Goal contract. Owner must confirm A/B/C, Telegram exclusion, and `/pane send` before a Goal is created.
- Updated `docs/ARCHITECTURE.md`'s status sentence: PTY and pane-slot ownership are implemented; no pane surface, command, layout, or TUI exists yet, and the L0–L3 ladder remains a direction.
- Recorded current state: `master` @ `3a924ba`, 328 passed / 0 failed (316 unit + 12 real CLI), stripped 7.36 MiB, dummy-config CLI idle RSS 4.34 MiB, 9 direct dependencies.
- No test or build was run for this documentation preparation. No Telegram, provider, TUI, or visual verification was performed. No commit or push.
- Owner approved **Option A** on 2026-11-27, with the recommended defaults: Telegram pane commands stay out of v1, and `/pane send` remains local CLI-only (raw bytes plus `\r`, no shell interpretation).
- Next: a new Goal session implements the approved Option A draft card. No B/C design work is opened. No commit or push without separate approval.

## Pane surface (C) Option A — CLI pane control (2026-11-27)

Status: **complete; automated verification passed; not committed or pushed.**

- Changed: new `src/pane_cli.rs` (parse/render/execute, no PTY code); `src/main.rs` (`mod pane_cli;`, `/panes` and `/pane` dispatch, CLI help); new `tests/pane_cli.rs` (three real-process tests); `SESSION.md`/`SESSION-LOG.md`. `src/channel.rs`, `src/locale.rs`, `src/run.rs`, `Cargo.toml`, and every store schema are unchanged.
- `--cli` commands: `/panes` (read-only slot list, `No panes.` on an empty host); `/pane start [rows cols]` (default 24×80, `1..=200`); `/pane stop <id>` (`finished`, or `interrupted` on teardown timeout); `/pane resize <id> <rows> <cols>` (prints the persisted size, refusal leaves it unchanged); `/pane send <id> <text>` (only a `live` slot; raw bytes plus `\r`, never interpreted as a shell string); `/pane text <id> [scrollback rows]` (pumps once, prints the visible screen, optionally up to the host scrollback capacity).
- Every structural error, unknown command/id, poisoned store, capacity refusal, and non-live send prints one English line. No pane command creates a Run, calls a model or tool, or touches Telegram. No pane starts at `--cli` startup and no background polling was added. Startup still creates no `panes.json`.
- Focused: `cargo test pane_cli` → **8 unit + 3 real-process = 11 passed, 0 failed**.
  - Unit: `parse_start_defaults_and_explicit_size`, `parse_rejects_out_of_range_and_zero_dimensions`, `parse_stop_resize_and_ids`, `parse_send_keeps_the_raw_remainder`, `parse_text_accepts_an_optional_scrollback`, `parse_rejects_malformed_and_unknown_commands_with_one_line`, `empty_list_is_one_notice_and_slots_render_state_and_exit`, `state_names_cover_every_pane_state`.
  - Real process: `pane_cli_rejects_malformed_input_without_spawning_or_calling_a_model`; `pane_cli_reports_a_poisoned_store_without_spawning`; `pane_cli_controls_a_live_pane_end_to_end`.
  - The end-to-end test proves `Pane 1 live 24x80`, `/panes` output, `SURFACE_OK` after `/pane send 1 printf 'SURFACE_OK\n'` + `/pane text 1`, `30 100` from `stty size` after resize, `Pane 1 finished` after stop, post-stop text/send unavailable, the 9th-start refusal (`Pane limit reached (8)`), zero connections to the counting model server, no `runs.json`, and `panes.json` mode `0600`.
- Full: `cargo test` → **324 unit + 3 pane CLI + 12 real CLI = 339 passed, 0 failed** (baseline 328 = 316 unit + 12 real CLI; +8 unit, +3 real process).
- `cargo fmt --all -- --check` exit 0; `rustfmt --edition 2024 --check src/pane_cli.rs src/main.rs tests/pane_cli.rs` clean. `cargo clippy --all-targets` keeps the same 4 pre-existing warnings (`channel.rs:368`, `locale.rs:19`, `memory.rs:108`, `memory.rs:133`); none in new code. `cargo build --release` has zero Iskra warnings; only the existing transitive `proc-macro-error2 v2.0.1` note remains.
- Budgets: stripped **7,827,192 B = 7.46 MiB** (delta **+107,240 B = 104.7 KiB** over 7,719,952 B; the pane host, PTY, and grid became reachable from production code, which B deferred); dummy-config `--cli` idle RSS **4,496 KB = 4.39 MiB**; direct dependencies **9** (limit 15). All hard limits pass.
- `git diff --check` exit 0. Only `src/main.rs` is modified; `src/pane_cli.rs` and `tests/pane_cli.rs` are new; no file outside the card's allowed list changed.
- Unverified: the `/pane text <id> [rows]` scrollback path is parser-covered but not exercised against a live pane in the process test; text after an observed exit is host-tested but not CLI-tested; no real vim/top/Korean visual check, no TUI/layout, no Telegram pane command, and no provider/Telegram live check. No commit or push was performed.
- Committed as `345cbb7` and pushed to `origin/master` on 2026-11-27 after this entry was written.

## Pane surface (C) Option A — manual verification (2026-11-27)

Status: **passed; documentation only; no code, dependency, schema, or test change.**

- Correction: the previous entry said "not committed or pushed". The work was committed as `345cbb7` and pushed to `origin/master` on 2026-11-27; `SESSION.md` and the memory `NOW.md` are corrected to match.
- Run: `target/release/iskra --cli` in an isolated temporary directory with dummy LLM configuration (`LLM_API_KEY=dummy`, `LLM_BASE_URL=http://127.0.0.1:1`, `LLM_MODEL=dummy`), commands fed over stdin with sleeps for PTY rendering. No real credential, provider, Telegram, or personal memory was touched.
- Start and list: `/pane start 30 100` → `Pane 1 live 30x100`; `/pane text 1` showed a real bash prompt.
- Korean: `printf '한글한글|ABC\n'` echoed intact. Two-cell alignment proven with `printf '한글한글\033[10GX\n'`, which rendered `한글한글 X` — the cursor reached column 10 after four Hangul characters.
- vim: version 9.1.1752 opened and drew its start screen. A literal ESC byte plus `:q!` through `/pane send` returned to the shell prompt.
- top: rendered a live process list; `q` returned to the shell.
- Resize: after `/pane resize 1 40 120`, `stty size` reported `40 120` inside the pane. `/pane resize 1 400 200` printed one refusal line and the size stayed `40 120`.
- Scrollback: `/pane text 1 30` returned 69 lines — 30 scrollback rows plus the 40 visible rows, the documented `n + rows` contract — including the older `seq 1 100` output.
- Observed exit and stop: after `exit`, the screen remained readable; the slot reported `1 finished 24x80 exit=0` one pump later, not on the first read. `/pane stop 1` printed `finished`, `/pane text 1` reported unavailable, a send reported not live, and an unknown id printed one error line.
- Store and model: `memory/panes.json` was mode `0600`; the log had zero `model_call` lines; `panes.json` contained `next_pane_id: 2` and one finished record.
- Findings for the TUI card: (1) an exited child appears `live` until a later pump reaps it, so a CLI user can see stale state for one command; a continuous TUI pump would not. (2) Alternate-screen programs restore the pre-run screen on exit, so shell-level checks must wait for the prompt before asserting. (3) `/pane send` has no escape syntax, so a real surface needs raw keyboard input for control keys; the interactive Ctrl-V ESC path was not exercised.
- Not verified: interactive keyboard Ctrl-V ESC, split UTF-8 across real partial reads, 8 simultaneous panes on screen, non-macOS behavior, TUI/layout, and Telegram pane control.
