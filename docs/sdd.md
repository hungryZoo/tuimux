# tuimux SDD

- **문서 버전**: 1.0
- **대상 릴리스**: v0.2.0-alpha.5
- **작성일**: 2026-06-13
- **상태**: Rust-native in-process multiplexer 설계

## 1. 설계 목표

기존 tuimux의 가장 큰 문제는 main pane이 실제 terminal이 아니라 tmux output을 간접적으로 보여주는 느낌을 준다는 점이었다. v0.2.0-alpha.5는 default path에서 tmux를 제거하고, tuimux가 직접 PTY child process와 terminal screen model을 소유한다.

핵심 목표는 다음과 같다.

- shell/editor/monitor가 실제 PTY 안에서 실행된다.
- main terminal mode는 host terminal 전체 크기를 child PTY에 제공한다.
- selection과 clipboard는 host terminal에 가까운 감각으로 동작한다.
- tmux C 코드는 구조 참고로만 삼고 Rust 모듈로 직접 구현한다.
- tmux fallback은 hidden `--native-client` 옵션에만 남긴다.

## 2. 전체 구조

```text
┌─────────────────────────────────────────────────────────────────┐
│                         tuimux process                          │
├─────────────────────────────────────────────────────────────────┤
│ src/main.rs                                                     │
│   ├─ CLI parse / doctor / layout-preview / tmux fallback         │
│   └─ tui::run(initial_session, cwd)                              │
├─────────────────────────────────────────────────────────────────┤
│ src/tui.rs                                                      │
│   ├─ ratatui frame rendering                                    │
│   ├─ terminal mode vs navigation mode                            │
│   ├─ mouse hit testing / selection state                         │
│   ├─ Ctrl-C clipboard interception                               │
│   └─ NativeMux command dispatch                                  │
├─────────────────────────────────────────────────────────────────┤
│ src/native_mux.rs                                               │
│   ├─ sessions: Vec<NativeSession>                                │
│   ├─ windows: Vec<NativeWindow>                                  │
│   └─ each NativeWindow owns PtyTerminal                          │
├─────────────────────────────────────────────────────────────────┤
│ src/terminal.rs                                                 │
│   ├─ portable-pty master/slave                                   │
│   ├─ child shell process                                         │
│   ├─ reader thread -> mpsc channel                               │
│   ├─ vt100::Parser screen model                                  │
│   └─ key/mouse/paste encoding                                    │
├─────────────────────────────────────────────────────────────────┤
│ src/clipboard.rs                                                │
│   └─ pbcopy / wl-copy / xclip / xsel / clip adapters             │
└─────────────────────────────────────────────────────────────────┘
```

## 3. 모듈 설계

### 3.1 `src/main.rs`

`main.rs`는 CLI boundary다.

- `tuimux` 기본 실행은 `tui::run()`으로 들어간다.
- `--doctor`는 native runtime 진단을 출력한다.
- `--layout-preview`는 정적 preview를 출력한다.
- `--native-client`가 있을 때만 `tmux::probe()`와 tmux fallback을 사용한다.

tmux가 설치되어 있지 않아도 기본 `tuimux` 실행과 `--doctor`는 성공할 수 있어야 한다.

### 3.2 `src/native_mux.rs`

`NativeMux`는 현재 알파의 multiplexer core다.

```rust
pub struct NativeMux {
    sessions: Vec<NativeSession>,
    active_session: usize,
    cwd: PathBuf,
    next_session: u32,
    next_window_id: u32,
}
```

각 `NativeSession`은 window list와 active window index를 가진다. 각 `NativeWindow`는 index/name과 `PtyTerminal`을 가진다.

주요 동작:

- `NativeMux::new()`는 초기 session과 첫 shell window를 생성한다.
- `create_next_session()`은 `tuimux-<n>` 이름으로 새 session을 만든다.
- `new_window()`는 active session에 새 shell PTY를 추가한다.
- `select_window_by_row()`와 `switch_session_by_row()`는 외부 process 없이 active index만 바꾼다.
- `kill_window_by_row()`는 마지막 window가 제거될 때 replacement shell을 만들어 빈 session을 방지한다.
- `drain_all()`은 모든 window의 pending PTY output을 parser에 반영한다.
- `resize_active()`는 active terminal만 host layout 크기에 맞춘다.

현재 설계는 in-process다. 따라서 tuimux process가 끝나면 child PTY들도 종료된다.

### 3.3 `src/terminal.rs`

`PtyTerminal`은 terminal fidelity의 중심이다.

```rust
pub struct PtyTerminal {
    parser: vt100::Parser,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    rx: Receiver<Vec<u8>>,
    rows: u16,
    cols: u16,
}
```

생성 흐름:

1. `portable_pty::native_pty_system().openpty()`로 PTY pair를 만든다.
2. slave에서 `$SHELL`을 실행한다.
3. child 환경에 `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=tuimux`를 설정한다.
4. master reader는 background thread에서 읽고 `mpsc` channel로 bytes를 보낸다.
5. UI loop는 `drain()`으로 bytes를 받아 `vt100::Parser::process()`에 넣는다.

렌더링:

- `styled_rows_with_selection(selection)`은 vt100 cell을 `TerminalSpan`으로 변환한다.
- fg/bg가 default일 때 ratatui default color를 그대로 사용해 host terminal color를 보존한다.
- selection cell은 inverse bit를 토글해 시각적으로 강조한다.
- wide continuation cell은 중복 렌더하지 않는다.

입력:

- `send_key()`는 crossterm `KeyEvent`를 xterm-compatible byte sequence로 변환한다.
- Ctrl 문자, Alt prefix, arrow/application cursor, function key, delete/page key를 처리한다.
- `send_paste()`는 bracketed paste mode를 감지해 paste wrapper를 적용한다.
- `send_mouse_event()`는 child가 mouse protocol을 활성화한 경우에만 SGR/normal mouse sequence를 전달한다.

### 3.4 `src/tui.rs`

`tui.rs`는 UI state, rendering, input routing을 담당한다.

```rust
struct UiState {
    session_modal_open: bool,
    hover: Option<Hotspot>,
    regions: Regions,
    mux: NativeMux,
    sessions: Vec<Session>,
    windows: Vec<Window>,
    current_session: String,
    status: Option<String>,
    terminal_error: Option<String>,
    terminal_mode: bool,
    selection: Option<SelectionState>,
}
```

두 가지 mode가 있다.

- **Terminal mode**: 첫 화면이자 기본 mode. terminal body가 전체 frame을 차지한다. 키 입력은 기본적으로 active PTY로 간다.
- **Navigation mode**: `F12`로 진입한다. main pane border와 right sidebar가 보이고 session/window 조작을 할 수 있다.

Terminal mode를 fullscreen으로 둔 이유는 full-screen 프로그램이 80x24 host에서 sidebar 때문에 52x22 같은 작은 PTY를 받지 않도록 하기 위해서다.

### 3.5 Selection

선택 state는 anchor/focus/dragging으로 구성된다.

```rust
struct SelectionState {
    anchor: (u16, u16),
    focus: (u16, u16),
    dragging: bool,
}
```

동작:

- child mouse protocol이 꺼져 있으면 left down/drag/up이 tuimux selection을 만든다.
- child mouse protocol이 켜져 있으면 normal mouse는 child로 전달한다.
- child mouse protocol이 켜져 있어도 Shift-left-drag는 tuimux selection을 만든다.
- mouse-up은 selection을 종료하지만 selection 자체를 지우지 않는다.
- selection이 zero-width이면 지운다.
- 새 key input이나 paste는 selection을 지운다. 단, Ctrl-C copy는 selection을 유지한다.

### 3.6 Ctrl-C 처리

terminal mode에서 Ctrl-C는 다음 순서로 처리된다.

1. `selection_range()`가 있는지 확인한다.
2. selection이 있으면 active terminal에서 `selected_text()`를 추출한다.
3. `clipboard::copy_text()`로 system clipboard에 복사한다.
4. 복사 성공 시 Ctrl-C byte를 PTY로 보내지 않는다.
5. selection이 없으면 일반 Ctrl-C byte를 PTY로 보낸다.

이 설계는 macOS 기본 Terminal처럼 “텍스트를 선택한 상태의 Ctrl-C는 복사”라는 동작을 우선한다.

### 3.7 Clipboard

별도 Rust clipboard crate를 추가하지 않고 platform command를 사용한다.

- macOS: `pbcopy`
- Windows: `clip`
- Linux: `wl-copy`, `xclip -selection clipboard`, `xsel --clipboard --input`

command가 없거나 실패하면 UI status에 error를 표시하고 panic하지 않는다.

## 4. 주요 흐름

### 4.1 시작

```text
main()
  -> parse CLI
  -> tui::run(session, cwd)
  -> UiState::bootstrap()
  -> NativeMux::new()
  -> PtyTerminal::new_shell()
  -> ratatui raw mode + alternate screen
```

### 4.2 렌더 루프

```text
loop
  -> state.sync_terminal(current terminal_body size)
  -> mux.resize_active(width, height)
  -> mux.drain_all()
  -> terminal.draw(ui)
  -> crossterm event poll/read
```

terminal mode에서 `terminal_body`는 root frame 전체다. navigation mode에서는 main pane과 sidebar layout으로 나뉜다.

### 4.3 Key Input

```text
crossterm KeyEvent
  -> F12이면 mode toggle
  -> terminal_mode이면 UiState::send_terminal_key()
     -> Ctrl-C + selection이면 clipboard copy
     -> 아니면 PtyTerminal::send_key()
  -> navigation mode이면 sidebar/session shortcut 처리
```

### 4.4 Mouse Input

```text
crossterm MouseEvent
  -> terminal cell 좌표로 변환
  -> child mouse protocol 상태 확인
  -> selection gesture이면 selection state 갱신
  -> terminal mode이면 child PTY로 mouse event 전달
  -> navigation mode이면 hit_test로 sidebar/modal action 처리
```

### 4.5 Window 생성

```text
sidebar + new
  -> NativeMux::new_window(width, height)
  -> PtyTerminal::new_shell()
  -> active_window = new window
  -> metadata refresh
```

### 4.6 Session 전환

```text
session modal row click
  -> NativeMux::switch_session_by_row(row)
  -> active_session = row
  -> active terminal render source changes
```

외부 tmux client나 host terminal은 전환하지 않는다.

## 5. 테스트 전략

### 5.1 자동 테스트

- version parser와 legacy tmux fallback argument tests.
- doctor의 `TERM` handling tests.
- native mux session/window metadata tests.
- layout preview deterministic output tests.
- terminal key/mouse encoder unit tests.

### 5.2 수동 PTY smoke

macOS Apple Silicon에서 다음을 확인한다.

- `tuimux --session smoke`가 native shell을 연다.
- `printf 'tuimux-native-smoke\n'`가 shell에서 실행된다.
- mouse drag selection이 mouse-up 이후 유지된다.
- selection 상태 Ctrl-C 후 `pbpaste`가 선택 텍스트를 반환한다.
- `btop`이 80x24에서 terminal too small 없이 열린다.
- `htop`이 full-screen UI로 열린다.
- `nano`가 열리고 Ctrl-X prompt가 정상 처리된다.
- `llmfit --help`가 PTY surface 안에서 표시된다.

## 6. 릴리스 설계

v0.2.0-alpha.5는 macOS Apple Silicon만 대상으로 한다.

- GitHub Actions `release.yml`은 `aarch64-apple-darwin` tarball만 만든다.
- release asset 이름은 `tuimux-v0.2.0-alpha.5-aarch64-apple-darwin.tar.gz`다.
- `SHA256SUMS`를 같이 게시한다.
- installer는 OS/architecture를 확인하고 macOS ARM이 아니면 즉시 실패한다.
- installer는 tmux를 설치하거나 `.tmux.conf`를 수정하지 않는다.

설치 명령:

```sh
curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/v0.2.0-alpha.5/scripts/install.sh | \
  TUIMUX_VERSION=v0.2.0-alpha.5 bash
```

## 7. 알려진 한계와 다음 단계

- 현재 mux는 in-process라서 UI 종료 시 child PTY도 종료된다.
- `Detach`는 tmux의 detach와 동등하지 않다.
- split panes가 없다.
- session/window 상태가 disk나 daemon에 저장되지 않는다.
- true persistent multiplexer가 되려면 별도 server process, Unix socket protocol, attach client, PTY ownership transfer 정책이 필요하다.

다음 큰 단계는 `tuimux-server`를 별도 daemon으로 분리하고, 현재 `NativeMux`를 server-side state machine으로 옮기는 것이다.
