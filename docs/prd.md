# tuimux PRD

- **문서 버전**: 1.5
- **대상 릴리스**: v0.2.0-alpha.11
- **작성일**: 2026-06-13

## 1. 제품 방향

tuimux는 prefix를 외우지 않고 mouse-first로 다룰 수 있는 terminal multiplexer다. v0.2.0-alpha.11의 기본 실행 경로는 tmux wrapper가 아니라 Rust-native daemon-backed multiplexer이며, 세션/윈도우/PTY를 tuimux daemon이 직접 소유한다.

tmux는 안정적이지만 사용자가 원하는 native selection, clipboard, mouse, visual fidelity를 tuimux UI 안에서 세밀하게 제어하기 어렵다. 따라서 tmux C 코드는 참고하되, tuimux runtime은 Rust로 직접 구현한다.

제품 UX는 split pane이 아니라 오른쪽 window 목록에서 full-size terminal window를 골라 쓰는 방식이다. split pane은 default UI와 daemon protocol에서 deprecated다.

## 2. 사용자 문제

- terminal pane이 “진짜 터미널”처럼 느껴지지 않으면 full-screen 앱이 깨진다.
- mouse drag 후 선택이 사라지면 복사 흐름이 macOS Terminal과 다르게 느껴진다.
- Ctrl-C가 항상 child program으로 전달되면 선택 텍스트 복사와 process interrupt가 충돌한다.
- UI를 닫을 때 shell/session까지 사라지면 multiplexer로 믿고 쓰기 어렵다.
- 한 화면을 여러 pane으로 나누는 방식은 현재 목표가 아니며, window 목록 중심 UX가 더 단순하고 안정적이다.
- tmux 설정이나 runtime 설치가 필수이면 tuimux 자체 제품 경험을 제어하기 어렵다.

## 3. 릴리스 목표

- 기본 `tuimux` 실행에서 tmux 없이 native shell window를 연다.
- daemon이 session/window/PTY를 소유하고 UI client는 Unix socket으로 attach한다.
- UI detach/종료 후 같은 session으로 재attach하면 shell state가 유지된다.
- 같은 daemon에 여러 client가 동시에 연결될 수 있다.
- navigation mode에서 오른쪽 window 목록을 보고 `Tab`/arrow key로 window를 전환할 수 있다.
- split pane hotkey는 새 pane을 만들지 않고 deprecated status를 보여준다.
- terminal mode는 full-screen으로 동작해 `btop`, `htop`, `nano` 같은 앱에 충분한 PTY 크기를 준다.
- shell scrollback을 mouse wheel, `PageUp`/`PageDown`, `Home`, `End`로 볼 수 있다.
- mouse selection은 mouse-up 이후 유지된다.
- selection이 있을 때 Ctrl-C는 system clipboard copy로 동작한다.
- host paste는 bracketed paste event로 받아 active PTY에 전달한다.
- child가 mouse tracking을 켠 경우 normal mouse는 child로 보내고 Shift-drag를 tuimux selection override로 쓴다.
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
- navigation mode window 전환이 split-pane 조작 대신 동작한다.
- scrollback daemon regression test가 통과한다.
- host bracketed paste setup/restore가 적용되어 paste event가 active pane으로 전달된다.
- 열린 client가 있는 상태에서 두 번째 client가 snapshot/window/scrollback command를 수행하고 세 번째 client가 shutdown할 수 있다.
- `btop`, `htop`, `nano`, `llmfit --help`가 native terminal surface에서 실행된다.
- drag selection + Ctrl-C + `pbpaste` smoke test가 통과한다.
- GitHub prerelease에 macOS Apple Silicon tarball과 `SHA256SUMS`가 게시된다.

## 6. 다음 단계

다음 큰 제품 단계는 daemon restart persistence, Linux/Windows backend, client별 독립 viewport/cursor 정책, 더 넓은 terminal app compatibility suite다.
