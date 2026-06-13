# tuimux SDD

- **문서 버전**: 3.6
- **대상 릴리스**: v0.2.0-alpha.33
- **작성일**: 2026-06-13
- **상태**: Rust-native daemon-backed multiplexer 설계

## 1. 설계 목표

기존 tuimux의 가장 큰 문제는 main pane이 실제 terminal이 아니라 tmux output을 간접적으로 보여주는 느낌을 준다는 점이었다. v0.2.0-alpha.33 릴리스는 default path에서 tmux를 제거한 daemon-backed 구조를 유지하고, 제품 UX와 native mux core를 split-pane이 아니라 window-list 중심의 terminal workflow로 정리한다. 또한 기본 terminal mode에서도 넓은 화면에는 오른쪽 boxed rail을 보여, 사용자가 `F12`를 누르기 전에도 tuimux 조작면이 보이게 한다. 위/아래 status bar와 compact top-tab fallback은 btop 같은 full-screen 앱의 PTY 크기를 보존하기 위해 비활성화한다.

핵심 목표는 다음과 같다.

- shell/editor/monitor가 실제 PTY 안에서 실행된다.
- UI client가 닫혀도 daemon-owned PTY와 shell process는 살아남는다.
- main terminal mode는 host terminal 안에 오른쪽 rail만 표시하고, rail을 제외한 body 크기를 child PTY에 제공한다.
- 사용자는 오른쪽 window 목록에서 terminal window를 고른다.
- terminal mode에서는 오른쪽 rail을 클릭하거나 `Alt-N`, `Alt-S`, `Alt-Left`/`Alt-Right` hotkey로 기본 window/session 작업을 바로 수행한다.
- split-pane 생성/resize/cycle/kill은 기본 UI, daemon protocol, native mux core에서 제거한다.
- 기존 client가 연결된 상태에서도 새 attach, snapshot, command, shutdown request를 처리한다.
- host terminal의 bracketed paste mode를 UI 생명주기에 맞춰 켜고 끈다.
- scrollback, selection, clipboard는 host terminal에 가까운 감각으로 동작한다.
- renderer는 이전 frame의 긴 줄이 다음 frame의 짧은 줄 뒤에 남지 않도록 row를 viewport 폭까지 지운다.
- terminal emulator 입력은 `CSI row;col f` HVP(cursor position) sequence를 `CSI row;col H`와 동일하게 처리해 btop 같은 앱의 절대 위치 렌더링을 보존한다.
- child application의 OSC title, OSC 52 clipboard copy, OSC 52 clipboard paste query를 terminal callback으로 처리한다.
- tmux C 코드는 구조 참고로만 삼고 Rust 모듈로 직접 구현한다.
- tmux fallback은 hidden `--native-client` 옵션에만 남긴다.

## 2. 전체 구조

```text
┌───────────────────────────────┐        JSON line over Unix socket
│ tuimux UI client process      │◀─────────────────────────────────┐
├───────────────────────────────┤                                  │
│ src/main.rs                   │                                  │
│   CLI / doctor / fallback     │                                  │
│ src/tui.rs                    │                                  │
│   ratatui render              │                                  │
│   input routing               │                                  │
│   selection state             │                                  │
│ src/mux_backend.rs            │                                  │
│   MuxBackend::Remote          │                                  │
└───────────────────────────────┘                                  │
                                                                   │
┌───────────────────────────────┐                                  │
│ tuimux daemon process         │──────────────────────────────────┘
├───────────────────────────────┤
│ src/mux_backend.rs            │
│   UnixListener / Request      │
│ src/native_mux.rs             │
│   sessions / windows / PTYs   │
│ src/terminal.rs               │
│   PTY / child shell / vt100    │
│ src/clipboard.rs              │
│   platform clipboard commands │
└───────────────────────────────┘
```

UI process는 terminal raw mode, alternate screen, host bracketed paste, ratatui frame, mouse/keyboard interaction, selection state를 담당한다. daemon process는 session/window state와 모든 PTY child를 소유한다.

## 3. 모듈 설계

### 3.1 `src/main.rs`

`main.rs`는 CLI boundary다.

- `tuimux` 기본 실행은 `tui::run()`으로 들어간다.
- `--daemon --socket <path>`는 내부 daemon entrypoint다.
- `--stop-server --session <name>`은 해당 session daemon에 shutdown request를 보낸다.
- `--doctor`는 native runtime 진단을 출력한다.
- `--layout-preview`는 정적 preview를 출력한다.
- `--native-client`가 있을 때만 `tmux::probe()`와 tmux fallback을 사용한다.

tmux가 설치되어 있지 않아도 기본 `tuimux` 실행과 `--doctor`는 성공할 수 있어야 한다.

### 3.2 `src/mux_backend.rs`

`mux_backend.rs`는 UI와 mux core 사이의 경계다.

```rust
pub enum MuxBackend {
    Remote(RemoteMuxClient),
    Local(NativeMux),
}
```

Unix/macOS에서는 `RemoteMuxClient::connect_or_spawn()`을 사용한다. 기존 daemon socket에 연결할 수 있으면 재사용하고, 연결할 수 없으면 stale socket을 제거한 뒤 현재 `tuimux` binary를 `--daemon` mode로 spawn한다.

daemon spawn 정책:

- `stdin`, `stdout`, `stderr`는 null로 닫는다.
- `setsid()`로 parent process group/session에서 분리한다.
- socket path는 `/tmp/tuimux-$USER/<session>-<hash>.sock`처럼 짧게 만든다.
- macOS `$TMPDIR`의 긴 경로는 Unix socket path length 제한에 걸릴 수 있으므로 사용하지 않는다.
- Unix에서 daemon 연결 실패는 조용히 local fallback으로 숨기지 않는다.

protocol은 newline-delimited JSON이다.

```text
Request::Snapshot { width, height, selection }
Request::SendKey { key }
Request::SendPaste { text }
Request::SendMouse { mouse }
Request::NewWindow { width, height }
Request::KillWindowByRow { row, width, height }
Request::SelectPaneByRow { row }
Request::CreateNextSession { width, height }
Request::SwitchSessionByRow { row }
Request::SelectWindowByRow { row }
Request::ScrollPane { lines }
Request::SelectedText { selection }
Request::Shutdown
```

응답은 `Ok`, `Snapshot`, `Name`, `Index`, `Scrollback`, `Text`, `Error` 중 하나다. daemon은 listener를 nonblocking mode로 두고 accepted client stream은 blocking mode로 되돌린 뒤 thread별로 처리한다. `NativeMux`는 `Arc<Mutex<NativeMux>>`로 공유한다. 여러 client가 동시에 attach할 수 있지만 독립 viewport/cursor 정책은 아직 범위 밖이며 active session/window state는 공유한다.

`SplitPaneRight`, `SplitPaneDown`, `SelectNextPane`, `KillActivePane`, `ResizePane` request는 default product protocol에서 제거했다. `native_mux.rs`의 split layout tree와 split/resize/kill pane core도 제거되어, legacy split hotkey는 daemon state를 변경하지 않는다.

### 3.3 `src/native_mux.rs`

`NativeMux`는 daemon-side multiplexer core다.

```rust
pub struct NativeMux {
    sessions: Vec<NativeSession>,
    active_session: usize,
    cwd: PathBuf,
    next_session: u32,
    next_window_id: u32,
}
```

각 `NativeSession`은 window list와 active window index를 가진다. 각 `NativeWindow`는 index/name, single PTY pane list, active pane index만 가진다. 기본 제품 path에서는 window마다 하나의 active terminal pane이 현재 terminal body를 채운다.

window list의 표시 이름은 `NativeWindow::display_name()`으로 계산한다. active pane의 `PtyTerminal::title()`이 있으면 child가 OSC 0/1/2로 설정한 title을 우선 표시하고, 없으면 기존 fallback 이름인 `shell` 또는 `shell-<n>`을 표시한다. pane metadata도 같은 방식으로 `display_title()`을 사용해 snapshot에 반영한다.

주요 동작:

- `NativeMux::new()`는 초기 session과 첫 shell window를 생성한다.
- `create_next_session()`은 `tuimux-<n>` 이름으로 새 session을 만든다.
- `new_window()`는 active session에 새 shell PTY를 추가한다.
- `select_window_by_row()`와 `switch_session_by_row()`는 외부 process 없이 active index만 바꾼다.
- `kill_window_by_row()`는 마지막 window가 제거될 때 replacement shell을 만들어 빈 session을 방지한다.
- `select_pane_by_row()`는 remote snapshot 호환을 위한 안전장치로 남아 있지만 기본 UI에서 pane 목록은 노출하지 않는다.
- `drain_all()`은 모든 window/pane의 pending PTY output을 parser에 반영한다.
- `reap_finished_windows()`는 종료된 PTY child를 가진 window를 제거하고, session의 마지막 window가 종료되었으면 replacement shell window를 생성한다.
- `resize_active()`는 active window의 terminal rect에 맞춰 PTY와 parser screen size를 갱신한다.

split 관련 `PaneNode::Split`, split/resize/kill pane 함수, pane separator 계산은 core에서 제거했다. `active_pane_refs()`는 항상 active window의 single PTY pane을 전체 terminal rect로 보고하고, `active_pane_separators()`는 빈 목록을 반환한다.

`NativeMux`가 daemon 안에 있기 때문에 UI client가 종료되어도 drop되지 않는다. daemon이 종료될 때만 `PtyTerminal::drop()`이 child를 정리한다.

### 3.4 `src/terminal.rs`

`PtyTerminal`은 terminal fidelity의 중심이다.

```rust
pub struct PtyTerminal {
    parser: vt100::Parser<TerminalCallbacks>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    rx: Receiver<Vec<u8>>,
    rows: u16,
    cols: u16,
    finished: bool,
}
```

생성 흐름:

1. `portable_pty::native_pty_system().openpty()`로 PTY pair를 만든다.
2. slave에서 `$SHELL`을 실행한다.
3. child 환경에 `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=tuimux`를 설정한다.
4. master reader는 background thread에서 읽고 `mpsc` channel로 bytes를 보낸다.
5. daemon은 `drain()`으로 bytes를 받아 `vt100::Parser::process()`에 넣는다.

`TerminalCallbacks`는 `vt100::Callbacks` 구현체다.

- OSC 0은 icon name과 window title을 모두 갱신한다.
- OSC 1은 icon name을 갱신하고 window title이 없을 때 fallback title로 쓰인다.
- OSC 2는 window title을 갱신한다.
- title은 control character를 제거하고 120자까지만 유지한다.
- OSC 52 `c` selector와 base64 payload는 UTF-8 text로 decode해 system clipboard에 best-effort copy한다.
- OSC 52 `c` selector와 `?` payload는 system clipboard text를 읽고 `ESC ] 52 ; c ; <base64> BEL` response를 PTY에 되돌려준다.

렌더링:

- `styled_rows_with_selection(selection)`은 vt100 cell을 `TerminalSpan`으로 변환한다.
- fg/bg가 default일 때 ratatui default color를 그대로 사용해 host terminal color를 보존한다.
- fg/bg가 truecolor일 때 `ratatui::Color::Rgb`로 유지하고, UI setup에서 `crossterm::style::force_color_output(true)`를 호출해 부모 환경의 `NO_COLOR`가 child가 명시적으로 출력한 SGR을 제거하지 못하게 한다.
- selection cell은 inverse bit를 토글해 시각적으로 강조한다.
- wide continuation cell은 중복 렌더하지 않는다.
- scrollback viewport가 bottom이 아니면 cursor를 숨긴다.

입력:

- `send_key()`는 crossterm `KeyEvent`를 xterm-compatible byte sequence로 변환한다.
- Ctrl 문자, Alt prefix, arrow/application cursor, function key, delete/page key를 처리한다.
- `send_paste()`는 child bracketed paste mode를 감지해 paste wrapper를 적용한다.
- `send_mouse_event()`는 child가 mouse protocol을 활성화한 경우에만 SGR/normal mouse sequence를 전달한다.
- key/paste/child mouse event는 scrollback viewport를 bottom으로 되돌린다.

스크롤백:

- parser는 10,000줄 scrollback buffer를 가진다.
- `scrollback_up(rows)`와 `scrollback_down(rows)`는 vt100 screen의 scrollback offset을 조정한다.
- `scrollback_bottom()`은 offset을 0으로 되돌린다.
- snapshot은 active terminal의 `scrollback` 값을 포함한다.

종료 감지:

- reader thread가 PTY EOF/read error로 종료되어 channel이 disconnect되면 `PtyTerminal`은 `finished` 상태를 기록한다.
- `drain()`은 pending bytes를 parser에 반영한 뒤 `Child::try_wait()`로 child process 종료를 nonblocking으로 확인한다.
- `is_finished()`는 cached `finished` 상태를 반환하거나 `try_wait()` 결과를 반영한다.
- `Drop`은 아직 finished가 아닌 child만 best-effort kill한다.
- daemon snapshot 경로는 `resize_active()`, `drain_all()`, `local_snapshot()`, `reap_finished_windows()` 순서로 실행한다. 그래서 child가 출력 직후 종료되어도 마지막 terminal 화면은 현재 snapshot에 한 번 노출되고, replacement/prune은 다음 snapshot부터 보인다.

### 3.5 `src/tui.rs`

`tui.rs`는 UI state, rendering, input routing을 담당한다. UI가 직접 `NativeMux`를 소유하지 않고 `MuxBackend`에 요청한다.

```rust
struct UiState {
    mux: MuxBackend,
    sessions: Vec<Session>,
    windows: Vec<Window>,
    panes: Vec<Pane>,
    current_session: String,
    terminal_rows: Vec<Vec<TerminalSpan>>,
    terminal_cursor: Option<(u16, u16)>,
    terminal_mouse_protocol_active: bool,
    terminal_scrollback: usize,
    selection: Option<SelectionState>,
}
```

두 가지 mode가 있다.

- **Terminal mode**: 첫 화면이자 기본 mode. 넓은 화면에서는 오른쪽 boxed rail이 session/window/action 조작을 제공하고, terminal body는 rail만 제외한 나머지 전체 영역을 차지한다. 좁은 화면에서는 compact top tabs를 만들지 않고 terminal body가 전체 화면을 사용한다. 키 입력은 기본적으로 active PTY로 간다.
- **Navigation mode**: `F12`로 진입한다. main pane border와 right sidebar가 보이고 session/window 조작을 할 수 있다.

Terminal mode는 넓은 화면에서 TUI rail을 기본으로 노출해 사용자가 multiplexer 기능을 마우스로 바로 쓰게 한다. rail은 100컬럼 이상에서만 표시하고, 표시될 때도 child PTY가 최소 80컬럼을 유지하도록 폭을 제한한다. 100컬럼 미만이거나 높이가 낮은 화면에서는 rail과 compact top-tab fallback을 모두 숨겨 full-screen 프로그램이 실제 host 크기에 가까운 PTY를 받도록 한다. Navigation mode의 오른쪽 sidebar는 같은 session, detach, status, windows 목록을 focus된 조작면으로 다시 보여준다.

terminal row 렌더링은 `terminal_row_spans_for_width(row, width)`를 통해 각 row 끝을 default-style 공백으로 채운다. ratatui backend가 짧은 `Line`만 받으면 이전 frame의 더 긴 glyph가 host terminal에 남을 수 있으므로, 모든 terminal row는 현재 viewport width만큼 명시적으로 덮어쓴다. default-style 공백은 foreground/background를 강제로 칠하지 않아 host terminal color policy를 유지한다.

Navigation mode 키:

- `Tab`, arrow key: 이전/다음 window 선택.
- `n`: active session에 새 shell window 생성.
- `x`: active window 종료. 마지막 window이면 replacement shell window를 만든다.
- `PageUp`, `PageDown`: active terminal scrollback을 한 화면 단위로 이동.
- `Home`: 가능한 가장 위쪽 scrollback으로 이동.
- `End`: scrollback bottom으로 이동.
- `|`, `v`, `-`, `h`: legacy split key로 예약되어 있지만 새 pane을 만들지 않는다.

`setup()`은 raw mode, alternate screen, mouse capture와 함께 `EnableBracketedPaste`를 실행한다. 또한 tuimux가 일반 CLI가 아니라 child terminal renderer라는 점 때문에 `force_color_output(true)`를 호출해 parent-side `NO_COLOR`가 child output color를 지우지 않게 한다. `restore()`는 alternate screen/mouse capture를 해제하면서 `DisableBracketedPaste`도 실행해 host terminal 상태를 되돌린다.

### 3.6 Selection과 Ctrl-C

선택 state는 UI client가 갖고, 선택 텍스트 추출은 daemon snapshot screen에서 수행한다.

```rust
struct SelectionState {
    pane: usize,
    anchor: (u16, u16),
    focus: (u16, u16),
    dragging: bool,
}
```

동작:

- child mouse protocol이 꺼져 있으면 left down/drag/up이 tuimux selection을 만든다.
- child mouse protocol이 켜져 있으면 normal mouse는 child로 전달한다.
- child mouse protocol이 켜져 있어도 Shift-left-drag는 tuimux selection을 만든다.
- mouse-up은 drag-in-progress 상태를 종료하지만 selection 자체를 지우지 않는다.
- selection이 zero-width이면 지운다.
- 새 key input이나 paste는 selection을 지운다. 단, Ctrl-C copy는 selection을 유지한다.

terminal mode에서 Ctrl-C는 다음 순서로 처리된다.

1. UI가 `selection_range()`를 확인한다.
2. selection이 있으면 `Request::SelectedText`로 daemon에서 텍스트를 추출한다.
3. UI process가 `clipboard::copy_text()`로 system clipboard에 복사한다.
4. 복사 성공 시 Ctrl-C byte를 PTY로 보내지 않는다.
5. selection이 없으면 일반 Ctrl-C byte를 daemon의 active PTY로 보낸다.

이 설계는 macOS 기본 Terminal처럼 “텍스트를 선택한 상태의 Ctrl-C는 복사”라는 동작을 우선한다.

### 3.7 Clipboard

별도 Rust clipboard crate를 추가하지 않고 platform command를 사용한다.

- macOS: `pbcopy`
- Windows: `clip`
- Linux: `wl-copy`, `xclip -selection clipboard`, `xsel --clipboard --input`

command가 없거나 실패하면 UI status에 error를 표시하고 panic하지 않는다.

clipboard read도 같은 command bridge를 사용한다.

- macOS: `pbpaste`
- Windows: `powershell -NoProfile -Command Get-Clipboard -Raw`
- Linux: `wl-paste --no-newline`, `xclip -selection clipboard -out`, `xsel --clipboard --output`

OSC 52 clipboard copy는 daemon-side terminal callback에서 처리한다. child가 `ESC ] 52 ; c ; <base64> BEL`을 출력하면 `PtyTerminal::drain()`이 pending clipboard text를 꺼내 `clipboard::copy_text()`로 전달한다. child가 `ESC ] 52 ; c ; ? BEL`을 출력하면 `PtyTerminal::respond_pending_clipboard_paste()`가 `clipboard::read_text()` 결과를 base64로 인코딩해 PTY에 `ESC ] 52 ; c ; <base64> BEL`을 쓴다. 두 경로 모두 terminal app compatibility를 위한 best-effort 동작이며, UI selection Ctrl-C 경로처럼 foreground child에 Ctrl-C를 보내지 않는 동작과 별개다.

## 4. 주요 흐름

### 4.1 시작과 attach

```text
main()
  -> parse CLI
  -> tui::run(session, cwd)
  -> UiState::bootstrap()
  -> MuxBackend::new()
  -> RemoteMuxClient::connect_or_spawn()
     -> connect existing socket, or spawn `tuimux --daemon --socket ...`
  -> ratatui raw mode + alternate screen
```

### 4.2 Daemon 시작

```text
tuimux --daemon --socket <path> --session <name> --cwd <path>
  -> UnixListener::bind(socket)
  -> NativeMux::new(session, cwd, 80, 24)
  -> nonblocking accept loop
  -> spawn one worker thread per accepted client
  -> worker reads Request lines
  -> lock shared NativeMux
  -> mutate/drain/resize NativeMux
  -> write Response lines
```

### 4.3 렌더 루프

```text
loop
  -> state.sync_terminal(current terminal_body size)
     -> Request::Snapshot { width, height, selection }
     -> daemon resize_active + drain_all + local_snapshot + reap_finished_windows
  -> terminal.draw(ui)
  -> crossterm event poll/read
```

terminal mode에서 root frame은 화면 폭에 따라 두 형태로 나뉜다. 넓은 화면은 왼쪽 `terminal_body`와 오른쪽 `side_rail`로만 나뉜다. 위/아래 status bar는 만들지 않는다. 좁은 화면은 `side_rail` 없이 전체 root를 `terminal_body`로 사용한다. `terminal_body`만 active child PTY size와 mouse/selection coordinate에 사용한다. navigation mode에서는 main pane과 sidebar layout으로 나뉜다.

### 4.3.1 Terminal Mode Chrome

```text
┌ root ───────────────────────────────────────┐
│ <active child PTY terminal body>             │Session │
│                                              │Detach  │
│                                              │        │
│                                              │WINDOWS │
│                                              │1:shell✕│
│                                              │+ new   │
│                                              │scroll… │
│                                              │F12 nav │
└──────────────────────────────────────────────┘
```

넓은 화면의 오른쪽 rail은 session button, detach button, active/inactive window rows, close button, 새 window row를 boxed control로 노출한다. windows 아래에는 scrollback count와 hint/status row 두 줄을 둔다. rail rect는 `Regions`에 별도 기록되며 `terminal_cell_at_pane()`에 포함되지 않는다. 따라서 rail 위 mouse click은 child mouse event로 전달되지 않고, terminal body 안의 drag/click만 selection 또는 child mouse protocol routing으로 들어간다.

### 4.4 Window Navigation

```text
navigation key Tab / arrow
  -> select_adjacent_window(delta)
  -> Request::SelectWindowByRow { row }
  -> daemon active window index 변경
  -> next Snapshot renders selected window
```

```text
navigation key n
  -> Request::NewWindow { width, height }
  -> daemon spawn_window()
  -> created window becomes active
```

```text
navigation key x
  -> Request::KillWindowByRow { active_row, width, height }
  -> daemon removes active window, or creates replacement when it was last
```

```text
window row click
  -> Request::SelectWindowByRow { row }
```

```text
new window click
  -> Request::NewWindow { width, height }
  -> daemon spawn_window()
```

### 4.5 Scrollback

```text
mouse wheel, child mouse protocol off
  -> Request::ScrollPane { lines: +/-3 }
  -> vt100 screen scrollback offset 변경
  -> Snapshot includes scrollback offset
```

```text
navigation PageUp/PageDown/Home/End
  -> Request::ScrollPane { lines }
  -> active terminal viewport 이동
```

key input, paste, child mouse event는 `PtyTerminal::scrollback_bottom()`을 먼저 호출해 과거 화면에서 현재 입력이 보이지 않는 문제를 방지한다.

### 4.6 Key와 Mouse Input

```text
crossterm KeyEvent
  -> F12이면 mode toggle
  -> Alt-N / Alt-S / Alt-Left / Alt-Right이면 terminal mode에서도 tuimux command 처리
  -> terminal_mode이면 UiState::send_terminal_key()
     -> Ctrl-C + selection이면 clipboard copy
     -> 아니면 Request::SendKey
  -> navigation mode이면 sidebar/session/window/scrollback shortcut 처리
```

```text
crossterm MouseEvent
  -> terminal cell 좌표 계산
  -> child mouse protocol 상태 확인
  -> mouse wheel + child mouse off이면 scrollback 처리
  -> selection gesture이면 selection state 갱신
  -> terminal mode이면 Request::SendMouse
  -> navigation mode이면 hit_test로 sidebar/modal action 처리
```

```text
crossterm PasteEvent
  -> terminal_mode이면 UiState::send_terminal_paste()
  -> Request::SendPaste { text }
  -> daemon active terminal PtyTerminal::send_paste()
  -> child bracketed paste mode이면 ESC [ 200 ~ / ESC [ 201 ~ wrapper 적용
```

### 4.7 Detach와 Reattach

```text
F12, q, Esc, or Detach button
  -> UI restores host terminal
  -> UI process exits
  -> daemon remains alive with PPID=1 on macOS test path
  -> same `tuimux --session <name>` connects to existing socket
  -> shell state and PTY screen continue from daemon state
```

## 5. 테스트 전략

### 5.1 자동 테스트

- version parser와 legacy tmux fallback argument tests.
- doctor의 `TERM` handling tests.
- native mux session/window metadata tests.
- native mux single terminal-body pane regression test.
- layout preview deterministic output tests.
- terminal key/mouse encoder unit tests.
- terminal parser truecolor preservation tests.
- terminal callback tests: OSC 0/1/2 title capture, OSC 52 clipboard decode, OSC 52 paste query, OSC 52 response encoding.
- ratatui paragraph truecolor buffer preservation tests.
- crossterm backend truecolor SGR emission tests with parent `NO_COLOR` override.
- terminal row padding regression test: 긴 row 이후 짧은 row를 그렸을 때 stale glyph가 남지 않는지 확인.
- UI selection lifecycle regression tests: mouse-up 후 선택 유지, zero-width 선택 제거, 일반 key input 시 선택 제거.
- terminal-mode rail regression tests: 넓은 기본 terminal mode render buffer에 Session/Detach/WINDOWS rail, scrollback count, hint row가 찍히고, rail이 terminal cell hit-test에서 제외되는지 확인. 좁은 화면에서는 compact top tab fallback 없이 전체 root가 terminal body가 되는지 확인.
- daemon multi-client regression test.
- daemon window workflow regression test: `NewWindow`, `SelectWindowByRow`, `KillWindowByRow`가 split command 없이 window list state를 갱신하는지 확인.
- daemon child-exit regression test: shell `exit` 직전 출력이 한 snapshot에 노출되고, 마지막 shell `exit` 후 replacement shell이 명령을 받을 수 있으며, non-last shell `exit` 후 window list에서 제거되는지 확인.
- daemon alternate-screen regression test: alternate-screen marker가 primary screen 복귀 후 primary scrollback snapshot에 섞이지 않는지 확인.
- daemon window-title regression test: child OSC 2 title이 snapshot window/pane metadata에 반영되는지 확인.
- daemon scrollback regression test: shell에서 50줄 출력, `ScrollPane` 요청, snapshot scrollback 증가와 cursor hide 확인.
- daemon selected-text regression test: PTY 화면의 선택 좌표에서 `SelectedText`가 텍스트를 반환하고 selection snapshot이 inverse style을 표시하는지 확인.
- macOS PTY UI smoke script: 실제 TUI client를 pseudo terminal에서 실행하고 SGR mouse drag 후 mouse-up frame에 reverse-video selection highlight가 남는지, Ctrl-C, `pbpaste`, foreground child SIGINT trap 미발생, host bracketed paste가 child PTY에서 실행되는지, child가 bracketed paste mode일 때 wrapper를 받는지 확인.
- macOS terminal-chrome smoke script: 실제 TUI client 기본 화면에서 boxed right rail과 scrollback/hint rows가 보이는지, child terminal body가 계속 명령을 실행하는지, rail `+ new` mouse click과 `F12` navigation handoff가 동작하는지 확인.
- macOS app smoke script: 실제 TUI client 안에서 `llmfit --help`, `btop`, `htop`, `nano`를 실행해 output/full-screen UI/input/save/exit 동작을 확인.
- macOS mouse-protocol smoke script: raw child가 `1002`/`1006` SGR mouse tracking을 켠 뒤 normal click forwarding, Shift-drag tuimux selection override, selection Ctrl-C child 누수 방지를 확인.
- macOS scrollback smoke script: 실제 TUI client에서 긴 shell output을 만든 뒤 mouse wheel, `PageUp`, `Home`, `End`가 active terminal history viewport를 이동하고 bottom으로 돌아오는지 확인하며, scrollback 중 host paste가 live bottom으로 복귀해 active shell에서 실행되는지 확인.
- macOS truecolor smoke script: parent `NO_COLOR=1` 환경에서 child가 출력한 `38;2` foreground, `48;2` background, default color reset이 실제 TUI output에 남는지 확인.
- macOS resize smoke script: 실제 TUI client의 host PTY를 resize한 뒤 active child process가 `SIGWINCH`와 오른쪽 rail을 제외한 새 `32x100` terminal body size를 관측하는지 확인.
- macOS alternate-screen smoke script: 실제 TUI client에서 raw alternate-screen sequence가 active일 때 보이고 종료 후 primary screen으로 복귀하는지 확인.
- macOS window-title smoke script: 실제 TUI client에서 child OSC title이 오른쪽 window list row에 표시되는지 확인.
- macOS OSC 52 clipboard smoke script: 실제 TUI client에서 child OSC 52 copy 요청이 macOS `pbpaste`에 반영되는지 확인.
- macOS OSC 52 paste smoke script: 실제 TUI client에서 child OSC 52 paste query가 macOS clipboard text를 PTY response로 돌려받는지 확인.
- macOS session-flow smoke script: 실제 TUI client에서 `F12` navigation mode, 오른쪽 window list, `n` 새 window, `x` window 종료, detach, 같은 session reattach 후 shell state 유지를 확인.
- macOS child-exit smoke script: 실제 TUI client에서 마지막 shell `exit` 후 replacement shell 명령 실행, non-last shell `exit` 후 오른쪽 window list 제거를 확인.
- macOS no-tmux smoke script: `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, `SHELL=/bin/sh` 환경에서 `--doctor`, default TUI PTY shell, `--native-client` failure boundary를 확인.

### 5.2 macOS Apple Silicon smoke

검증할 항목:

- `cargo fmt -- --check`
- `cargo test --quiet`
- `TERM=xterm-256color COLORTERM=truecolor cargo run --quiet -- --doctor`
- `TERM=dumb cargo run --quiet -- --doctor` non-zero 확인
- `cargo build --release --locked --target aarch64-apple-darwin`
- `python3 scripts/smoke_macos_terminal_chrome.py --binary target/debug/tuimux`
- `python3 scripts/smoke_macos_ui_selection.py --binary target/debug/tuimux`
- `python3 scripts/smoke_macos_apps.py --binary target/debug/tuimux`
- `python3 scripts/smoke_macos_mouse_protocol.py --binary target/debug/tuimux`
- `python3 scripts/smoke_macos_scrollback.py --binary target/debug/tuimux`
- `python3 scripts/smoke_macos_color.py --binary target/debug/tuimux`
- `python3 scripts/smoke_macos_resize.py --binary target/debug/tuimux`
- `python3 scripts/smoke_macos_altscreen.py --binary target/debug/tuimux`
- `python3 scripts/smoke_macos_window_title.py --binary target/debug/tuimux`
- `python3 scripts/smoke_macos_osc52_clipboard.py --binary target/debug/tuimux`
- `python3 scripts/smoke_macos_osc52_paste.py --binary target/debug/tuimux`
- `python3 scripts/smoke_macos_session_flow.py --binary target/debug/tuimux`
- `python3 scripts/smoke_macos_child_exit.py --binary target/debug/tuimux`
- `python3 scripts/smoke_macos_no_tmux.py --binary target/debug/tuimux`
- `tuimux --session persist-smoke`에서 `export TUIMUX_PERSIST_MARK=alive`
- UI 종료 후 daemon `PPID=1`과 `/tmp/tuimux-$USER/*.sock` 유지 확인
- 같은 session reattach 후 `echo $TUIMUX_PERSIST_MARK`가 `alive` 출력
- `btop`, `htop`, `nano`, `llmfit --help` 실행 확인
- mouse drag selection, Ctrl-C, `pbpaste`가 선택 텍스트 반환 확인
- mouse-up 이후 선택 텍스트가 reverse-video highlight로 남는지 확인
- child mouse tracking 중 normal click은 child로 전달되고 Shift-drag는 tuimux selection으로 처리되는지 확인
- parent `NO_COLOR=1` 상태에서도 child truecolor foreground/background SGR과 default reset이 tuimux TUI output에 보존되는지 확인
- host resize 후 active child PTY가 새 rows/cols와 `SIGWINCH`를 관측하는지 확인
- alternate-screen active/exit 후 primary screen 복귀와 primary scrollback 격리 확인
- child OSC title이 오른쪽 window list에 표시되는지 확인
- child OSC 52 copy가 `pbpaste`에 반영되는지 확인
- child OSC 52 paste query가 clipboard text를 PTY response로 돌려받는지 확인
- mouse wheel/PageUp/PageDown/Home/End scrollback 확인
- scrollback 중 host paste가 live bottom으로 돌아와 active shell에서 실행되는지 확인
- navigation mode에서 `n` 새 window, `x` active window 종료, `Tab`/arrow window 전환 확인
- shell `exit`로 마지막 window replacement와 non-last window list 제거 확인
- legacy split hotkey가 native mux core에 pane state를 만들지 않는지 확인

## 6. 릴리스 설계

v0.2.0-alpha.33 릴리스는 macOS Apple Silicon만 대상으로 한다.

- GitHub Actions `release.yml`은 `aarch64-apple-darwin` tarball만 만든다.
- release asset 이름은 `tuimux-v0.2.0-alpha.33-aarch64-apple-darwin.tar.gz`다.
- `SHA256SUMS`를 같이 게시한다.
- installer는 OS/architecture를 확인하고 macOS ARM이 아니면 즉시 실패한다.
- installer는 tmux를 설치하거나 `.tmux.conf`를 수정하지 않는다.

설치 명령:

```sh
curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/v0.2.0-alpha.33/scripts/install.sh | \
  TUIMUX_VERSION=v0.2.0-alpha.33 bash
```

## 7. 알려진 한계와 다음 단계

- daemon state는 memory-only라 daemon 종료, reboot 후 session이 복구되지 않는다.
- 여러 client는 같은 active session/window state를 공유한다. client별 독립 viewport/cursor는 아직 없다.
- split-pane UX는 deprecated이며 native mux core에는 split layout state가 없다. window-list workflow를 우선한다.
- Windows named-pipe daemon backend가 없다.
- tmux command/plugin/config 호환성은 없다.
- scrollback reflow와 더 다양한 alternate-screen 앱 edge case는 실제 앱 확대 테스트가 더 필요하다.

다음 큰 단계는 daemon persistence metadata, Linux/Windows backend, client별 독립 viewport/cursor 정책, 더 긴 full-screen app compatibility suite다.
