# tuimux SRS (Software Requirements Specification)

- **문서 버전**: 0.4 (Draft)
- **작성일**: 2026-06-08
- **상태**: v0.4 스켈레톤 구현 대상 명세
- **프로젝트명**: tuimux
- **상위 문서**: [docs/prd.md](./prd.md) (PRD v0.4)
- **한 줄 요약**: tmux control mode를 백엔드로 하는 Rust TUI 래퍼. v0.4는 좌측/하단/PROCS를 제거하고, 우측 Session 버튼 + 제목 없는 Detach 버튼 + 창 탭 + 헤더 없는 중앙 session dialog를 검증한다.

---

## 1. 범위

본 SRS는 v0.4 스켈레톤의 구현 가능한 요구사항을 정의한다.

### 포함

- `ratatui` 기반 v0.4 레이아웃.
- `crossterm` mouse event 기반 hover/click scaffold.
- 우측 `Session` 버튼(테두리 title은 `Session`, 버튼 내부에는 현재 세션명 표시).
- 우측 빨간 Detach 버튼(테두리 title/header 없음).
- 우측 window 세로 탭.
- 중앙 session dialog(테두리 title/header 없음).
- `--layout-preview` static mock.
- `--doctor` 환경 체크.
- macOS prerelease packaging/installer.

### 제외

- 실제 tmux control-mode client.
- 실제 session/window/pane 상태 동기화.
- 좌측 파일 탐색기.
- 하단 메뉴 바.
- PROCS panel.
- 세션명 `session:` prefix와 `▾` 드롭다운 glyph.

---

## 2. 용어

- **Main area**: tmux pane들이 렌더될 중앙 영역. v0.4는 mock text.
- **Session button**: 우측 상단 버튼. 버튼 테두리 title은 `Session`, 버튼 내부 텍스트는 현재 세션명(예: `dev`)이다.
- **Detach button**: session button 바로 아래의 빨간 버튼. 테두리 title/header 없이 버튼 내부에 `Detach`만 표시한다.
- **Window tabs**: 우측의 세로 window 목록.
- **Session dialog**: session button 클릭 시 중앙에 뜨는 overlay dialog.
- **Hover**: mouse cursor가 버튼/탭/항목 위에 있을 때 색상·배경·테두리가 변하는 상태.

---

## 3. 기능 요구사항

### 3.1 CLI / 실행

- **FR-CLI-1 [M]** `tuimux --help`는 사용 가능한 옵션을 표시한다.
- **FR-CLI-2 [M]** `tuimux --version`은 패키지 버전을 표시한다.
- **FR-CLI-3 [M]** `tuimux --doctor`는 tmux 설치/버전과 터미널 환경을 점검한다.
- **FR-CLI-4 [M]** `tuimux --layout-preview`는 v0.4 static layout을 출력한다.
- **FR-CLI-5 [M]** 인자 없이 실행하면 interactive TUI shell을 연다. stdout이 TTY가 아니면 안전하게 거부한다.

### 3.2 레이아웃

- **FR-LAYOUT-1 [M]** 화면은 `status line + body`로 구성한다. 하단 메뉴 바는 없어야 한다.
- **FR-LAYOUT-2 [M]** body는 `main area + right sidebar` 두 영역만 가진다. 좌측 파일 탐색기는 없어야 한다.
- **FR-LAYOUT-3 [M]** main area는 mock tmux pane 내용을 표시한다.
- **FR-LAYOUT-4 [M]** right sidebar는 위에서부터 session button, Detach button, window tabs 순서로 배치한다.
- **FR-LAYOUT-5 [M]** PROCS 영역은 렌더하지 않는다.

### 3.3 Session button

- **FR-SESS-1 [M]** 우측 상단 버튼의 테두리 title은 현재 세션명(`dev`)이 아니라 고정 라벨 `Session`이다.
- **FR-SESS-2 [M]** 버튼 내부 중앙 텍스트는 현재 세션명이다. 예: `dev`.
- **FR-SESS-3 [M]** 세션명 앞에 `session:` prefix를 붙이지 않는다.
- **FR-SESS-4 [M]** 세션명 뒤에 `▾` 등 드롭다운 glyph를 붙이지 않는다.
- **FR-SESS-5 [M]** Session 영역은 버튼처럼 보여야 한다: border, 강조색, hover 반응을 가진다.
- **FR-SESS-6 [M]** session button 클릭 또는 `Alt-s`는 session dialog를 연다/닫는다.

### 3.4 Detach button

- **FR-DETACH-1 [M]** right sidebar에서 session button 바로 아래에 Detach button을 둔다.
- **FR-DETACH-2 [M]** Detach button은 빨간색 계열로 표시한다.
- **FR-DETACH-3 [M]** Detach button의 테두리에는 `Detach` title/header를 표시하지 않는다. 글자 `Detach`는 버튼 내부 중앙에만 표시한다.
- **FR-DETACH-4 [M]** Detach button hover 시 버튼 배경/테두리를 강조한다.
- **FR-DETACH-5 [M]** Detach button 클릭 또는 `d`는 v0.4 scaffold에서 detach exit path로 종료한다.
- **FR-DETACH-6 [M]** `d` 또는 Detach click은 tmux 내부 실행 시 `detach-client`를 송신하고 종료한다. tmux 외부 실행 시에는 송신할 client가 없으므로 안전하게 UI만 종료한다.

### 3.5 Window tabs

- **FR-WIN-1 [M]** right sidebar의 Detach button 아래에 window 세로 탭 목록을 표시한다.
- **FR-WIN-2 [M]** 활성 window는 `▸`와 배경색 등으로 강조한다.
- **FR-WIN-3 [M]** 각 window 탭은 hover 시 하이라이트된다.
- **FR-WIN-4 [M]** `+ new` 항목을 표시하고 hover 시 버튼처럼 반응한다.
- **FR-WIN-5 [M]** window 탭 클릭 시 `select-window`, `+ new` 클릭 시 `new-window` 명령을 tmux로 보낸다.

### 3.6 Session dialog

- **FR-DIALOG-1 [M]** session button 클릭 시 중앙에 modal/dialog overlay를 렌더한다.
- **FR-DIALOG-2 [M]** dialog 외곽선에는 `Session picker` 같은 title을 표시하지 않는다.
- **FR-DIALOG-3 [M]** dialog 내부에는 `Sessions` 같은 별도 헤더 행을 표시하지 않는다.
- **FR-DIALOG-4 [M]** dialog는 세션 목록을 바로 표시한다: 이름, window 수, active 표시.
- **FR-DIALOG-5 [M]** 현재 세션은 `●` 또는 색상으로 강조한다.
- **FR-DIALOG-6 [M]** dialog 하단에 빨간 Detach button을 둔다.
- **FR-DIALOG-7 [M]** dialog Detach button도 테두리 title/header 없이 버튼 내부 `Detach` 텍스트만 표시한다.
- **FR-DIALOG-8 [M]** 세션 항목 hover 시 행이 하이라이트된다.
- **FR-DIALOG-9 [M]** dialog Detach button hover 시 빨간 버튼 강조가 적용된다.
- **FR-DIALOG-10 [M]** `Esc`는 dialog가 열려 있으면 dialog를 닫고, 닫혀 있으면 TUI를 종료한다.
- **FR-DIALOG-11 [M]** 세션 항목 클릭 시 tmux 내부에서는 `switch-client -t <session>`을 송신한다. tmux 외부에서는 interactive `attach-session`을 실행하지 않고 TUI의 현재 target session만 전환해 window 목록/명령 target을 갱신한다.
- **FR-DIALOG-12 [M]** dialog가 열려 있을 때 `n` 키는 `tuimux-<n>` 이름의 detached session을 생성하고 live state를 refresh한다.

### 3.7 Mouse / hover routing

- **FR-MOUSE-1 [M]** TUI는 mouse capture를 활성화한다.
- **FR-MOUSE-2 [M]** session button, Detach button, window tab, `+ new`, dialog session row, dialog Detach button에 hit-test 영역을 둔다.
- **FR-MOUSE-3 [M]** mouse move/down 이벤트마다 현재 hover target을 계산한다.
- **FR-MOUSE-4 [M]** hover target에 맞춰 색상/배경/테두리를 변경한다.
- **FR-MOUSE-5 [M]** modal이 열려 있을 때 modal 항목이 sidebar보다 우선 hit-test된다.

---

## 4. UI 레이아웃 명세

```
┌────────────────────────────────────────────────────────────────────┐
│ tuimux  dev · tmux 3.x · scaffold preview                          │
├──────────────────────────────────────────────────────┬─────────────┤
│ MAIN AREA (tmux panes — mock)                        │ [Session]   │
│ ─────────────────────────────────────────────        │    dev      │
│ pane 0 (focus)            pane 1                     │ [Detach]    │
│ $ cargo build             $ htop                     │ WINDOWS     │
│ Compiling tuimux…         tasks: 142                 │ ▸ 1: build  │
│ ────────────────(drag border ↔ to resize)────────    │   2: logs   │
│ pane 2                                               │   3: ssh    │
│ $ tail -f app.log                                    │   + new     │
└──────────────────────────────────────────────────────┴─────────────┘
```

Dialog overlay:

```
        ┌────────────────────────────────────────┐
        │  ● dev        3 windows                │
        │    work       2 windows                │
        │    scratch    1 window                 │
        │                                        │
        │              [ Detach ]                │
        └────────────────────────────────────────┘
```

---

## 5. 상태 모델

```
Start
 └─> AttachedScaffold
      ├─ session button / Alt-s -> SessionDialogOpen
      ├─ d / Detach click       -> DetachedExit
      ├─ q                      -> QuitExit
      └─ mouse move             -> Hover(target)

SessionDialogOpen
 ├─ Esc / session button        -> AttachedScaffold
 ├─ session row click           -> AttachedScaffold  (inside tmux: switch-client, outside tmux: local target select)
 ├─ n                           -> create detached tuimux-<n> session
 ├─ Detach click / d            -> DetachedExit
 └─ mouse move                  -> Hover(modal target)
```

---

## 6. 비기능 요구사항

- **NFR-1** 유휴 CPU는 이벤트 polling 기반으로 낮게 유지한다.
- **NFR-2** TUI 종료 시 raw mode, alternate screen, mouse capture를 복구한다.
- **NFR-3** stdout이 TTY가 아니면 interactive UI를 시작하지 않는다.
- **NFR-4** 작은 터미널에서도 panic 없이 최소 레이아웃을 유지한다.
- **NFR-5** macOS prerelease artifact는 installer로 설치 가능해야 한다.

---

## 7. 인수 기준

- **AC-1** `cargo test`가 통과한다.
- **AC-2** `cargo run -- --layout-preview` 출력에 `[ Session ]`, `[ Detach ]`, `WINDOWS`, `● dev`가 있고, dialog에는 `Session picker`/`Sessions` 헤더가 없다.
- **AC-3** preview 출력에 `EXPLORER`, `PROCS`, `Detach Alt-d`, `session:`, `▾`가 없다.
- **AC-4** interactive TUI는 session button, Detach button, window tabs, dialog를 렌더한다.
- **AC-5** mouse hover target에 따라 스타일이 바뀌는 코드 경로가 있다.
- **AC-6** macOS release asset과 `SHA256SUMS`가 GitHub prerelease에 업로드된다.
- **AC-7** raw one-line installer가 최신 prerelease를 설치한다.

---

## 8. 추적성

- PRD NG1/NG2/NG3 → SRS FR-LAYOUT-1/2/5.
- PRD 세션 버튼 UX → SRS FR-SESS, FR-DIALOG.
- PRD 빨간 Detach 요구 → SRS FR-DETACH, FR-DIALOG-6/7/9.
- PRD hover 요구 → SRS FR-MOUSE.
- PRD 스켈레톤 검증 → SRS FR-CLI, AC-1~AC-7.

---

## 9. 변경 이력

- **0.4 / 2026-06-08**: `list-sessions`/`list-windows` 기반 live state, `select-window`/`new-window`/`new-session`/`detach-client` command dispatch 요구사항을 스켈레톤에 반영. control-mode pane streaming은 다음 단계로 유지.
- **0.3 / 2026-06-08**: Session 버튼 title을 `Session`으로 고정하고 내부에 현재 세션명을 표시. Detach 버튼 및 dialog Detach의 테두리 title 제거. dialog의 `Session picker`/`Sessions` 헤더 제거.
- **0.2 / 2026-06-08**: 좌측 파일 탐색기, 하단 메뉴 바, PROCS 제거. 세션명 버튼/빨간 Detach/session dialog/hover 명세 추가.
- **0.1 / 2026-06-08**: 초기 control-mode 래퍼 SRS.
