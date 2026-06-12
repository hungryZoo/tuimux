# tuimux SRS (Software Requirements Specification)

- **문서 버전**: 0.9
- **작성일**: 2026-06-12
- **상태**: PTY 기반 터미널 surface 전환 명세
- **프로젝트명**: tuimux
- **상위 문서**: [docs/prd.md](./prd.md)
- **설계 문서**: [docs/sdd.md](./sdd.md)
- **한 줄 요약**: `tuimux`는 ratatui UI를 기본으로 유지하면서, main pane을 `capture-pane` 스냅샷이 아닌 실제 PTY + VT screen model 기반 터미널로 렌더해야 한다.

---

## 1. 범위

### 포함

- 인자 없는 `tuimux` 실행 시 ratatui 기반 tuimux TUI 시작.
- tmux server/session/window 조작은 실제 tmux backend를 사용.
- main pane은 별도 PTY에서 실행한 real tmux client output을 `vt100::Parser`로 해석해 렌더.
- `clear`, ANSI color/style, resize, cursor, alternate screen app에 대한 기본 terminal fidelity 개선.
- right sidebar의 Session, Detach, WINDOWS, `+ new`, window close UX 유지.
- session dialog에서 session 선택 시 embedded tmux client를 해당 session으로 재attach.
- Shift + 왼쪽 마우스 드래그는 tuimux가 소비하지 않고 host terminal native text selection에 맡긴다.
- Unix 계열 설치 스크립트는 `.tmux.conf`에 `mouse on`, `history-limit 100000` 설정이 없을 때만 추가한다.
- GitHub prerelease는 macOS, Windows, Linux tarball, Linux `.deb`/`.rpm`, Raspberry Pi용 Linux ARM asset을 제공한다.
- `--native-client`, `--doctor`, `--version`, `--layout-preview` 유지.
- 한국어 SRS/SDD 문서 유지.

### 제외

- v0.9에서 iTerm2 수준의 full `tmux -CC` control-mode client 구현.
- tmux pane split 전체를 tuimux 레이아웃으로 재구성하는 기능.
- tmux statusline/옵션을 영구 변경하는 방식의 UI 통합.
- native tmux client를 기본 UX로 되돌리는 변경.

---

## 2. 기능 요구사항

### 2.1 CLI

- **FR-CLI-1 [P0]** `tuimux`는 기본적으로 tuimux ratatui TUI를 실행해야 한다.
- **FR-CLI-2 [P0]** `tuimux --native-client`를 지정한 경우에만 plain native tmux client fallback을 실행해야 한다.
- **FR-CLI-3 [P0]** `tuimux --doctor`는 tmux 설치/버전/TERM/truecolor/TMUX 여부를 진단해야 한다.
- **FR-CLI-4 [P1]** `tuimux --layout-preview`는 non-interactive 환경에서 레이아웃 preview를 출력해야 한다.
- **FR-CLI-5 [P0]** stdout이 TTY가 아니면 raw mode/alternate screen에 진입하지 않고 안내 메시지와 non-zero exit code를 반환해야 한다.

### 2.2 TUI shell

- **FR-TUI-1 [P0]** TUI는 crossterm raw mode, alternate screen, mouse capture를 사용해야 한다.
- **FR-TUI-2 [P0]** tmux server/session이 없으면 detached `tuimux` session을 생성해야 한다.
- **FR-TUI-3 [P0]** 오른쪽 sidebar는 Session button, Detach button, WINDOWS list, `+ new` row를 렌더해야 한다.
- **FR-TUI-4 [P0]** session dialog는 session rows, `New Session`, `Detach` action을 제공해야 한다.
- **FR-TUI-5 [P0]** window row의 close target은 `tmux kill-window`를 실행해야 한다.
- **FR-TUI-6 [P0]** window 선택과 생성은 `tmux select-window`, `tmux new-window`를 통해 실제 tmux server에 반영되어야 한다.

### 2.3 Main Terminal Surface

- **FR-TERM-1 [P0]** main pane은 별도 PTY 안에서 `tmux -u attach-session -t <session>`을 실행한 real tmux client의 화면을 표시해야 한다.
- **FR-TERM-2 [P0]** PTY output byte stream은 `vt100::Parser`로 처리해 terminal screen cell state를 유지해야 한다.
- **FR-TERM-3 [P0]** main pane 렌더링은 `capture-pane` 반복 polling에 의존하면 안 된다.
- **FR-TERM-4 [P0]** `clear`, `reset`, `tput clear`, CSI clear sequence 이후 지워진 cell에 이전 glyph가 남으면 안 된다.
- **FR-TERM-5 [P0]** 16색, bold, dim, italic, underline, reverse style을 가능한 범위에서 ratatui style로 변환해야 한다.
- **FR-TERM-6 [P1]** 256색과 truecolor foreground/background를 지원해야 한다.
- **FR-TERM-7 [P0]** terminal cursor 위치와 cursor hidden state를 `vt100` screen model에서 읽어 main pane 안에 반영해야 한다.
- **FR-TERM-8 [P0]** host terminal resize 시 main pane inner rect 크기를 PTY size와 `vt100` parser size에 동기화해야 한다.
- **FR-TERM-9 [P1]** terminal app이 mouse protocol을 활성화하면 main pane mouse event를 child terminal로 전달해야 한다.
- **FR-TERM-10 [P1]** bracketed paste가 활성화된 경우 paste payload를 `\x1b[200~`/`\x1b[201~`로 감싸 전달해야 한다.
- **FR-TERM-11 [P0]** Shift + 왼쪽 mouse down/drag/up은 hover, sidebar action, child terminal mouse event로 처리하지 않아야 한다.

### 2.4 Input Mode

- **FR-IN-1 [P0]** main pane 클릭 시 terminal input mode에 진입해야 한다.
- **FR-IN-2 [P0]** terminal input mode에서 일반 문자, Enter, Backspace, Delete, Tab, arrows, Home/End, PageUp/PageDown, function key를 PTY byte sequence로 전달해야 한다.
- **FR-IN-3 [P0]** Ctrl-C, Ctrl-D, Ctrl-L 같은 shell control key는 terminal input mode에서 shell/tmux client로 전달되어야 한다.
- **FR-IN-4 [P0]** F12는 terminal input mode를 종료하고 navigation mode로 돌아와야 한다.
- **FR-IN-5 [P0]** navigation mode에서 `q`, `Esc`, Ctrl-C는 tuimux 종료로 처리되어야 한다.

### 2.5 Session 전환

- **FR-SESS-1 [P0]** session dialog에서 session을 선택하면 tuimux host process가 바깥 tmux client를 `switch-client`하지 않아야 한다.
- **FR-SESS-2 [P0]** session 선택 시 embedded tmux terminal을 종료하고 선택 session에 새 PTY tmux client를 attach해야 한다.
- **FR-SESS-3 [P0]** session/window metadata는 tmux command output을 주기적으로 또는 mutation 이후 갱신해야 한다.

### 2.6 Native fallback

- **FR-NATIVE-1 [P0]** `--native-client`는 명시적으로 요청한 경우에만 target session을 ensure하고 native tmux client를 실행해야 한다.
- **FR-NATIVE-2 [P0]** tmux 밖에서는 `tmux -u attach-session -t <session>`을 실행해야 한다.
- **FR-NATIVE-3 [P0]** tmux 안에서는 nested attach 대신 `tmux switch-client -t <session>`을 실행해야 한다.

### 2.7 Installer / Release

- **FR-INSTALL-1 [P0]** `scripts/install.sh`는 macOS와 Linux에서 실행 가능해야 한다.
- **FR-INSTALL-2 [P0]** `scripts/install.sh`는 OS/architecture를 감지해 macOS x86_64/arm64, Linux x86_64/arm64/armv7 tarball asset을 설치해야 한다.
- **FR-INSTALL-3 [P0]** `scripts/install.sh`는 `.tmux.conf`에 활성 `mouse` 설정이 없으면 `set -g mouse on`을 추가해야 한다.
- **FR-INSTALL-4 [P0]** `scripts/install.sh`는 `.tmux.conf`에 활성 `history-limit` 설정이 없으면 `set -g history-limit 100000`을 추가해야 한다.
- **FR-INSTALL-5 [P0]** `scripts/install.ps1`는 Windows x86_64/arm64 zip asset을 설치해야 한다.
- **FR-REL-1 [P0]** tag push release workflow는 macOS x86_64/arm64 tarball을 생성해야 한다.
- **FR-REL-2 [P0]** tag push release workflow는 Windows x86_64/arm64 zip을 생성해야 한다.
- **FR-REL-3 [P0]** tag push release workflow는 Linux x86_64/arm64/armv7 tarball을 생성해야 한다.
- **FR-REL-4 [P0]** tag push release workflow는 Debian/Ubuntu용 amd64/arm64/armhf `.deb`를 생성해야 한다.
- **FR-REL-5 [P0]** tag push release workflow는 RPM 계열용 x86_64/aarch64/armv7hl `.rpm`을 생성해야 한다.
- **FR-REL-6 [P0]** release는 모든 artifact에 대한 `SHA256SUMS`를 제공해야 한다.

---

## 3. 비기능 요구사항

- **NFR-FIDELITY-1 [P0]** main pane은 “text preview”처럼 보이면 안 되며, terminal screen state를 지속적으로 유지해야 한다.
- **NFR-SAFE-1 [P0]** 종료, detach, panic path 이후 raw mode, alternate screen, mouse capture, cursor 상태를 복구해야 한다.
- **NFR-PERF-1 [P1]** idle 상태에서 과도한 tmux command polling을 수행하지 않아야 한다.
- **NFR-PERF-2 [P1]** output이 빠르게 들어와도 UI input loop가 장시간 멈추면 안 된다.
- **NFR-COMPAT-1 [P0]** macOS, Linux, Raspberry Pi OS의 xterm-compatible terminal에서 동작해야 한다.
- **NFR-COMPAT-1A [P1]** Windows binary는 빌드/배포하되, 런타임은 PATH에서 접근 가능한 tmux 환경(MSYS2/Cygwin/WSL interop 등)을 요구한다.
- **NFR-COMPAT-2 [P0]** minimum tmux version은 `tmux 3.0` 이상으로 유지한다.
- **NFR-DOC-1 [P0]** 문서는 한국어로 새 구조와 알려진 한계를 설명해야 한다.

---

## 4. tmux command 명세

```text
TUI bootstrap/state:
  tmux list-sessions -F '#{session_name}\t#{session_windows}\t#{session_attached}'
  tmux new-session -d -s tuimux
  tmux list-windows -t <session> -F '#{window_index}\t#{window_name}\t#{window_active}'

Embedded terminal:
  env -u TMUX tmux -u attach-session -t <session>   # Unix PTY child

TUI mutations:
  tmux select-window -t <session>:<index>
  tmux new-window -t <session>
  tmux kill-window -t <session>:<index>
  tmux new-session -d -s <name>

Native fallback only:
  tmux set-option -gq mouse on
  tmux -u attach-session -t <session>               # outside tmux
  tmux switch-client -t <session>                   # inside tmux
```

---

## 5. 인수 기준

- **AC-1 [P0]** `cargo test --quiet`가 통과한다.
- **AC-2 [P0]** `cargo fmt -- --check`가 통과한다.
- **AC-3 [P0]** `tuimux` 기본 run mode는 ratatui TUI이며 `--native-client`에서만 native fallback을 선택한다.
- **AC-4 [P0]** main pane은 PTY-backed terminal surface로 동작하고 `capture-pane` 렌더링 경로를 사용하지 않는다.
- **AC-5 [P0]** `clear`/Ctrl-L 이후 이전 output이 main pane에 잔상으로 남지 않는다.
- **AC-6 [P0]** ANSI 16색과 bold/underline/reverse style이 렌더된다.
- **AC-7 [P0]** resize 후 main pane inner rect와 PTY/parser size가 일치한다.
- **AC-8 [P0]** terminal input mode에서 `echo hello`가 정확히 한 번 실행된다.
- **AC-9 [P0]** F12 후 `q`는 shell에 입력되지 않고 tuimux 종료로 처리된다.
- **AC-10 [P1]** `less`, `top`, `vim` 같은 alternate-screen app이 이전 snapshot 방식보다 명확히 안정적으로 표시된다.
- **AC-11 [P0]** `scripts/install.sh`를 빈 `TUIMUX_TMUX_CONF`로 실행하면 `set -g mouse on`과 `set -g history-limit 100000`이 추가되고, 재실행해도 중복되지 않는다.
- **AC-12 [P0]** Shift + 왼쪽 mouse drag unit test가 tuimux host text selection override를 검증한다.
- **AC-13 [P0]** release workflow가 macOS, Windows, Linux tarball, `.deb`, `.rpm`, `SHA256SUMS` artifact를 생성한다.

---

## 6. 알려진 한계와 후속 과제

- 현재 구현은 `tmux -CC` control-mode client가 아니라 native tmux client를 PTY 안에 embedding하는 방식이다.
- 따라서 장기적으로는 tmux pane 단위 event와 layout을 직접 해석하는 control-mode renderer로 발전할 수 있다.
- embedded tmux client의 statusline과 tuimux sidebar가 동시에 보일 수 있다. 별도 tmux option을 영구 변경하지 않기 위해 v0.9에서는 이를 허용한다.
- old `capture-pane`/`send-keys` 렌더링 경로는 runtime path에서 제거되었다.

---

## 7. 변경 이력

- **0.9 / 2026-06-12**: `hungryZoo/tscode`의 terminal pane 구조를 참고해 `portable-pty` + `vt100` 기반 embedded tmux terminal surface로 main pane을 전환. SRS/SDD 한국어 재작성.
- **0.8 / 2026-06-08**: default run mode를 tuimux ratatui TUI로 복구. plain native tmux client는 hidden `--native-client` fallback으로 이동.
- **0.6-0.7 / 2026-06-08**: `capture-pane`/`send-keys` 기반 interaction 개선 및 default native client 회귀 수정.
