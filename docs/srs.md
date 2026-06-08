# tuimux SRS (Software Requirements Specification)

- **문서 버전**: 0.1 (Draft)
- **작성일**: 2026-06-08
- **상태**: 구현 대상 명세 (구현 코드 미포함, 요구사항/설계 명세)
- **프로젝트명**: tuimux
- **상위 문서**: [docs/prd.md](./prd.md) (PRD v0.1)
- **한 줄 요약**: tmux가 설치된 환경에서, tmux 서버를 백엔드로 두고 그 control mode(`tmux -CC`)에 붙어 동작하는 Rust 기반 풀-TUI 래퍼. prefix 키 없이 VS Code 같은 레이아웃과 마우스로 tmux를 다룬다.

---

## 1. 서론 (Introduction)

### 1.1 목적 (Purpose)
이 문서는 tuimux의 소프트웨어 요구사항을 구현 가능한 수준으로 정의한다. 기능 요구사항(ID 부여), 비기능 요구사항, UI 레이아웃, 인터랙션 명세, tmux control mode 통합, 상태 모델, 데이터 모델, 이벤트 흐름, 마우스 지원, 오류 처리, 인수 기준, MVP 경계, 그리고 PRD 및 사용자 요구사항으로의 추적성을 포함한다. 본 문서는 구현 코드를 담지 않는다.

### 1.2 범위 (Scope) — 본 SRS의 아키텍처 고정
PRD §8은 두 아키텍처 옵션을 비교하며 "control mode로 UX 증명 → 네이티브로 본구현"의 2단계를 권고했다. **본 SRS는 PRD의 옵션 2(2a, tmux control mode 래퍼)를 명시적으로 채택한다.**

- 멀티플렉싱·PTY 관리·VT 에뮬레이션·detach/attach 지속성은 **tmux 서버**가 담당한다.
- tuimux는 그 위에 얹는 **UI/제어 레이어**로서, `tmux -CC`(control mode) 채널로 tmux 서버에 붙어 이벤트를 파싱·렌더하고, 사용자 동작을 tmux 명령으로 변환해 보낸다.
- 따라서 tuimux는 **tmux가 설치된 환경**을 전제로 한다(PRD NG의 "단일 바이너리" 목표는 본 단계에서 의도적으로 완화하며, PRD §8 옵션 2 단점으로 명시된 외부 의존을 수용한다).

본 SRS 범위에 **포함되지 않는** 것: 자체 PTY/VT 엔진(PRD 옵션 1), 자체 데몬, 자체 IPC 소켓 프로토콜. 이들은 tmux 서버 및 tmux 제어 프로토콜로 대체된다.

### 1.3 용어 (Definitions)
| 용어 | 정의 |
|---|---|
| tmux 서버 | 세션/창/패널과 PTY를 보유하는 백그라운드 데몬. tuimux의 백엔드. |
| control mode | `tmux -CC`로 진입하는 텍스트 제어 프로토콜. tmux가 `%`로 시작하는 비동기 알림과 명령 응답(`%begin`/`%end`/`%error`)을 stdout으로 내보낸다. |
| 세션(session) | tmux 세션. 여러 창을 가진다. |
| 창(window) | tmux 윈도우. 세션 내 탭 단위. 여러 패널을 가진다. |
| 패널(pane) | tmux pane. 하나의 PTY와 셸/프로세스를 가진다. tuimux 메인 영역에 렌더된다. |
| 메인 영역(main area) | 현재 창의 패널들이 렌더되는 화면 중앙 영역. |
| 좌측 사이드바 | 현재 폴더 파일 목록 영역. |
| 우측 사이드바 | 상단의 세션명 줄 + 그 아래 세로 창 탭 바. |
| 하단 메뉴 바 | 항상 보이는 클릭 가능한 동작 바(Detach 포함). |
| 세션 리스트 모달 | 세션명을 클릭하면 뜨는 세션 전환 오버레이. |
| 패스스루(passthrough) | 패널 내부 앱(vim/htop 등)으로 키/마우스 이벤트를 그대로 전달하는 것. |

### 1.4 참조 (References)
- PRD: `docs/prd.md` (특히 §5 요구사항, §6 UX, §8 옵션 2, §9 지속성).
- tmux control mode (`tmux(1)`의 CONTROL MODE 절): `%output`, `%window-add`, `%window-close`, `%window-renamed`, `%session-changed`, `%layout-change`, `%begin`/`%end`/`%error` 등.

### 1.5 사용자 요구사항 요약 (이 SRS가 반드시 충족)
1. VS Code에서 영감을 받은 레이아웃.
2. 좌측 사이드바: 현재 폴더 파일 목록 + 파일 크기.
3. 하단 메뉴 바: 항상 사용 가능, 마우스 클릭 가능.
4. 우측 사이드바 존재.
5. 우측 사이드바 최상단 한 줄: 현재 세션명. 클릭 시 세션 리스트 모달/창을 열어 언제든 세션 전환.
6. 세션명 아래: tmux 창들을 세로 탭 바처럼 나열. 클릭 시 메인 영역 창 전환.
7. 메인 영역: tmux 패널처럼 동작. 패널 리사이즈를 마우스로 가능해야 함.
8. 하단 메뉴 바에 Detach 포함.

이 8개 항목은 §13 추적성 표에서 기능 요구사항 ID와 매핑된다.

---

## 2. 전체 설명 (Overall Description)

### 2.1 제품 관점 (Product Perspective)
tuimux는 독립형 멀티플렉서가 아니라 **tmux 프론트엔드**다. 사용자 터미널 에뮬레이터 안에서 실행되며, 시작 시 tmux 서버에 control mode 클라이언트로 붙는다.

```
┌─────────────────────────── 사용자 터미널 에뮬레이터 ───────────────────────────┐
│                                                                              │
│   ┌───────────────────────── tuimux (클라이언트/렌더러) ─────────────────────┐ │
│   │  ratatui 렌더(좌/우 사이드바, 하단 바, 메인 영역) + crossterm 입력       │ │
│   │           ▲ stdout(이벤트 파싱)        │ stdin(명령 전송)                 │ │
│   └───────────│──────────────────────────│─────────────────────────────────┘ │
│               │  control mode 채널 (tmux -CC)                                 │
│   ┌───────────┴──────────────────────────▼─────────────────────────────────┐ │
│   │                       tmux 서버 (백엔드 데몬)                            │ │
│   │   세션 ── 창 ── 패널(PTY) ── 셸/프로세스   ·   detach/attach 지속성      │ │
│   └────────────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────────┘
```

- tuimux ↔ tmux 통신은 tmux control mode의 텍스트 프로토콜을 통한다(자체 소켓 프로토콜 없음).
- 지속성(SSH 끊김 후 생존)은 tmux 서버가 보장한다(PRD §9.6 "옵션 2: tmux 백엔드" 행).

### 2.2 사용자 클래스 (User Classes)
PRD §4 페르소나를 계승한다: P1 원격 서버 개발자, P2 터미널 입문 주니어, P3 운영/SRE. 본 SRS는 특히 P2(마우스·GUI 친화)와 P1(detach 생존)을 1차 타깃으로 한다.

### 2.3 운영 환경 (Operating Environment)
- OS: Linux 우선, macOS best-effort (PRD TD-10).
- 전제: 지원 버전의 `tmux` 바이너리가 `PATH`에 존재하고 control mode(`-CC`)를 지원해야 한다(§9 ENV-1).
- 터미널: SGR(1006) 확장 마우스 모드와 truecolor/256색을 지원하는 에뮬레이터 권장. 모노 폴백 제공(PRD NFR-4).

### 2.4 제약 (Constraints)
- C-1: 멀티플렉싱 핵심 기능은 tmux가 노출하는 control mode 이벤트·명령의 범위에 종속된다(PRD §8 옵션 2 단점).
- C-2: 지원 tmux 버전 차이로 control mode 출력 포맷이 다를 수 있어, 파서는 버전 분기를 격리해야 한다(PRD RK-6).
- C-3: prefix 키를 도입하지 않는다(PRD R-U2). 모든 전역 동작은 마우스 또는 단일 modifier(`Alt`) 또는 명령 팔레트로 수행.
- C-4: 프로세스 가시성(PRD R-X)은 tmux가 직접 주지 않으므로 `pane_pid` → `/proc` 트리 조회로 tuimux가 별도 구현한다(PRD §8 옵션 2 단점, §10 권고).

### 2.5 가정과 의존성 (Assumptions & Dependencies)
- A-1: 사용자는 tmux를 직접 조작할 필요가 없다. tuimux가 유일한 프론트엔드 진입점이다.
- A-2: tmux 서버는 tuimux가 시작할 수도, 이미 떠 있을 수도 있다(둘 다 지원).
- A-3: 좌측 파일 목록의 "현재 폴더"는 포커스된 패널의 작업 디렉터리(`pane_current_path`)를 기준으로 한다(§7.3).

---

## 3. 기능 요구사항 (Functional Requirements)

표기: **[M]** MVP 필수, **[L]** 이후, **[O]** 선택. 각 요구사항은 검증 가능하도록 기술한다.

### 3.1 tmux 백엔드 연결 (FR-CONN)
- **FR-CONN-1 [M]** tuimux 시작 시 `tmux -CC`(또는 `tmux -CC attach`)로 control mode 클라이언트를 연결한다. 기존 세션이 없으면 새 세션을 생성하여 붙는다.
- **FR-CONN-2 [M]** control mode stdout 스트림을 라인 기반으로 파싱하여 `%begin`/`%end`/`%error` 명령 응답 블록과 `%`-접두 비동기 알림을 구분 처리한다.
- **FR-CONN-3 [M]** 사용자 동작을 tmux 명령 문자열로 변환해 control mode stdin으로 전송하고, 대응하는 `%begin/%end` 응답과 매칭한다(요청-응답 상관).
- **FR-CONN-4 [M]** tmux가 지원되지 않거나(미설치/버전 미달) control mode 핸드셰이크가 실패하면 명확한 오류 화면을 표시하고 종료 코드를 반환한다(§10 ERR-1).
- **FR-CONN-5 [M]** control mode 채널이 끊기면(tmux 종료/EOF) 사용자에게 알리고, detach로 간주하여 깔끔히 종료한다(§10 ERR-3).
- **FR-CONN-6 [L]** 지원 tmux 버전 범위를 명시하고, 버전별 control mode 출력 차이를 어댑터 레이어에서 흡수한다(PRD RK-6).

### 3.2 세션 (FR-SESS)
- **FR-SESS-1 [M]** 현재 attach된 세션의 이름을 우측 사이드바 최상단 한 줄에 표시한다. (사용자 요구 5, PRD R-U4)
- **FR-SESS-2 [M]** 세션명 줄을 마우스로 클릭하면 **세션 리스트 모달**을 연다. (사용자 요구 5)
- **FR-SESS-3 [M]** 세션 리스트 모달은 tmux 서버의 모든 세션을 나열한다(세션명, 창 개수, attach 여부). 데이터는 `list-sessions`로 조회한다.
- **FR-SESS-4 [M]** 모달에서 세션 항목을 클릭(또는 키 선택)하면 해당 세션으로 전환한다(control mode에서 `switch-client -t <session>`). 전환 후 모달을 닫고 우측 사이드바·메인 영역·좌측 파일 목록을 갱신한다.
- **FR-SESS-5 [M]** 모달에는 "새 세션 만들기" 진입점을 포함한다(`new-session`). 생성 후 해당 세션으로 전환.
- **FR-SESS-6 [M]** 모달은 마우스 클릭(바깥/닫기 버튼) 또는 `Esc`로 닫을 수 있다.
- **FR-SESS-7 [M]** 세션은 사용자가 명시적으로 detach할 때까지 백그라운드 유지된다(지속성은 tmux 보장, PRD R-P1/R-P3).
- **FR-SESS-8 [L]** 모달에서 세션 이름 변경(`rename-session`)·삭제(`kill-session`)를 제공한다.

### 3.3 창(window) — 우측 세로 탭 바 (FR-WIN)
- **FR-WIN-1 [M]** 우측 사이드바의 세션명 줄 **아래**에, 현재 세션의 tmux 창 목록을 **세로 탭 바**로 나열한다. (사용자 요구 6, PRD R-U1)
- **FR-WIN-2 [M]** 각 탭은 창 인덱스와 창 이름을 표시한다. 현재 활성 창은 시각적으로 구분(예: `▸` 마커/하이라이트)한다.
- **FR-WIN-3 [M]** 창 탭을 클릭하면 메인 영역이 해당 창으로 전환된다(control mode `select-window -t <id>`). (사용자 요구 6)
- **FR-WIN-4 [M]** 세로 탭 바에 "새 창(+ new)" 항목을 두고, 클릭 시 새 창을 만들고 전환한다(`new-window`).
- **FR-WIN-5 [M]** 창 추가/삭제/이름변경/활성 변경 이벤트(`%window-add`, `%window-close`, `%window-renamed`, `%session-window-changed` 등)를 수신하여 탭 바를 실시간 갱신한다.
- **FR-WIN-6 [M]** 키보드 대등 경로: `Alt-1..9`로 인덱스 전환, `Alt-↑/↓`로 인접 창 전환(PRD §6.2).
- **FR-WIN-7 [L]** 탭 우클릭 컨텍스트 메뉴로 창 이름변경·닫기.
- **FR-WIN-8 [O]** 창에 변화(프로세스 종료/벨/출력)가 있으면 탭에 표식(점/색)을 표시.

### 3.4 메인 영역 / 패널 (FR-PANE)
- **FR-PANE-1 [M]** 메인 영역은 현재 창의 tmux 패널 레이아웃을 그대로 렌더한다(패널 위치·크기·내용). 패널 내용은 `%output`(또는 `capture-pane` 초기 스냅샷)으로 채운다.
- **FR-PANE-2 [M]** 패널 클릭 시 해당 패널로 포커스 이동(`select-pane -t <id>`). 포커스 패널은 시각적으로 구분(테두리 강조).
- **FR-PANE-3 [M]** **패널 경계를 마우스로 드래그하여 리사이즈**한다. 드래그 결과를 tmux `resize-pane`(방향·셀 수)으로 반영한다. (사용자 요구 7, PRD R-U3)
- **FR-PANE-4 [M]** 경계에 마우스를 올리면 리사이즈 가능함을 알리는 커서/시각 힌트를 표시한다(가능한 경우).
- **FR-PANE-5 [M]** 패널 분할: 수평/수직 분할 동작 제공(`split-window -h` / `-v`). 진입점은 우클릭 메뉴 또는 하단 바(PRD R-S4, §6.2).
- **FR-PANE-6 [M]** 패널 닫기: 포커스 패널을 닫는다(`kill-pane`). 마지막 패널/창 닫기 시 안전 처리(§10 ERR-5).
- **FR-PANE-7 [M]** 레이아웃 변경 이벤트(`%layout-change`)를 수신해 메인 영역 패널 배치를 재계산·재렌더한다.
- **FR-PANE-8 [M]** 키보드 대등 리사이즈: `Alt-Shift-방향키`(PRD §6.2). 키보드 분할: `Alt-|`, `Alt--`.
- **FR-PANE-9 [M]** 패널 입력: 포커스된 패널 영역 안의 키 입력은 해당 tmux 패널로 전달된다(`send-keys` 또는 control mode 입력 경로). 단 전역 `Alt` 조합은 tuimux가 가로챈다(§5.4).
- **FR-PANE-10 [L]** 패널 이동/스왑, 줌(임시 전체화면)(PRD R-S5/R-S6).

### 3.5 좌측 사이드바 — 파일 목록 (FR-FILES)
- **FR-FILES-1 [M]** 좌측 사이드바에 "현재 폴더"의 파일/디렉터리 목록을 표시한다. (사용자 요구 2)
- **FR-FILES-2 [M]** 각 항목에 **파일 크기**를 표시한다(사람이 읽기 쉬운 단위: B/KB/MB/GB). 디렉터리는 크기 대신 디렉터리 표식을 표시. (사용자 요구 2)
- **FR-FILES-3 [M]** "현재 폴더"의 기준은 포커스된 패널의 작업 디렉터리(`pane_current_path`)다. 포커스 패널이 바뀌거나 디렉터리가 변하면 목록을 갱신한다(§7.3 폴링/이벤트).
- **FR-FILES-4 [M]** 목록은 디렉터리 우선 정렬 후 이름 정렬(기본), 항목은 스크롤 가능(목록이 길면 휠/키 스크롤).
- **FR-FILES-5 [L]** 디렉터리 항목 클릭 시 하위로 진입/상위로 이동(`..`)하여 목록 기준 경로를 탐색한다(패널의 실제 cwd와는 독립적인 브라우징 모드).
- **FR-FILES-6 [L]** 파일 항목 클릭 시 동작(미리보기/포커스 패널에 경로 삽입). 구체 동작은 설정 가능.
- **FR-FILES-7 [O]** 숨김 파일 토글, 정렬 기준(크기/수정시간) 토글.

### 3.6 하단 메뉴 바 (FR-BAR)
- **FR-BAR-1 [M]** 화면 하단에 **항상 보이는 메뉴 바**를 둔다. 어떤 모달/모드에서도(모달은 그 위에 뜸) 메뉴 바 영역은 유지된다. (사용자 요구 3, PRD R-U4)
- **FR-BAR-2 [M]** 메뉴 바의 각 항목은 **마우스로 클릭 가능**하며, 클릭 시 해당 동작을 즉시 실행한다. (사용자 요구 3)
- **FR-BAR-3 [M]** 메뉴 바는 최소한 다음 항목을 포함한다: **Detach**, New(창/패널), Split, Close, Help(`?`), Palette. (사용자 요구 8, PRD §6.1 하단 힌트 바)
- **FR-BAR-4 [M]** **Detach** 클릭 시 현재 클라이언트를 tmux에서 detach한다(control mode `detach-client`). 세션과 프로세스는 백그라운드 유지되고 tuimux는 셸로 빠져나온다. (사용자 요구 8, PRD R-P2/§6.2)
- **FR-BAR-5 [M]** 각 항목에는 동등한 키보드 단축키를 병기 표시한다(예: `Detach Alt-d`)(PRD G5/R-U2).
- **FR-BAR-6 [O]** 메뉴 바에 상태 정보(세션명/접속 클라이언트 수/시간) 표시를 옵션으로 통합(PRD R-U4 상태 바와 통합 가능).

### 3.7 마우스·입력 공통 (FR-INPUT)
- **FR-INPUT-1 [M]** SGR(1006) 확장 마우스 모드로 클릭·드래그·휠 좌표를 수신한다(PRD TD-6).
- **FR-INPUT-2 [M]** 화면 영역별 마우스 라우팅: 좌측 사이드바/우측 사이드바/하단 바/패널 경계는 tuimux UI가 가로채고, 패널 내용 영역은 내부 앱으로 패스스루한다(PRD TD-6, RK-4).
- **FR-INPUT-3 [M]** 휠 스크롤: 패널 내용 위에서는 스크롤백(tmux copy-mode 스크롤), 좌측 파일 목록/모달 위에서는 해당 리스트 스크롤.
- **FR-INPUT-4 [M]** 우클릭 컨텍스트 메뉴: 패널(분할/닫기), 창 탭(이름변경/닫기) 등 위치별 메뉴(PRD R-U3).
- **FR-INPUT-5 [M]** prefix 없음. 전역 동작은 `Alt` 조합 또는 마우스 또는 명령 팔레트로만 수행(PRD R-U2/TD-5).
- **FR-INPUT-6 [M]** 명령 팔레트(`Alt-p`, 폴백 `Ctrl-Space`): 모든 동작을 검색·실행하는 단일 진입점(PRD R-U5).

### 3.8 프로세스 가시성 (FR-PROC) — 차별화
- **FR-PROC-1 [M]** 현재 세션의 패널별 포그라운드 프로세스(명령줄, PID, 실행시간)를 리스트로 표시한다. `pane_pid`를 기준으로 `/proc` 트리에서 포그라운드 프로세스를 해석한다(PRD R-X1, C-4).
- **FR-PROC-2 [M]** 프로세스 종료 시 종료 상태(exit code, 성공/실패 색상)를 표시한다(PRD R-X2).
- **FR-PROC-3 [M]** 리스트에서 프로세스 선택 후 신호 전송(SIGINT/SIGTERM/SIGKILL)을 메뉴로 제공한다(PRD R-X3).
- **FR-PROC-4 [O]** PROCS 패널의 화면 배치(우측 사이드바 하단 vs 별도 오버레이)는 설정 가능. MVP 기본은 우측 사이드바 하단(PRD §6.1).
- **FR-PROC-5 [L]** 패널별 CPU/메모리 표시, 완료/실패 알림(PRD R-X4/R-X5).

### 3.9 도움말/발견성 (FR-HELP)
- **FR-HELP-1 [M]** `?` 또는 하단 바 Help 클릭으로 전체 단축키 도움말 오버레이를 연다. 마우스로도 닫을 수 있다(PRD R-U4).
- **FR-HELP-2 [M]** 모든 마우스 동작에 동등한 키보드 단축키가 존재하며 도움말에 노출된다(PRD G5).

---

## 4. 비기능 요구사항 (Non-Functional Requirements)

- **NFR-1 (지연)** 키 입력→화면 반영 < 16ms 목표, 첫 화면 렌더 < 100ms(단, tmux 연결 핸드셰이크 시간 제외)(PRD NFR-1/성공지표).
- **NFR-2 (자원)** 유휴 시 CPU ≈ 0%. control mode 스트림은 이벤트 기반으로 처리하고 busy-loop를 금지한다. 파일 목록/프로세스 폴링은 가시 상태에서만, 합리적 주기(기본 1–2초, 설정 가능)로 수행한다(PRD NFR-2).
- **NFR-3 (안정성)** tuimux(클라이언트) 패닉/종료가 tmux 서버나 세션을 죽이지 않는다. 프로세스 분리는 tmux 데몬 구조로 보장된다(PRD NFR-3).
- **NFR-4 (호환성)** `$TERM`/색 지원에 따른 기능 차등(truecolor/256/모노 폴백), ASCII/Unicode 경계 문자 토글(PRD NFR-4/R-U7).
- **NFR-5 (이식성/의존성)** tmux 의존을 명시한다. 지원 tmux 최소 버전과 권장 버전을 문서화하고, 미충족 시 친절한 오류를 낸다(C-1/C-2, PRD RK-6).
- **NFR-6 (보안)** tuimux는 추가 네트워크 포트를 열지 않는다. tmux 소켓 권한 모델을 그대로 따른다. 좌측 파일 목록은 현재 사용자 권한으로 읽기만 하며 임의 경로 노출을 막는다(기본은 패널 cwd 하위)(PRD NFR-5).
- **NFR-7 (반응성/리사이즈)** 터미널 리사이즈 시 좌/우 사이드바·하단 바·메인 영역을 재배치하고, 메인 영역 크기를 tmux 클라이언트 크기로 반영한다(다중 attach 정책은 §6 상태/RK-5).
- **NFR-8 (국제화/렌더)** 와이드/조합 문자, 한글 폭 계산을 올바르게 처리하여 패널·목록 정렬이 깨지지 않게 한다.
- **NFR-9 (관측성)** 디버그 로그(옵션)로 control mode 송수신을 기록할 수 있어 버전 차이 디버깅을 돕는다(C-2).

---

## 5. UI 레이아웃 및 인터랙션 명세 (UI & Interaction)

### 5.1 화면 레이아웃 (VS Code 영감)
사용자 요구 1~8을 모두 반영한 영역 구성:

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ (선택) 상단 상태 줄 — 통합 시 하단 바로 흡수 가능                              │
├───────────────┬──────────────────────────────────────────┬───────────────────┤
│  EXPLORER     │            MAIN AREA (panes)             │  session: dev   ▾  │ ← 세션명(클릭→모달)
│  ~/proj       │  ┌──────────────┬───────────────────┐    │ ───────────────────│
│  ▸ src/       │  │ pane 0 (focus)│ pane 1            │    │  WINDOWS           │ ← 세로 창 탭 바
│    main.rs 12K│  │ $ cargo build │ $ htop            │    │ ▸ 1: build         │
│    lib.rs  4K │  │ Compiling…    │                   │    │   2: logs          │
│  ▸ docs/      │  ├──────────────┤  (drag border ↔)  │    │   3: ssh           │
│    prd.md 18K │  │ pane 2        │                   │    │   + new            │
│    srs.md  …  │  │ $ tail -f log │                   │    │ ───────────────────│
│  Cargo.toml 1K│  └──────────────┴───────────────────┘    │  PROCS             │
│  README.md 2K │                                          │ ● build…  pid 4211 │
│               │                                          │ ● htop    pid 4250 │
│               │                                          │ ✓ test            │
├───────────────┴──────────────────────────────────────────┴───────────────────┤
│ [Detach Alt-d] [New Alt-n] [Split Alt-|] [Close Alt-w] [? Help] [Palette Alt-p]│ ← 하단 메뉴 바(항상)
└──────────────────────────────────────────────────────────────────────────────┘
```

영역 정의:
- **좌측 사이드바(EXPLORER)**: 현재 폴더 파일 목록 + 크기(FR-FILES). 폭은 기본 고정, 경계 드래그로 조절(L).
- **메인 영역(MAIN AREA)**: tmux 패널 레이아웃 미러링(FR-PANE). 패널 경계 드래그 리사이즈.
- **우측 사이드바 상단**: `session: <name>` 한 줄, 클릭 시 세션 모달(FR-SESS-1/2).
- **우측 사이드바 중단(WINDOWS)**: 세로 창 탭 바(FR-WIN). `▸` = 활성, `+ new` 포함.
- **우측 사이드바 하단(PROCS)**: 프로세스 리스트(FR-PROC), MVP 기본 위치.
- **하단 메뉴 바**: 항상 보임·클릭 가능, Detach 포함(FR-BAR).

### 5.2 세션 리스트 모달 (FR-SESS-2~6)
```
        ┌──────────── Sessions ────────────┐
        │  ● dev        3 windows  (attached)│
        │    work       2 windows            │
        │    scratch    1 window             │
        │ ────────────────────────────────── │
        │  + New session…                    │
        │                          [Esc 닫기]│
        └────────────────────────────────────┘
```
- 메인 화면 위 중앙 오버레이로 표시. 하단 메뉴 바는 계속 보인다(FR-BAR-1).
- 항목 클릭 → `switch-client` → 닫기 + 전체 갱신(FR-SESS-4).
- 바깥 클릭/닫기 버튼/`Esc` → 닫기(FR-SESS-6).

### 5.3 인터랙션 표 (요약)
| 동작 | 마우스 | 키보드 | tmux 명령 | FR |
|---|---|---|---|---|
| 세션 모달 열기 | 세션명 줄 클릭 | `Alt-s` | `list-sessions` | FR-SESS-2/3 |
| 세션 전환 | 모달 항목 클릭 | `↑↓`+`Enter` | `switch-client -t` | FR-SESS-4 |
| 창 전환 | 창 탭 클릭 | `Alt-1..9`/`Alt-↑↓` | `select-window -t` | FR-WIN-3/6 |
| 새 창 | `+ new` 클릭 | `Alt-n` | `new-window` | FR-WIN-4 |
| 패널 포커스 | 패널 클릭 | `Alt-방향키` | `select-pane -t` | FR-PANE-2 |
| 패널 리사이즈 | 경계 드래그 | `Alt-Shift-방향` | `resize-pane` | FR-PANE-3/8 |
| 패널 분할 | 우클릭 메뉴/하단 바 | `Alt-|`,`Alt--` | `split-window -h/-v` | FR-PANE-5 |
| 패널 닫기 | 우클릭 메뉴/하단 바 | `Alt-w` | `kill-pane` | FR-PANE-6 |
| Detach | 하단 바 Detach 클릭 | `Alt-d` | `detach-client` | FR-BAR-4 |
| 도움말 | 하단 바 `?` 클릭 | `?` | — | FR-HELP-1 |
| 팔레트 | 하단 바 Palette 클릭 | `Alt-p`/`Ctrl-Space` | (동작 디스패치) | FR-INPUT-6 |
| 스크롤백 | 패널 위 휠 | copy-mode 키 | copy-mode 스크롤 | FR-INPUT-3 |
| 프로세스 신호 | PROCS 우클릭 메뉴 | 선택+메뉴 | `kill -SIG`(직접) | FR-PROC-3 |

### 5.4 키 라우팅 정책 (No-prefix)
- 전역 가로채기: `Alt`(Meta) 조합, `?`(도움말), 팔레트 트리거(`Alt-p`/`Ctrl-Space`).
- 그 외 모든 키는 포커스된 패널의 tmux로 패스스루(FR-PANE-9).
- 모달/팔레트/도움말이 열려 있으면 그 위젯이 키 입력을 우선 소비하고, 닫히면 패널 패스스루로 복귀.
- `Alt`가 막힌 터미널을 위해 마우스 메뉴와 명령 팔레트라는 prefix-free 대체 경로를 항상 제공(PRD §6.3/RK-3).

---

## 6. 상태 모델 (State Model)

### 6.1 애플리케이션 상태 머신
```
        ┌─────────────┐  연결 성공   ┌──────────────┐
   start│ Connecting  ├────────────▶│  Attached    │◀──────────┐
        └──────┬──────┘             └──┬────┬────┬──┘           │ 모달/오버레이 닫기
               │ 실패                  │    │    │ 모달/팔레트/도움말 열기
               ▼                       │    │    ▼
        ┌─────────────┐                │    │  ┌──────────────┐
        │   Error     │                │    │  │ Overlay 활성 │──────────┘
        └─────────────┘                │    │  └──────────────┘
                                       │    │ Detach(Alt-d/버튼)
                          채널 EOF/끊김 │    ▼
                                       │  ┌──────────────┐
                                       └─▶│  Detaching   │──▶ exit(셸로 복귀)
                                          └──────────────┘
```
- **Connecting**: tmux control mode 핸드셰이크. 타임아웃 시 Error(ERR-1).
- **Attached**: 정상 운영 상태. 메인 루프가 control mode 이벤트와 입력 이벤트를 처리.
- **Overlay 활성**: 세션 모달/명령 팔레트/도움말/컨텍스트 메뉴가 떠 있는 하위 상태. 키 입력은 오버레이가 우선 소비. 하단 바는 계속 보임.
- **Detaching**: 사용자 Detach 또는 채널 EOF. 화면 정리 후 종료.
- **Error**: 치명 오류 표시 후 종료(§10).

### 6.2 미러 상태(클라이언트가 보유)
tuimux는 tmux를 진실 원천(source of truth)으로 삼고, control mode 이벤트로 동기화되는 **읽기용 미러**를 메모리에 유지한다(§7 데이터 모델). 사용자 동작은 미러를 직접 수정하지 않고 tmux 명령을 보낸 뒤, 돌아오는 이벤트로 미러를 갱신한다(낙관적 UI는 MVP 비목표 — 단순성 우선).

### 6.3 다중 클라이언트/리사이즈 정책
- 같은 세션에 다른 클라이언트(다른 tuimux/tmux)가 attach될 수 있다(PRD R-P4).
- MVP 리사이즈 정책: tmux 기본 동작을 따른다(가장 작은 클라이언트에 맞춤 또는 `aggressive-resize` 설정에 의존). 정교한 협상은 이후(PRD RK-5/OQ-2). tuimux는 메인 영역 가용 셀 크기를 tmux 클라이언트 크기로 보고한다.

---

## 7. 데이터 모델 (Data Models)

미러 상태의 논리적 구조(직렬화 포맷이 아니라 개념 모델):

### 7.1 핵심 엔티티
```
Server
 └─ sessions: [Session]
    Session { id, name, attached: bool, windows: [Window], active_window_id }
       Window { id, index, name, active: bool, panes: [Pane], layout }
          Pane { id, index, active: bool,
                 geometry { x, y, width, height },   // 메인 영역 렌더용
                 pane_pid,                            // /proc 조회 기준
                 current_path,                        // 좌측 파일 목록 기준
                 title }
```
- 출처: `list-sessions`, `list-windows`, `list-panes`, 그리고 `%`-이벤트 갱신.
- `geometry`는 `%layout-change`/`list-panes -F`의 좌표로 산출.

### 7.2 파생 모델 — 좌측 파일 목록
```
FileEntry { name, is_dir: bool, size_bytes: u64, size_display: String, mtime }
FileListing { base_path, entries: [FileEntry], scroll_offset }
```
- `base_path`는 포커스 패널의 `current_path`(FR-FILES-3) 또는 브라우징 모드의 탐색 경로(FR-FILES-5, L).
- `size_display`는 사람이 읽기 쉬운 단위 변환(FR-FILES-2).

### 7.3 파생 모델 — 프로세스
```
ProcEntry { pane_id, pid, cmdline, started_at, state: Running|Exited,
            exit_code: Option<i32> }
```
- `pane_pid`에서 출발해 `/proc/<pid>` 및 포그라운드 프로세스 그룹을 조회(C-4, FR-PROC-1).
- 종료 감지 시 `state=Exited` + `exit_code`(가능한 범위)로 갱신(FR-PROC-2).

### 7.4 갱신 트리거
| 모델 | 트리거 |
|---|---|
| 세션/창/패널 미러 | control mode `%`-이벤트(비동기) + 주기/요청 시 `list-*` 동기화 |
| 파일 목록 | 포커스 패널 변경, `current_path` 변경 감지, 주기 폴링(가시 시), 수동 새로고침 |
| 프로세스 | 주기 폴링(가시 시, 기본 1–2초), 패널/포커스 변경 |

---

## 8. 이벤트 흐름 (Event Flows)

### 8.1 시작/연결
```
사용자: tuimux 실행
 → tuimux: tmux 존재/버전 확인 (ENV-1)
 → tuimux: `tmux -CC (attach|new-session)` 기동, control mode 채널 연결 (FR-CONN-1)
 → tmux: %begin … 초기 상태(세션/창/패널) … %end
 → tuimux: 미러 구축 → 좌/우 사이드바·메인·하단 바 렌더 (Attached)
```

### 8.2 창 전환(클릭)
```
사용자: 우측 창 탭 "2: logs" 클릭
 → tuimux: 클릭 좌표 → WINDOWS 영역 → window id 매핑 (FR-INPUT-2)
 → tuimux: control mode로 `select-window -t @<id>` 전송 (FR-WIN-3)
 → tmux: %session-window-changed / %layout-change 알림
 → tuimux: active_window 갱신, 메인 영역 재렌더, 포커스 패널 cwd로 좌측 파일 목록 갱신
```

### 8.3 세션 전환(모달)
```
사용자: 우측 상단 "session: dev" 클릭
 → tuimux: Overlay 활성, `list-sessions` 조회 → 모달 렌더 (FR-SESS-2/3)
사용자: 모달에서 "work" 클릭
 → tuimux: `switch-client -t work` 전송 (FR-SESS-4)
 → tmux: %session-changed
 → tuimux: 모달 닫기, Attached 복귀, 우측 탭/메인/좌측 전체 갱신
```

### 8.4 패널 리사이즈(드래그)
```
사용자: pane0/pane1 경계에 mouse-down → drag → mouse-up
 → tuimux: mouse-down 좌표가 경계 히트박스인지 판정 (FR-PANE-4)
 → drag 중: 이동 델타(셀 수)를 누적, 프리뷰(가능 시) 표시
 → mouse-up: `resize-pane -t <pane> -x/-y <cells>` (또는 -L/-R/-U/-D <delta>) 전송 (FR-PANE-3)
 → tmux: %layout-change → tuimux: 새 geometry로 메인 영역 재배치
```

### 8.5 Detach
```
사용자: 하단 바 "Detach" 클릭 (또는 Alt-d)
 → tuimux: `detach-client` 전송 (FR-BAR-4)
 → tmux: control mode 채널 종료(%exit 또는 EOF)
 → tuimux: Detaching → 화면 정리/원복 → exit 0 (세션은 백그라운드 생존)
```

### 8.6 패널 출력 스트리밍
```
tmux: %output %<pane-id> <data>  (비동기, 연속)
 → tuimux: 해당 pane 버퍼에 반영 → 가시 창의 패널이면 부분 재렌더 (FR-PANE-1, NFR-2)
```

### 8.7 프로세스 신호 전송
```
사용자: PROCS에서 "build…" 우클릭 → "Send SIGINT"
 → tuimux: 해당 ProcEntry.pid 확인
 → tuimux: 신호 전송(SIGINT) (FR-PROC-3)
 → 다음 폴링/종료 감지: state=Exited, exit_code 표시 (FR-PROC-2)
```

---

## 9. tmux Control Mode 통합 명세 (Integration)

### 9.1 환경/전제
- **ENV-1**: `tmux` 바이너리가 `PATH`에 있고 control mode를 지원하는 버전이어야 한다. 미충족 시 ERR-1.
- **ENV-2**: tuimux는 control mode 클라이언트로만 동작한다. tmux의 키 테이블/prefix 설정에 의존하지 않으며, 사용자에게 tmux 단축키를 노출하지 않는다.

### 9.2 명령 송신 (tuimux → tmux)
- 길이 제약 없는 줄 단위 명령을 control mode stdin으로 전송한다. 각 명령은 `%begin <id> ... %end <id>` 또는 `%error <id>`로 응답되며, tuimux는 id로 요청-응답을 상관한다(FR-CONN-3).
- 사용하는 명령(개념): `attach-session`/`new-session`, `list-sessions`, `list-windows`, `list-panes -F`, `switch-client -t`, `select-window -t`, `new-window`, `select-pane -t`, `split-window -h/-v`, `kill-pane`, `resize-pane`, `detach-client`. 키 입력 전달은 control mode 입력 경로/`send-keys`를 사용.
- 명세는 명령의 **의미**를 고정하되 정확한 플래그 조합은 지원 tmux 버전에 맞춰 어댑터에서 확정한다(C-2/FR-CONN-6).

### 9.3 이벤트 수신 (tmux → tuimux)
파서가 처리해야 하는 비동기 알림(개념):
| 알림 | tuimux 처리 |
|---|---|
| `%output %<pane> <data>` | 패널 버퍼 갱신·재렌더 (FR-PANE-1) |
| `%window-add` / `%window-close` | 창 탭 바 추가/제거 (FR-WIN-5) |
| `%window-renamed` | 창 탭 라벨 갱신 (FR-WIN-5) |
| `%session-changed` | 현재 세션 전환 반영 (FR-SESS-4) |
| `%session-window-changed` / 활성창 변경 | 활성 창 마커·메인 영역 갱신 (FR-WIN-2/3) |
| `%layout-change` | 패널 geometry 재계산·메인 영역 재배치 (FR-PANE-7) |
| `%sessions-changed` | 세션 모달 목록 무효화 (FR-SESS-3) |
| `%exit` / EOF | Detaching 처리 (FR-CONN-5) |
| `%begin/%end/%error <id>` | 명령 응답 상관 (FR-CONN-2) |

### 9.4 격리/버전 흡수
- control mode 파싱·명령 생성은 **어댑터 레이어**로 캡슐화하여, tmux 버전별 출력 차이를 상위 UI 로직에서 격리한다(PRD RK-6, C-2).
- 어댑터는 시작 시 tmux 버전을 질의하고(예: `display-message`로 버전), 알려진 차이를 분기 처리한다.

---

## 10. 오류 처리 (Error Handling)

| ID | 상황 | 처리 | 관련 |
|---|---|---|---|
| **ERR-1** | tmux 미설치/버전 미달/control mode 미지원 | 명확한 안내 화면("tmux ≥ <ver> 필요") + 비정상 종료 코드. 설치 힌트 표시. | FR-CONN-4, ENV-1, NFR-5 |
| **ERR-2** | `%error` 응답(명령 실패) | 비치명: 토스트/상태줄에 사유 표시, 미러 불변. 사용자 동작만 무효. | FR-CONN-2/3 |
| **ERR-3** | control mode 채널 EOF/끊김(비-Detach) | Detaching으로 전이, 화면 원복, 사유 메시지. 세션은 tmux가 유지. | FR-CONN-5 |
| **ERR-4** | 이벤트 파싱 실패(미지의 포맷) | 해당 라인 스킵 + 디버그 로그 기록(NFR-9), 동기화 보정 위해 `list-*` 재조회. | C-2 |
| **ERR-5** | 마지막 패널/창 닫기 시도 | 확인 또는 안전 정책(창 닫힘→세션 유지/세션 비면 detach). 의도치 않은 세션 종료 방지. | FR-PANE-6 |
| **ERR-6** | 좌측 파일 목록: 경로 접근 불가/권한 없음 | 빈 목록 + "접근 불가" 표시, 크래시 금지. | FR-FILES-1, NFR-6 |
| **ERR-7** | 프로세스 조회 실패(`/proc` 없음, PID 소멸) | 해당 항목 비표시/정리, 부분 실패 허용. | FR-PROC-1 |
| **ERR-8** | 신호 전송 실패(권한/소멸) | 비치명 메시지, 다음 폴링에서 상태 정정. | FR-PROC-3 |
| **ERR-9** | 터미널 마우스/색 미지원 | 마우스 기능 비활성 + 키보드 경로 안내(PRD G5), 모노 폴백 렌더. | NFR-4, FR-INPUT-1 |

원칙: 클라이언트의 어떤 오류도 tmux 세션/프로세스를 죽이지 않는다(NFR-3). 비치명 오류는 동작만 거부하고 미러를 보존한다.

---

## 11. 인수 기준 (Acceptance Criteria)

각 기준은 수동/자동으로 검증 가능해야 한다.

- **AC-1 (연결)** tmux가 설치된 환경에서 tuimux 실행 시 control mode로 붙어 현재 세션의 창/패널이 화면에 렌더된다. (FR-CONN-1, FR-PANE-1)
- **AC-2 (좌측 파일 목록)** 좌측 사이드바에 포커스 패널 cwd의 파일/디렉터리가 표시되고, 각 파일에 사람이 읽을 수 있는 크기가 보인다. cwd가 바뀌면 목록이 갱신된다. (FR-FILES-1/2/3)
- **AC-3 (하단 바·Detach)** 하단 메뉴 바가 항상 보이고, 마우스로 Detach를 클릭하면 tuimux가 종료되며 셸로 돌아온다. 재실행/attach 시 세션과 그 안의 프로세스가 그대로 살아 있다. (FR-BAR-1~4, PRD §7 DoD)
- **AC-4 (우측 세션명·모달)** 우측 상단에 현재 세션명이 한 줄로 보이고, 클릭하면 세션 리스트 모달이 열려 다른 세션을 클릭해 즉시 전환할 수 있다. (FR-SESS-1/2/4)
- **AC-5 (우측 창 탭)** 세션명 아래에 현재 세션의 창들이 세로 탭으로 나열되고, 탭을 클릭하면 메인 영역이 해당 창으로 전환된다. 창 추가/삭제가 실시간 반영된다. (FR-WIN-1/3/5)
- **AC-6 (패널 리사이즈)** 메인 영역에서 두 패널 사이 경계를 마우스로 드래그하면 패널 크기가 변하고, 그 결과가 tmux 레이아웃에 반영된다. (FR-PANE-3, FR-PANE-7)
- **AC-7 (패널 포커스/분할/닫기)** 패널 클릭으로 포커스가 이동하고, 분할/닫기를 마우스 또는 단축키로 수행할 수 있다. (FR-PANE-2/5/6)
- **AC-8 (No-prefix·키보드 대등)** prefix 키 없이 모든 핵심 동작을 마우스로 수행할 수 있고, 동등한 `Alt`/단축키 경로가 도움말에 노출된다. (FR-INPUT-5, FR-HELP)
- **AC-9 (프로세스 가시성)** PROCS 리스트에 실행 중 프로세스(PID/명령/실행시간)가 보이고, 종료 시 종료 상태가 표시되며, 우클릭으로 신호를 보낼 수 있다. (FR-PROC-1/2/3)
- **AC-10 (안정성)** tuimux를 강제 종료해도 tmux 세션과 프로세스가 살아남고, 다시 붙으면 상태가 복구된다. (NFR-3, PRD R-P1/R-P3)
- **AC-11 (오류)** tmux가 없거나 버전이 낮은 환경에서 실행하면 크래시 없이 안내 메시지를 띄우고 종료한다. (ERR-1)
- **AC-12 (지연/유휴)** 유휴 상태에서 CPU 사용이 0%에 수렴하고, 입력 반응이 체감상 즉시다. (NFR-1/2)

### 11.1 핵심 검증 시나리오 (PRD §7 DoD 매핑)
SSH 접속 → tuimux 실행 → 빌드 명령 시작 → 하단 바 Detach(또는 SSH 강제 종료) → 재접속 → tuimux 재실행(attach) → 빌드가 계속 진행 중이고 출력이 이어진다. 이 시나리오 100% 재현이 핵심 통과 기준. (AC-3/AC-10)

---

## 12. MVP 경계 (MVP Scope)

### 12.1 MVP 포함
- tmux control mode 연결/이벤트 파싱/명령 송신(FR-CONN-1~5).
- 세션명 표시 + 세션 리스트 모달 + 세션 전환/생성(FR-SESS-1~7).
- 우측 세로 창 탭 바 + 전환 + 실시간 갱신(FR-WIN-1~6).
- 메인 영역 패널 렌더 + 포커스 + 마우스 드래그 리사이즈 + 분할/닫기(FR-PANE-1~9).
- 좌측 파일 목록 + 크기 + cwd 연동(FR-FILES-1~4).
- 항상 보이는 클릭형 하단 메뉴 바 + Detach(FR-BAR-1~5).
- 마우스 라우팅/휠/우클릭/팔레트/No-prefix(FR-INPUT-1~6).
- PROCS: 실행 표시 + 종료 상태 + 신호 전송(FR-PROC-1~3).
- 도움말 오버레이(FR-HELP-1/2).

### 12.2 MVP 제외(이후/선택)
- 파일 목록 브라우징(디렉터리 진입)·파일 클릭 동작(FR-FILES-5/6, L).
- 패널 이동/스왑/줌(FR-PANE-10, L).
- 창 탭 변화 표식/탭 우클릭(FR-WIN-7/8).
- 세션 이름변경/삭제(FR-SESS-8).
- CPU/메모리 표시·알림(FR-PROC-5).
- 정교한 다중 클라이언트 리사이즈 협상(§6.3, PRD RK-5).
- macOS 정식 지원, Windows(PRD NG4/TD-10).

### 12.3 본 SRS가 PRD에서 의도적으로 고정/완화한 사항
- 아키텍처를 옵션 2(tmux control mode)로 **고정**. 옵션 1(자체 엔진)·자체 데몬·자체 IPC는 본 SRS 범위 밖.
- "단일 바이너리/무의존" 목표(PRD G6/NG)는 **완화**: tmux 의존을 명시 전제로 수용(PRD §8 옵션 2 단점, RK-8 인지).
- 좌측 파일 목록(파일 크기 포함)은 PRD에 없던 **사용자 신규 요구**로, 본 SRS에서 FR-FILES로 정식 편입.

---

## 13. 추적성 (Traceability)

### 13.1 사용자 요구 → 기능 요구사항
| # | 사용자 요구 | 기능 요구사항 |
|---|---|---|
| 1 | VS Code 영감 레이아웃 | §5.1, FR-FILES, FR-WIN, FR-BAR |
| 2 | 좌측 사이드바 파일 목록 + 크기 | FR-FILES-1, FR-FILES-2, FR-FILES-3 |
| 3 | 하단 메뉴 바 항상/클릭 가능 | FR-BAR-1, FR-BAR-2, FR-BAR-3 |
| 4 | 우측 사이드바 존재 | §5.1, FR-SESS-1, FR-WIN-1 |
| 5 | 우측 상단 세션명 + 클릭→세션 모달 전환 | FR-SESS-1, FR-SESS-2, FR-SESS-3, FR-SESS-4 |
| 6 | 세션명 아래 세로 창 탭 + 클릭 전환 | FR-WIN-1, FR-WIN-2, FR-WIN-3, FR-WIN-5 |
| 7 | 메인 영역 패널 + 마우스 리사이즈 | FR-PANE-1, FR-PANE-2, FR-PANE-3, FR-PANE-7 |
| 8 | 하단 바에 Detach 포함 | FR-BAR-3, FR-BAR-4 |

### 13.2 기능 요구사항 → PRD 매핑
| SRS | PRD |
|---|---|
| FR-CONN-* | §8 옵션 2a(control mode), TD-11, RK-6 |
| FR-SESS-* | R-U1, R-U4, R-P1/R-P2, OQ-1 |
| FR-WIN-* | R-U1, §6.1 TABS, §6.2 |
| FR-PANE-* | R-S2/R-S3/R-S4, R-U3, §6.2 |
| FR-FILES-* | (PRD 미수록, 사용자 신규 요구 — §12.3) |
| FR-BAR-* | R-U4, §6.1 하단 힌트 바, §6.2 detach, R-P2 |
| FR-INPUT-* | R-U2/R-U3/R-U5, TD-5/TD-6, §6.3, RK-3/RK-4 |
| FR-PROC-* | R-X1/R-X2/R-X3, §8 옵션 2 단점(/proc), C-4 |
| FR-HELP-* | R-U4, G2/G5 |
| NFR-1~9 | NFR-1~5, G5, RK-5/RK-6 |
| 지속성/Detach | §9.2/§9.6(옵션 2 행), R-P1~R-P3, §7 DoD |

### 13.3 미해결 질문 매핑 (PRD §13)
- OQ-1(탭 단위): 본 SRS는 **창=세로 탭, 세션=상단 모달**로 분리 결정(2단 구조). (FR-WIN/FR-SESS)
- OQ-2(다중 attach 리사이즈): MVP는 tmux 기본 정책 위임(§6.3). 추후 정교화.
- OQ-3(프로세스 정의): MVP는 **패널 포그라운드 프로세스** 기준(FR-PROC-1).
- OQ-6(macOS 지속성): best-effort, 본 SRS MVP 제외(§12.2).

---

## 14. 변경 이력 (Change Log)
| 버전 | 날짜 | 내용 |
|---|---|---|
| 0.1 | 2026-06-08 | 초안. 옵션 2(tmux control mode)로 아키텍처 고정. 사용자 요구 8항목(좌측 파일목록+크기, 하단 Detach 바, 우측 세션명 모달, 세로 창 탭, 마우스 패널 리사이즈 등) 반영한 기능/비기능 요구사항·UI·상태/데이터 모델·이벤트 흐름·오류·인수기준·MVP·추적성 정의. |
