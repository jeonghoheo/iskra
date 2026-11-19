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
