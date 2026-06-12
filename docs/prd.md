# tuimux PRD

- **문서 버전**: 3.2
- **대상 릴리스**: v0.2.0-alpha.28
- **작성일**: 2026-06-13

## 1. 제품 방향

tuimux는 prefix를 외우지 않고 mouse-first로 다룰 수 있는 terminal multiplexer다. v0.2.0-alpha.28의 기본 실행 경로는 tmux wrapper가 아니라 Rust-native daemon-backed multiplexer이며, 세션/윈도우/PTY를 tuimux daemon이 직접 소유한다.

tmux는 안정적이지만 사용자가 원하는 native selection, clipboard, mouse, visual fidelity를 tuimux UI 안에서 세밀하게 제어하기 어렵다. 따라서 tmux C 코드는 참고하되, tuimux runtime은 Rust로 직접 구현한다.

제품 UX는 split pane이 아니라 오른쪽 window 목록에서 full-size terminal window를 골라 쓰는 방식이다. split pane은 default UI와 daemon protocol에서 deprecated이며 native mux core에서도 layout state를 갖지 않는다.

## 2. 사용자 문제

- terminal pane이 “진짜 터미널”처럼 느껴지지 않으면 full-screen 앱이 깨진다.
- mouse drag 후 선택이 사라지면 복사 흐름이 macOS Terminal과 다르게 느껴진다.
- Ctrl-C가 항상 child program으로 전달되면 선택 텍스트 복사와 process interrupt가 충돌한다.
- UI를 닫을 때 shell/session까지 사라지면 multiplexer로 믿고 쓰기 어렵다.
- shell이 `exit`로 종료됐는데 window가 stale 화면으로 남으면 실제 terminal이 아니라 frozen preview처럼 느껴진다.
- alternate-screen 앱이 종료된 뒤 잔상이나 alternate-screen text가 primary scrollback에 남으면 native terminal과 다르게 느껴진다.
- child 앱이 terminal title이나 OSC 52 clipboard를 쓰는데 UI가 무시하면 window list와 복사 흐름이 native terminal보다 둔하게 느껴진다.
- 한 화면을 여러 pane으로 나누는 방식은 현재 목표가 아니며, window 목록 중심 UX가 더 단순하고 안정적이다.
- tmux 설정이나 runtime 설치가 필수이면 tuimux 자체 제품 경험을 제어하기 어렵다.

## 3. 릴리스 목표

- 기본 `tuimux` 실행에서 tmux 없이 native shell window를 연다.
- daemon이 session/window/PTY를 소유하고 UI client는 Unix socket으로 attach한다.
- UI detach/종료 후 같은 session으로 재attach하면 shell state가 유지된다.
- 같은 daemon에 여러 client가 동시에 연결될 수 있다.
- navigation mode에서 오른쪽 window 목록을 보고 `Tab`/arrow key로 window를 전환하고, `n`/`x`로 window를 만들고 닫을 수 있다.
- child가 OSC 0/1/2로 terminal title을 설정하면 오른쪽 window 목록에 해당 title을 표시한다.
- child shell이 자체 종료되면 non-last window는 목록에서 제거하고, 마지막 window는 새 shell로 대체한다.
- split pane hotkey는 새 pane을 만들지 않고 deprecated status를 보여주며 core state를 바꾸지 않는다.
- terminal mode는 full-screen으로 동작해 `btop`, `htop`, `nano` 같은 앱에 충분한 PTY 크기를 준다.
- shell scrollback을 mouse wheel, `PageUp`/`PageDown`, `Home`, `End`로 볼 수 있다.
- mouse selection은 mouse-up 이후 유지되며 선택된 텍스트는 daemon이 active PTY screen에서 추출한다.
- selection이 있을 때 Ctrl-C는 system clipboard copy로 동작한다.
- child의 OSC 52 clipboard copy 요청은 macOS system clipboard로 이어진다.
- host paste는 bracketed paste event로 받아 active PTY에 전달한다.
- child가 mouse tracking을 켠 경우 normal mouse는 child로 보내고 Shift-drag를 tuimux selection override로 쓴다.
- child가 명시적으로 출력한 truecolor foreground/background/default reset은 부모 환경의 `NO_COLOR`와 무관하게 native terminal color로 보존한다.
- host terminal resize는 active child PTY까지 전달되어 full-screen 앱과 shell이 새 rows/cols를 관측한다.
- terminal row는 viewport 폭까지 명시적으로 렌더해 이전 frame의 긴 줄 glyph가 다음 frame에 남지 않게 한다.
- alternate-screen output은 active 상태에서만 보이고 종료 후 primary screen/scrollback으로 누수되지 않는다.
- macOS Apple Silicon 프리릴리즈를 먼저 배포한다.

## 4. 비목표

- daemon 재시작/host reboot 후 session 복구.
- tmux layout string, tmux command 호환성, plugin 호환성.
- split-pane UX.
- client별 독립 cursor/viewport 정책.
- Windows/Linux asset은 이번 프리릴리즈에 포함하지 않는다.

## 5. 성공 기준

- `cargo fmt -- --check`와 `cargo test --quiet` 통과.
- macOS ARM release build 성공.
- `tuimux --doctor`가 tmux 부재를 실패로 보지 않는다.
- detach 후 reattach smoke에서 shell 환경값이 유지된다.
- navigation mode window 전환/생성/종료가 split-pane 조작 대신 동작한다.
- native mux core에 split layout tree가 남지 않고 window마다 single full-size PTY pane만 유지된다.
- scrollback daemon regression test가 통과한다.
- host bracketed paste setup/restore가 적용되어 paste event가 active pane으로 전달된다.
- 열린 client가 있는 상태에서 두 번째 client가 snapshot/window/scrollback command를 수행하고 세 번째 client가 shutdown할 수 있다.
- `btop`, `htop`, `nano`, `llmfit --help`가 native terminal surface에서 실행된다.
- drag selection이 mouse-up 이후 화면에 reverse-video highlight로 남고 Ctrl-C + `pbpaste` smoke test가 통과한다.
- UI selection lifecycle과 daemon selected-text/highlight regression test가 통과한다.
- macOS PTY UI smoke에서 drag selection, Ctrl-C clipboard copy, foreground child SIGINT 미전달, host bracketed paste 전달, child bracketed paste wrapper 보존이 통과한다.
- macOS session-flow smoke에서 detach/reattach shell state 유지와 window-list workflow, split deprecated status가 통과한다.
- macOS no-tmux smoke에서 tmux 없는 PATH의 default TUI/doctor 성공과 `--native-client` 실패가 통과한다.
- `--layout-preview`가 split-pane/resize 샘플이 아닌 single full-size terminal + window-list preview를 출력한다.
- macOS mouse-protocol smoke에서 child SGR mouse tracking 중 normal mouse forwarding과 Shift-drag selection override가 통과한다.
- macOS scrollback smoke에서 실제 TUI의 mouse wheel, `PageUp`, `Home`, `End` active terminal history navigation과 scrollback 중 paste bottom 복귀가 통과한다.
- macOS truecolor smoke에서 `NO_COLOR=1` 부모 환경에서도 child `38;2`/`48;2` SGR과 default reset이 실제 TUI output에 보존된다.
- macOS resize smoke에서 host PTY resize 후 child가 `SIGWINCH`와 새 `32x120` terminal size를 관측한다.
- macOS alternate-screen smoke와 daemon regression에서 alternate-screen active/exit, primary screen 복귀, primary scrollback 격리가 통과한다.
- macOS child-exit smoke에서 마지막 shell 종료 후 replacement shell이 명령을 받고, non-last shell 종료 후 window list에서 제거된다.
- macOS window-title smoke에서 child OSC title이 오른쪽 window list에 표시된다.
- macOS OSC 52 clipboard smoke에서 child copy 요청 후 `pbpaste`가 요청한 텍스트를 반환한다.
- GitHub prerelease에 macOS Apple Silicon tarball과 `SHA256SUMS`가 게시된다.

## 6. 다음 단계

다음 큰 제품 단계는 daemon restart persistence, Linux/Windows backend, client별 독립 viewport/cursor 정책, 더 넓은 terminal app compatibility suite다.
