# tuimux PRD

- **문서 버전**: 1.0
- **대상 릴리스**: v0.2.0-alpha.5
- **작성일**: 2026-06-13

## 1. 제품 방향

tuimux는 prefix를 외우지 않고 mouse-first로 다룰 수 있는 terminal multiplexer다. v0.2.0-alpha.5부터 기본 실행 경로는 tmux wrapper가 아니라 Rust-native multiplexer다.

tmux는 안정적이지만 사용자가 원하는 native selection, clipboard, mouse, visual fidelity를 tuimux UI 안에서 세밀하게 제어하기 어렵다. 따라서 tmux C 코드는 참고하되, tuimux runtime은 Rust로 직접 구현한다.

## 2. 사용자 문제

- terminal pane이 “진짜 터미널”처럼 느껴지지 않으면 full-screen 앱이 깨진다.
- mouse drag 후 선택이 사라지면 복사 흐름이 macOS Terminal과 다르게 느껴진다.
- Ctrl-C가 항상 child program으로 전달되면 선택 텍스트 복사와 process interrupt가 충돌한다.
- tmux 설정이나 runtime 설치가 필수이면 tuimux 자체 제품 경험을 제어하기 어렵다.

## 3. 릴리스 목표

- 기본 `tuimux` 실행에서 tmux 없이 native shell window를 연다.
- terminal mode는 full-screen으로 동작해 `btop`, `htop`, `nano` 같은 앱에 충분한 PTY 크기를 준다.
- mouse selection은 mouse-up 이후 유지된다.
- selection이 있을 때 Ctrl-C는 system clipboard copy로 동작한다.
- child가 mouse tracking을 켠 경우 normal mouse는 child로 보내고 Shift-drag를 tuimux selection override로 쓴다.
- macOS Apple Silicon 프리릴리즈를 먼저 배포한다.

## 4. 비목표

- v0.2.0-alpha.5에서 tmux와 동일한 persistent daemon을 완성하지 않는다.
- detach 후 session/process가 살아남는 것을 보장하지 않는다.
- split panes, tmux command 호환성, plugin 호환성은 포함하지 않는다.
- Windows/Linux asset은 이번 프리릴리즈에 포함하지 않는다.

## 5. 성공 기준

- `cargo test` 통과.
- macOS ARM release build 성공.
- `tuimux --doctor`가 tmux 부재를 실패로 보지 않는다.
- `btop`, `htop`, `nano`, `llmfit --help`가 native terminal surface에서 실행된다.
- drag selection + Ctrl-C + `pbpaste` smoke test가 통과한다.
- GitHub prerelease에 macOS Apple Silicon tarball과 `SHA256SUMS`가 게시된다.

## 6. 다음 단계

진짜 tmux 대체재가 되려면 현재 in-process `NativeMux`를 `tuimux-server` daemon으로 분리해야 한다. 그 다음 attach/detach protocol, socket API, persistent PTY ownership, split pane layout을 추가한다.
