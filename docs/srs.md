# tuimux SRS

- **문서 버전**: 1.4
- **대상 릴리스**: v0.2.0-alpha.9
- **작성일**: 2026-06-13
- **상태**: Rust-native daemon-backed multiplexer 알파 명세

## 1. 목적

tuimux는 기본 실행 경로에서 tmux server/client에 의존하지 않는 Rust-native terminal multiplexer다. 사용자가 `tuimux`를 실행하면 UI client는 tuimux daemon에 attach하고, daemon은 세션, 윈도우, pane, PTY child process, terminal screen model을 직접 소유해야 한다.

tmux의 C 코드는 세션/윈도우/PTY ownership/입력 라우팅/mouse mode 처리의 참고 자료로만 사용한다. 제품 코드는 Rust로 직접 구현한다.

## 2. 범위

### 2.1 포함 범위

- Unix/macOS에서 Rust-native daemon process를 자동 실행하고 Unix socket으로 attach한다.
- UI detach/종료 후에도 daemon-owned session/window/PTY child process를 유지한다.
- Rust-native 세션 목록, 활성 세션, 윈도우 목록, 윈도우 생성/선택/종료를 제공한다.
- 각 윈도우는 하나 이상의 Rust-native pane을 가진다.
- pane right/down split, pane 선택, pane cycle, pane 종료를 제공한다.
- 각 pane은 실제 PTY-backed shell을 실행한다.
- `vt100` parser 기반 screen model을 ratatui로 렌더링한다.
- terminal fullscreen mode와 navigation/sidebar mode를 제공한다.
- mouse drag selection은 mouse-up 이후 유지된다.
- 선택 영역이 있을 때 Ctrl-C는 child process SIGINT가 아니라 system clipboard copy로 처리된다.
- host terminal paste는 crossterm paste event로 받아 active PTY로 전달하되 child bracketed paste mode를 존중한다.
- child application이 mouse tracking을 켠 경우 normal mouse는 child로 전달하고, Shift-drag는 tuimux selection으로 처리한다.
- tmux는 hidden `--native-client` fallback에서만 사용한다.
- macOS Apple Silicon 프리릴리즈 설치 스크립트를 제공한다.

### 2.2 제외 범위

- tmux command language, control mode, plugin/config 호환성.
- tmux layout string 호환성.
- daemon 재시작 후 session 복구를 위한 disk persistence.
- 동시 다중 attach의 독립 cursor/viewport 정책.
- Windows named-pipe daemon backend.
- Linux/Windows 프리릴리즈 asset.

현재 `Detach`는 UI client 종료이며 daemon과 child PTY는 계속 살아남는다. 다만 `tuimux --stop-server`, daemon crash, host reboot 후에는 세션 복구를 보장하지 않는다.

## 3. 기능 요구사항

### 3.1 CLI

- **FR-CLI-1 [P0]** 인자 없는 `tuimux`는 Rust-native tuimux TUI client를 실행해야 한다.
- **FR-CLI-2 [P0]** Unix/macOS 기본 실행은 daemon에 attach해야 하며, daemon이 없으면 자동 spawn해야 한다.
- **FR-CLI-3 [P0]** daemon spawn/connect 실패는 조용한 in-process fallback으로 숨기지 말고 사용자에게 실패로 드러나야 한다.
- **FR-CLI-4 [P0]** `--native-client`를 지정한 경우에만 plain tmux client fallback을 실행해야 한다.
- **FR-CLI-5 [P0]** `--doctor`는 tmux 부재를 기본 실행 실패로 처리하지 않아야 한다.
- **FR-CLI-6 [P0]** `--doctor`는 `TERM`이 비어 있거나 `dumb`이면 실패해야 한다.
- **FR-CLI-7 [P1]** `--layout-preview`는 CI/문서 확인용 정적 preview를 출력해야 한다.
- **FR-CLI-8 [P1]** 내부용 `--daemon`, `--socket`, `--stop-server`는 운영/테스트 lifecycle을 지원해야 한다.

### 3.2 Daemon Backend

- **FR-DMN-1 [P0]** daemon은 UI process와 별도 process로 실행되어야 한다.
- **FR-DMN-2 [P0]** macOS/Unix spawn 시 daemon은 parent process group/session에서 분리되어 UI 종료 후에도 유지되어야 한다.
- **FR-DMN-3 [P0]** socket path는 macOS Unix socket path length 제한을 넘지 않도록 `/tmp/tuimux-$USER/<session-hash>.sock` 형태의 짧은 경로를 사용해야 한다.
- **FR-DMN-4 [P0]** daemon은 JSON line request/response protocol로 snapshot, key, paste, mouse, session/window/pane command를 처리해야 한다.
- **FR-DMN-5 [P0]** `--stop-server --session <name>`은 해당 session daemon에 shutdown request를 보내야 한다.
- **FR-DMN-6 [P1]** stale socket이 있으면 새 daemon spawn 전에 제거해야 한다.

### 3.3 Native Multiplexer

- **FR-MUX-1 [P0]** daemon은 시작 시 초기 session 하나와 shell window 하나를 직접 생성해야 한다.
- **FR-MUX-2 [P0]** session 생성은 외부 tmux 명령 없이 native state와 PTY child process를 생성해야 한다.
- **FR-MUX-3 [P0]** session 선택은 active session index만 변경하고 host terminal이나 외부 tmux client를 조작하지 않아야 한다.
- **FR-MUX-4 [P0]** window 생성은 active session에 새 PTY-backed shell을 추가해야 한다.
- **FR-MUX-5 [P0]** window 선택은 active window를 변경하고 해당 window의 screen을 렌더해야 한다.
- **FR-MUX-6 [P0]** 마지막 window를 종료하면 replacement shell window를 만들어 빈 session panic을 방지해야 한다.
- **FR-MUX-7 [P0]** UI detach 후 같은 `--session`으로 재attach하면 기존 shell state가 유지되어야 한다.
- **FR-MUX-8 [P0]** active window는 하나 이상의 pane을 소유해야 하며 각 pane은 별도 PTY-backed shell이어야 한다.
- **FR-MUX-9 [P0]** navigation mode에서 active pane을 right split 또는 down split할 수 있어야 한다.
- **FR-MUX-10 [P0]** split 후 새 pane이 active가 되고 입력, cursor, mouse event는 active pane으로 라우팅되어야 한다.
- **FR-MUX-11 [P0]** pane cycle/select는 외부 tmux 없이 active pane index만 변경해야 한다.
- **FR-MUX-12 [P0]** 마지막 pane을 종료하면 replacement shell pane을 만들어 빈 window panic을 방지해야 한다.
- **FR-MUX-13 [P0]** split은 active pane leaf만 나누는 nested pane tree로 저장해야 하며, 기존 split 구조를 덮어쓰면 안 된다.
- **FR-MUX-14 [P0]** active pane resize는 가장 가까운 같은 방향 split ratio를 조정해야 한다.

### 3.4 PTY Terminal

- **FR-TERM-1 [P0]** 각 pane은 real PTY를 소유해야 한다.
- **FR-TERM-2 [P0]** PTY child는 사용자의 `$SHELL`을 현재 작업 디렉터리에서 실행해야 한다.
- **FR-TERM-3 [P0]** child 환경에는 `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=tuimux`를 제공해야 한다.
- **FR-TERM-4 [P0]** PTY output은 byte stream 그대로 `vt100::Parser`에 입력해야 한다.
- **FR-TERM-5 [P0]** 화면 렌더링은 cell별 fg/bg, bold, dim, italic, underline, inverse를 보존해야 한다.
- **FR-TERM-6 [P0]** default foreground/background는 강제로 칠하지 않아 host terminal의 native color 느낌을 유지해야 한다.
- **FR-TERM-7 [P0]** terminal mode에서는 main terminal body가 host TUI 전체 크기를 사용해야 한다.
- **FR-TERM-8 [P0]** host resize 시 active PTY size와 parser screen size를 같이 갱신해야 한다.
- **FR-TERM-9 [P0]** full-screen TUI 앱의 alternate screen, cursor visibility, mouse tracking escape sequence를 보존해야 한다.
- **FR-TERM-10 [P0]** split pane layout에서는 pane별 PTY size와 screen size가 화면의 pane rect와 일치해야 한다.

### 3.5 입력과 마우스

- **FR-IN-1 [P0]** terminal mode의 일반 키 입력은 active PTY로 전달해야 한다.
- **FR-IN-2 [P0]** navigation mode에서는 `F12`, `q`, `Esc`, sidebar mouse action 등 tuimux chrome 조작을 처리해야 한다.
- **FR-IN-3 [P0]** `F12`는 terminal mode와 navigation/sidebar mode를 전환해야 한다.
- **FR-IN-4 [P0]** 선택 영역이 없을 때 Ctrl-C는 active PTY로 전달되어 실행 중 프로그램의 일반 Ctrl-C로 동작해야 한다.
- **FR-IN-5 [P0]** 선택 영역이 있을 때 Ctrl-C는 선택 텍스트를 system clipboard에 복사하고 PTY로 Ctrl-C를 보내지 않아야 한다.
- **FR-IN-6 [P0]** mouse-up 이후 선택 영역은 자동으로 사라지지 않아야 한다.
- **FR-IN-7 [P0]** child가 mouse protocol을 켜지 않은 상태에서는 left drag가 tuimux selection을 시작해야 한다.
- **FR-IN-8 [P0]** child가 mouse protocol을 켠 상태에서는 normal mouse를 child로 보내야 한다.
- **FR-IN-9 [P0]** child mouse protocol 활성 상태에서도 Shift-left-drag는 tuimux selection을 시작해야 한다.
- **FR-IN-10 [P0]** UI setup은 host bracketed paste mode를 활성화하고 restore 시 비활성화해야 한다.
- **FR-IN-11 [P1]** paste event는 child bracketed paste mode가 활성일 때 `ESC [ 200 ~` / `ESC [ 201 ~`로 감싸야 한다.
- **FR-IN-12 [P0]** split pane에서 mouse event 좌표는 해당 pane의 local row/col로 변환되어 active pane에 전달되어야 한다.
- **FR-IN-13 [P0]** drag selection은 시작 pane을 기억해야 하며, drag가 separator나 다른 pane 위로 지나가도 focus가 튀지 않아야 한다.

### 3.6 Clipboard

- **FR-CLIP-1 [P0]** macOS에서는 `pbcopy`로 system clipboard에 복사해야 한다.
- **FR-CLIP-2 [P1]** Linux에서는 `wl-copy`, `xclip`, `xsel` 순서로 가능한 clipboard command를 사용해야 한다.
- **FR-CLIP-3 [P1]** Windows에서는 `clip`을 사용할 수 있어야 한다.
- **FR-CLIP-4 [P0]** clipboard command 실패는 panic이 아니라 status message로 알려야 한다.

### 3.7 설치와 릴리스

- **FR-REL-1 [P0]** v0.2.0-alpha.9 프리릴리즈는 macOS Apple Silicon tarball만 게시한다.
- **FR-REL-2 [P0]** `scripts/install.sh`는 macOS Apple Silicon 외의 OS/architecture에서 명확히 실패해야 한다.
- **FR-REL-3 [P0]** installer는 tmux 설치를 요구하거나 `.tmux.conf`를 수정하지 않아야 한다.
- **FR-REL-4 [P0]** installer는 release asset checksum이 있으면 검증해야 한다.

## 4. 비기능 요구사항

- **NFR-UX-1 [P0]** terminal mode는 “가짜 터미널 preview”처럼 보이지 않아야 하며, shell/editor/monitor 프로그램이 실제 PTY 안에서 실행되어야 한다.
- **NFR-UX-2 [P0]** `btop`, `htop`, `nano` 같은 full-screen 앱은 80x24 host에서 크기 부족 오류 없이 실행되어야 한다.
- **NFR-UX-3 [P0]** 선택/복사는 macOS 기본 Terminal에 가깝게 동작해야 한다.
- **NFR-COMPAT-1 [P0]** macOS Terminal.app / iTerm2 계열 xterm-compatible host에서 동작해야 한다.
- **NFR-PERF-1 [P1]** idle loop는 과도한 external command polling을 수행하지 않아야 한다.
- **NFR-ROBUST-1 [P0]** child PTY read error나 process 종료가 tuimux panic으로 이어지면 안 된다.
- **NFR-OBS-1 [P1]** doctor 출력에서 native tuimux가 tmux를 요구하지 않는다는 사실이 드러나야 한다.

## 5. 수용 기준

- **AC-1 [P0]** `cargo fmt -- --check`와 `cargo test --quiet`가 통과한다.
- **AC-2 [P0]** `TERM=xterm-256color tuimux --doctor`가 0으로 종료한다.
- **AC-3 [P0]** `TERM=dumb tuimux --doctor`가 non-zero로 종료한다.
- **AC-4 [P0]** `tuimux` 실행 시 tmux attach 화면이 아니라 tuimux native UI가 뜬다.
- **AC-5 [P0]** terminal mode에서 `printf 'hello\n'` 입력이 active shell에서 실행된다.
- **AC-6 [P0]** UI를 종료한 뒤 같은 `--session`으로 재attach하면 shell 환경값이 유지된다.
- **AC-7 [P0]** `btop`이 80x24 host에서 “terminal too small” 오류 없이 열린다.
- **AC-8 [P0]** `htop`이 full-screen UI로 열린 뒤 `q`로 종료된다.
- **AC-9 [P0]** `nano`가 열리고 입력, 저장, 종료가 정상 처리된다.
- **AC-10 [P0]** `llmfit --help` 출력이 native PTY surface 안에서 깨지지 않고 표시된다.
- **AC-11 [P0]** mouse drag로 선택한 텍스트가 mouse-up 이후 남아 있다.
- **AC-12 [P0]** 선택 영역이 있을 때 Ctrl-C 후 macOS `pbpaste`가 선택 텍스트를 반환한다.
- **AC-13 [P0]** 선택 영역이 있을 때 Ctrl-C가 shell에 SIGINT를 보내지 않는다.
- **AC-14 [P0]** navigation mode에서 `|` 또는 `v`로 right split하면 sidebar에 pane 2가 나타나고 active pane shell이 입력을 받는다.
- **AC-15 [P0]** navigation mode에서 `-` 또는 `h`로 down split하면 pane별 local 좌표와 cursor가 깨지지 않는다.
- **AC-16 [P0]** split pane 상태에서 drag selection + Ctrl-C는 선택 시작 pane의 텍스트를 clipboard로 복사한다.
- **AC-17 [P0]** `cargo build --release --locked --target aarch64-apple-darwin`가 성공한다.
- **AC-18 [P0]** macOS ARM installer가 `tuimux --version`과 `tuimux --doctor` 검증 안내를 출력한다.
- **AC-19 [P0]** host paste는 개별 key 입력 폭주가 아니라 paste event로 처리되어 active pane PTY에 전달된다.
- **AC-20 [P0]** right split 후 기존 pane을 down split해도 기존 right split이 보존되는 nested layout이 유지된다.
- **AC-21 [P0]** navigation mode arrow resize는 active pane의 rect와 PTY size를 변경한다.

## 6. 변경 이력

- **1.4 / 2026-06-13**: nested pane tree, pane separator geometry, arrow-key pane resize 요구사항을 추가하고 macOS ARM alpha.9 요구사항을 명시.
- **1.3 / 2026-06-13**: host bracketed paste enable/disable 요구사항을 추가하고 macOS ARM alpha.8 요구사항을 명시.
- **1.2 / 2026-06-13**: Rust-native daemon window 안에 pane 모델을 추가. right/down split, pane select/cycle/kill, pane-local mouse 좌표, selection 시작 pane 유지, macOS ARM alpha.7 요구사항을 명시.
- **1.1 / 2026-06-13**: 기본 backend를 Rust-native daemon-backed multiplexer로 갱신. Unix socket attach, daemon detach/reattach, 짧은 socket path, daemon stop flow, macOS ARM alpha.6 요구사항을 명시.
- **1.0 / 2026-06-13**: 기본 backend를 tmux embedding에서 Rust-native in-process multiplexer로 전환. PTY shell window, fullscreen terminal mode, mouse selection 유지, Ctrl-C clipboard copy, macOS ARM prerelease 요구사항을 명시.
