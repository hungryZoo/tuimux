# tuimux SDD (Software Design Description)

- **문서 버전**: 0.9
- **작성일**: 2026-06-12
- **상태**: PTY 기반 터미널 surface 설계
- **관련 요구사항**: [docs/srs.md](./srs.md)
- **참고 구현**: [hungryZoo/tscode](https://github.com/hungryZoo/tscode)의 `portable-pty` + `vt100` terminal pane

---

## 1. 설계 목표

tuimux의 가장 큰 문제는 main pane이 실제 터미널이 아니라 `tmux capture-pane` 결과를 주기적으로 그리는 “터미널처럼 보이는 preview”였다는 점이다. 이 구조는 `clear`, full-screen app, cursor, resize, ANSI style에서 쉽게 깨진다.

새 설계의 목표는 다음과 같다.

- tuimux ratatui chrome/sidebar는 유지한다.
- main pane은 실제 PTY에서 실행되는 tmux client 화면을 렌더한다.
- output은 byte stream으로 받고 `vt100::Parser`가 screen cell state를 유지한다.
- input은 `send-keys` 명령이 아니라 PTY writer에 terminal byte sequence로 전달한다.
- session/window 조작은 기존처럼 tmux command backend를 사용한다.

---

## 2. 아키텍처 개요

```text
┌──────────────────────────────────────────────────────────────┐
│ host terminal                                                 │
│  └─ tuimux process                                            │
│     ├─ ratatui/crossterm UI loop                              │
│     │  ├─ main pane renderer                                  │
│     │  ├─ right sidebar/session dialog                        │
│     │  └─ keyboard/mouse router                               │
│     ├─ tmux command backend                                   │
│     │  ├─ list-sessions/list-windows                          │
│     │  ├─ new-session/new-window/select-window/kill-window    │
│     │  └─ native fallback                                     │
│     └─ embedded terminal                                      │
│        ├─ portable-pty master/slave                           │
│        ├─ child: env -u TMUX tmux -u attach-session -t <s>     │
│        ├─ reader thread -> mpsc<Vec<u8>>                      │
│        ├─ vt100::Parser screen model                          │
│        └─ PTY writer for key/paste/mouse bytes                 │
└──────────────────────────────────────────────────────────────┘
```

---

## 3. 주요 컴포넌트

### 3.1 `src/terminal.rs`

새로운 terminal surface의 핵심 모듈이다.

- `TmuxTerminal`
  - `portable_pty::native_pty_system().openpty(...)`로 PTY pair를 만든다.
  - slave에서 `tmux -u attach-session -t <session>`을 실행한다.
  - Unix에서는 `env -u TMUX`로 부모의 `TMUX` 환경변수를 제거해 nested tmux 경고/오동작을 줄인다.
  - master reader를 별도 thread에서 읽고 `mpsc::Receiver<Vec<u8>>`로 UI loop에 전달한다.
  - `vt100::Parser`가 output byte stream을 terminal screen cell로 유지한다.

- `TerminalSpan`, `TerminalStyle`, `TerminalColor`
  - `vt100::Cell`의 text/style/color를 ratatui가 소비하기 쉬운 중립 구조로 변환한다.
  - UI layer가 `vt100` crate 세부 타입에 직접 의존하지 않게 한다.

- 입력 인코딩
  - 일반 문자, Enter, Backspace, Tab, Esc, arrows, Home/End, PageUp/PageDown, Insert/Delete, F1-F12를 terminal byte sequence로 변환한다.
  - Ctrl 문자와 일부 control punctuation을 ASCII control byte로 변환한다.
  - Alt 조합은 ESC prefix를 붙인다.
  - bracketed paste가 켜져 있으면 paste payload를 bracketed paste sequence로 감싼다.

- mouse encoding
  - child terminal이 mouse protocol을 켠 경우 SGR/default/UTF-8 mouse encoding을 사용해 event를 전달한다.

### 3.2 `src/tui.rs`

ratatui shell과 user interaction을 담당한다.

- `UiState`
  - tmux session/window metadata를 보관한다.
  - 현재 session에 연결된 `Option<TmuxTerminal>`을 보관한다.
  - session이 바뀌면 기존 embedded terminal을 drop하고 새 session으로 다시 attach한다.

- event loop
  - 매 frame 전 `TmuxTerminal::drain()`으로 PTY output을 parser에 반영한다.
  - main pane inner rect가 바뀌면 PTY size와 parser size를 함께 resize한다.
  - tmux metadata는 `capture-pane`처럼 빠르게 polling하지 않고, mutation 이후 또는 느슨한 interval로 갱신한다.

- renderer
  - `TmuxTerminal::styled_rows()`를 ratatui `Line/Span`으로 변환한다.
  - cursor가 hidden이 아니고 session modal이 닫혀 있으면 parser cursor 좌표를 `Frame::set_cursor`에 반영한다.
  - main pane border는 terminal input mode일 때 green, navigation mode일 때 dark gray로 표시한다.

### 3.3 `src/tmux.rs`

tmux server 상태와 mutation command를 담당한다.

- 유지되는 책임
  - tmux binary/version probe.
  - session/window list parsing.
  - session/window create/select/kill.
  - native fallback 실행.

- 축소된 책임
- TUI main pane 렌더링을 위한 `capture-pane`, `resize-window`, `send-keys` runtime path는 제거되었다.

---

## 4. 데이터 흐름

### 4.1 Output path

```text
tmux client inside PTY
  -> PTY master reader thread
  -> mpsc channel
  -> UiState::sync_terminal()
  -> TmuxTerminal::drain()
  -> vt100::Parser::process(bytes)
  -> TmuxTerminal::styled_rows()
  -> ratatui Paragraph/Span render
```

이 경로는 screen state를 누적 유지한다. 따라서 `clear`처럼 “기존 글자를 지우는” escape sequence도 parser가 cell state에 반영한다.

### 4.2 Input path

```text
crossterm Event::Key / Event::Paste / Event::Mouse
  -> tui event router
  -> terminal input mode 여부 확인
  -> key/paste/mouse byte encoding
  -> PTY writer
  -> tmux client
  -> active tmux pane/shell
```

terminal input mode에서 Ctrl-C는 tuimux 종료가 아니라 shell/tmux client로 전달된다. navigation mode에서는 기존처럼 Ctrl-C, q, Esc가 tuimux 종료 shortcut이다.

### 4.3 Session switch path

```text
session dialog row click
  -> state.current_session = selected
  -> drop old TmuxTerminal
  -> next sync_terminal()
  -> spawn new PTY tmux client attached to selected session
```

이 방식은 tuimux가 tmux 안에서 실행 중이어도 바깥 tmux client를 `switch-client`하지 않는다. 사용자가 보고 조작하는 것은 tuimux 내부 main pane이다.

---

## 5. Resize 설계

ratatui layout은 main pane의 border를 포함한 `Rect`를 만든다. 실제 terminal surface는 border 안쪽 `inner Rect`만 사용한다.

- renderer가 `regions.terminal_body`에 inner rect를 기록한다.
- 다음 loop에서 `sync_terminal(width, height)`가 이 값을 읽는다.
- `TmuxTerminal::resize(width, height)`는 다음을 함께 수행한다.
  - `vt100::Parser::screen_mut().set_size(rows, cols)`
  - `portable_pty::MasterPty::resize(PtySize { rows, cols, ... })`

이 구조는 tmux client가 실제 PTY resize를 받게 하므로 wrapping, full-screen app redraw, prompt 위치가 기존 `resize-window` + `capture-pane` 조합보다 자연스럽다.

---

## 6. Error/cleanup 설계

- stdout이 TTY가 아니면 interactive UI를 시작하지 않는다.
- setup은 raw mode, alternate screen, mouse capture를 켠다.
- restore는 raw mode 해제, alternate screen leave, mouse capture 해제, cursor 표시를 수행한다.
- `TmuxTerminal`은 drop 시 child tmux client를 kill한다. tmux server/session은 종료하지 않는다.
- embedded terminal spawn 실패 시 main pane에 error message를 표시하고 tuimux UI는 계속 유지한다.

---

## 7. 왜 control-mode가 아닌가

장기적으로 가장 정교한 방향은 `tmux -CC` control-mode client다. control-mode는 pane output event, layout event, window/session event를 protocol 단위로 받을 수 있다.

이번 설계는 다음 이유로 PTY embedded tmux client를 먼저 선택한다.

- 사용자가 지적한 핵심 문제는 “가짜 터미널처럼 보임”이며, PTY + VT parser가 이 문제를 즉시 줄인다.
- `hungryZoo/tscode`에서 검증된 구조와 동일한 기반을 재사용할 수 있다.
- 현재 tuimux의 sidebar/session/window command 구조를 크게 흔들지 않는다.
- tmux option을 영구 변경하지 않고도 native tmux rendering fidelity를 얻는다.

단, control-mode 전환을 막지 않도록 `src/terminal.rs`를 독립 모듈로 두었다. 향후 `ControlModeTerminal`을 같은 interface로 추가하면 renderer와 input router를 크게 바꾸지 않고 교체할 수 있다.

---

## 8. 테스트 전략

### 현재 자동화

- tmux version parser.
- tmux command argument builder.
- run mode selection.
- layout preview.
- hit testing.
- terminal key encoding.
- terminal mouse encoding.

### 추가 권장 테스트

- PTY smoke test: `tuimux` 실행 후 `Session`, `Detach`, `WINDOWS`와 shell prompt가 함께 보이는지 확인.
- clear test: `printf 'AAAAA\nBBBBB\n'; clear` 후 이전 output이 main pane에 없는지 확인.
- color test: 16색/256색/truecolor sample 출력.
- resize test: PTY 크기 변경 후 border/sidebar/terminal wrapping 확인.
- fullscreen test: `less`, `top`, `vim` 진입/종료 확인.

---

## 9. 알려진 trade-off

- embedded tmux client는 tmux statusline까지 그릴 수 있다. v0.9에서는 사용자의 tmux option을 강제로 바꾸지 않는 쪽을 선택했다.
- main pane이 “active pane만”이 아니라 tmux client 화면 전체를 보여준다. 이는 terminal fidelity를 우선한 선택이다.
- control-mode보다 session/window event 정밀도는 낮다. metadata는 command polling과 mutation 이후 refresh로 보정한다.
- snapshot 기반 terminal emulation test는 제거되었고, terminal key/mouse encoding test는 `src/terminal.rs`가 담당한다.
