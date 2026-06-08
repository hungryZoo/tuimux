# tuimux PRD (Product Requirements Document)

- **문서 버전**: 0.5 (Draft)
- **작성일**: 2026-06-08
- **상태**: 스켈레톤 구현/UX 검증용 요구사항
- **프로젝트명**: tuimux
- **한 줄 요약**: tmux를 백엔드로 쓰되, prefix 키 없이 마우스와 단순한 TUI 버튼으로 세션·창·패널을 다루는 Rust 기반 TUI 래퍼.

---

## 1. 개요

tuimux는 tmux의 핵심 가치(세션 지속성, detach/attach, 여러 창/패널 관리)는 유지하되, tmux prefix 키와 명령 암기 부담을 줄이는 TUI 프론트엔드다.

v0.5 UX 방향은 “기능을 많이 보이는 화면”이 아니라 **중앙의 tmux 작업 영역 + 우측의 최소 조작 영역**이다. 이전 초안의 좌측 파일 탐색기, 하단 메뉴 바, 우측 PROCS 패널은 MVP에서 제외한다.

핵심 차별점:

1. **No prefix-key**: `Ctrl-b`류 prefix 없이 마우스/단일 단축키로 조작한다.
2. **단순한 화면**: 중앙은 tmux pane, 우측은 Session 버튼·Detach 버튼·창 탭만 둔다.
3. **Session 버튼 중심 UX**: 우측 상단 버튼은 세션명이 아니라 `Session`으로 라벨링하고, 버튼 내부에 현재 세션명을 보여준다. 클릭하면 중앙에 헤더 없는 dialog 형태의 세션 선택/Detach 창이 뜬다.
4. **마우스 반응성**: 모든 버튼/탭/모달 항목은 hover 시 색상·강조 등 즉각적인 반응을 보여야 한다.
5. **tmux 백엔드 우선**: 초기 구현은 tmux control mode로 UX를 검증한다.

---

## 2. 목표

- **G1. 학습 비용 최소화**: 사용자가 화면의 버튼과 탭만 보고 세션 전환, 창 전환, detach를 이해한다.
- **G2. 작업 영역 우선**: 화면 대부분을 tmux pane에 할당하고 보조 UI는 우측에 압축한다.
- **G3. 지속성**: detach/SSH 끊김 후에도 tmux 세션과 프로세스가 유지된다.
- **G4. 마우스 우선**: 버튼 hover, 클릭, 탭 클릭이 TUI 안에서 자연스럽게 동작한다.
- **G5. 스켈레톤 우선 검증**: 실제 control-mode 구현 전에도 레이아웃·설치·릴리즈·기본 TUI shell을 검증 가능해야 한다.

### 성공 지표

- 신규 사용자가 문서 없이 Session 버튼과 Detach 버튼의 의미를 이해한다.
- `tuimux --layout-preview`와 기본 TUI 실행 화면이 v0.5 레이아웃을 일관되게 보여준다.
- hover 가능한 모든 UI 요소가 시각적으로 반응한다.
- prerelease installer로 macOS에서 설치/실행 검증 가능하다.

---

## 3. 비목표

- **NG1. 좌측 파일 탐색기**: MVP에서 제외한다. 파일 목록/크기/클릭 동작은 후순위다.
- **NG2. 하단 메뉴 바**: MVP에서 제외한다. Detach는 우측 세션 영역과 세션 dialog 안에 둔다.
- **NG3. PROCS 패널**: MVP에서 제외한다. 프로세스 가시성은 추후 overlay 또는 별도 view로 재검토한다.
- **NG4. 드롭다운 장식**: 세션명 옆 `session:` prefix나 `▾` glyph는 쓰지 않는다.
- **NG5. tmux 설정/플러그인 호환성**: `.tmux.conf`, tpm 등과의 완전 호환은 목표가 아니다.
- **NG6. 자체 terminal multiplexer 엔진**: 초기 MVP에서는 직접 PTY/VT 엔진을 만들지 않는다.

---

## 4. 사용자 페르소나

### P1. 원격 서버 개발자
- SSH로 서버에 접속해 빌드/학습/서버를 돌린다.
- tmux를 쓰지만 prefix 키를 자주 잊는다.
- **니즈**: 보이는 버튼으로 detach/attach를 안전하게 수행.

### P2. 터미널 입문자
- GUI처럼 보이는 세션/창 전환을 원한다.
- **니즈**: Session 버튼, 창 탭, hover 반응으로 “클릭 가능한 곳”을 직관적으로 파악.

### P3. 도구에 민감한 시니어
- 화면을 낭비하는 보조 패널을 싫어한다.
- **니즈**: 작업 pane을 넓게 유지하고, 조작 UI는 최소화.

---

## 5. MVP 요구사항

### 5.1 레이아웃

```
┌────────────────────────────────────────────────────────────────────┐
├──────────────────────────────────────────────────────┬─────────────┤
│                                                      │ ┌─Session─┐ │
│                                                      │ │   dev   │ │ ← Session 버튼
│       실제 tmux pane 출력/입력                       │ └─────────┘ │
│                                                      │ ┌─────────┐ │
│       pane 0 / pane 1 / pane 2                       │ │ Detach  │ │ ← title/header 없는 빨간 버튼
│                                                      │ └─────────┘ │
│                                                      │ WINDOWS     │
│                                                      │ ▸ 1 build  ✕│
│                                                      │   2 logs   ✕│
│                                                      │   3 ssh    ✕│
│                                                      │   + new     │
└──────────────────────────────────────────────────────┴─────────────┘
```

- 중앙: 현재 tmux window의 pane 영역.
- 우측 상단: 테두리 title이 `Session`인 버튼. 버튼 내부에는 현재 세션명(예: `dev`)을 표시한다.
- 세션명 앞에 `session:` prefix를 붙이지 않고, 드롭다운 glyph도 쓰지 않는다.
- Session 버튼 아래: title/header 없이 글자 `Detach`만 중앙에 보이는 빨간 Detach 버튼.
- 그 아래: 현재 세션의 window 세로 탭.
- 좌측 파일 탐색기 없음.
- 하단 메뉴 바 없음.
- PROCS 영역 없음.

### 5.2 세션 dialog

Session 버튼 클릭 시 중앙에 dialog처럼 뜬다. dialog 외곽선에는 `Session picker` 같은 title/header를 넣지 않는다.

```
        ┌────────────────────────────────────────┐
        │  ● dev        3 windows                │
        │    work       2 windows                │
        │    scratch    1 window                 │
        │                                        │
        │     [ New Session ]     [ Detach ]     │
        └────────────────────────────────────────┘
```

요구사항:

- dialog에는 별도 헤더/제목 줄을 두지 않는다.
- 현재 세션은 강조 표시한다.
- 세션 항목 클릭 시 해당 세션으로 전환한다.
- dialog 내부 하단에 `New Session` 버튼과 빨간 Detach 버튼을 둔다.
- `Esc` 또는 바깥 클릭으로 닫는다.
- dialog가 떠도 기본 화면의 레이아웃은 유지되고, overlay만 중앙에 올라온다.

### 5.3 마우스/hover

- Session 버튼 hover: 강조 색상 변경.
- Detach 버튼 hover: 빨간색 배경/테두리 등으로 강조. 단, 버튼 테두리 title/header는 표시하지 않는다.
- window 탭 hover: 선택 가능한 행임을 표시. 각 window 행 오른쪽 `✕`는 경고 없이 해당 window를 닫는다.
- `+ new` hover: 버튼처럼 강조.
- dialog 세션 항목 hover: 행 하이라이트.
- dialog Detach hover: 빨간 버튼 강조.

### 5.4 기능 범위

v0.5에 포함:

- `--help`, `--version`, `--doctor`.
- `--layout-preview`에서 v0.5 레이아웃 출력.
- 기본 TUI에서 v0.5 레이아웃 렌더.
- header 없는 compact TUI와 session dialog.
- mouse hover 상태 반영.
- 실제 tmux session/window 조회, 전환, 생성.
- window 행 오른쪽 `✕` 클릭 시 경고 없이 `kill-window`.
- main area의 실제 tmux pane 출력(`capture-pane`) 및 클릭 후 키 입력 전달(`send-keys`, F12로 navigation 복귀).
- `q`/`Esc` 종료, `d` detach command.
- installer/release pipeline.

아직 제외:

- full `tmux -CC` streaming renderer.
- multi-pane split/resize 조작 UI.
- ANSI 색상/커서/alternate-screen 앱의 완전한 렌더링.

---

## 6. 아키텍처 결정

- **TD-1 언어**: Rust.
- **TD-2 TUI**: `ratatui` + `crossterm`.
- **TD-3 백엔드**: v0.5는 tmux CLI 명령(`list-*`, `capture-pane`, `send-keys`) 기반. 장기적으로 full control mode streaming.
- **TD-4 입력**: 마우스 이벤트는 UI 영역 hit-test 후 처리. pane 영역 클릭 시 terminal mode로 들어가 키 입력을 tmux에 전달하고 F12로 navigation mode에 복귀.
- **TD-5 UX 원칙**: 작업 영역을 넓게, 보조 UI를 우측으로 최소화.

---

## 7. 마일스톤

- **M0 — 스켈레톤**: 설치, preview, doctor, compact layout, hover/dialog.
- **M1 — tmux CLI 연결**: tmux server attach, session/window 목록 읽기.
- **M2 — window/session 전환**: UI 클릭 → tmux command 송신.
- **M3 — pane 출력/입력**: `capture-pane` 렌더링, `send-keys` passthrough.
- **M4 — pane 조작**: split/new/close/resize.
- **M5 — polish**: 도움말, palette, 접근성, 성능.

---

## 8. 변경 이력

- **0.5 / 2026-06-08**: top header row 제거, dialog New Session action 추가, window별 `✕` kill 버튼 추가, main area를 `capture-pane`/`send-keys` 기반 실제 tmux pane interaction으로 전환.
- **0.4 / 2026-06-08**: 실제 tmux CLI 기반 session/window 조회, window 선택/생성, session 선택/생성, best-effort detach를 스켈레톤 TUI에 연결. 중앙 pane 렌더링은 여전히 control-mode 다음 단계.
- **0.3 / 2026-06-08**: 우측 상단 버튼 title을 현재 세션명(`dev`)이 아니라 `Session`으로 변경. Detach 버튼 테두리 title 제거. 세션 dialog의 `Session picker`/`Sessions` 헤더 제거.
- **0.2 / 2026-06-08**: 좌측 파일 탐색기, 하단 메뉴 바, PROCS 제거. 우측 세션명 버튼, 빨간 Detach 버튼, 중앙 세션 dialog, hover 반응 요구사항 반영.
- **0.1 / 2026-06-08**: 초기 초안.
