# tuimux SRS (Software Requirements Specification)

- **문서 버전**: 0.8 (Draft)
- **작성일**: 2026-06-08
- **상태**: v0.1.8 TUI 기본값 복구 구현 명세
- **프로젝트명**: tuimux
- **상위 문서**: [docs/prd.md](./prd.md) (PRD v0.8)
- **한 줄 요약**: v0.1.8은 인자 없는 `tuimux`가 plain tmux client가 아니라 tuimux ratatui TUI를 실행하도록 복구한다.

---

## 1. 범위

### 포함

- `tuimux` 기본 실행 시 ratatui TUI 시작.
- session 자동 생성 및 live tmux state 조회.
- sidebar session/window controls 유지.
- hidden `--native-client` fallback 제공.
- `--session <NAME>`은 fallback native client target에 사용하고, 기본 TUI에서는 tmux state UI를 보여준다.
- `--doctor`, `--version`, `--layout-preview` 유지.
- 기본 run mode 회귀 방지 unit test 추가.

### 제외

- v0.1.8에서 full `tmux -CC` custom client 구현.
- v0.1.8에서 완전한 terminal emulator/VT parser 구현.
- plain tmux client를 기본 interactive UX로 사용하는 것.

---

## 2. 기능 요구사항

### 2.1 CLI

- **FR-CLI-1 [M]** `tuimux --help`는 tuimux TUI 기본 UX를 설명한다.
- **FR-CLI-2 [M]** `tuimux --version`은 package version을 출력한다.
- **FR-CLI-3 [M]** `tuimux --doctor`는 tmux 설치/버전/TERM/truecolor/TMUX 여부를 점검한다.
- **FR-CLI-4 [M]** `tuimux --layout-preview`는 non-interactive preview를 출력한다.
- **FR-CLI-5 [M]** `tuimux` 인자 없음은 `tui::run` 경로를 선택한다.
- **FR-CLI-6 [M]** `tuimux --native-client`만 plain native tmux client 경로를 선택한다.
- **FR-CLI-7 [M]** `--dashboard`는 default TUI와 동일한 compatibility hidden option이다.

### 2.2 TUI 기본 경로

- **FR-TUI-1 [M]** stdout이 TTY면 crossterm raw mode와 alternate screen을 시작한다.
- **FR-TUI-2 [M]** tmux server/session이 없으면 detached `tuimux` session을 생성한다.
- **FR-TUI-3 [M]** 오른쪽 sidebar에 Session button, Detach button, WINDOWS list, `+ new` row를 렌더한다.
- **FR-TUI-4 [M]** session dialog는 headerless이며 session rows, `New Session`, `Detach`를 렌더한다.
- **FR-TUI-5 [M]** window row의 `✕`는 별도 hover/action target으로 `tmux kill-window`를 실행한다.
- **FR-TUI-6 [M]** terminal input mode는 `KeyEventKind::Press`만 tmux에 전달하고 F12로 navigation mode에 복귀한다.

### 2.3 Native fallback

- **FR-NATIVE-1 [M]** `--native-client`가 있을 때만 target session을 ensure하고 `tmux set-option -gq mouse on`을 실행한다.
- **FR-NATIVE-2 [M]** fallback은 tmux 밖에서 `tmux -u attach-session -t <session>`을 실행한다.
- **FR-NATIVE-3 [M]** fallback은 tmux 안에서 nested attach 대신 `tmux switch-client -t <session>`을 실행한다.

---

## 3. tmux command 명세

```text
TUI bootstrap:
  tmux list-sessions -F '#{session_name}\t#{session_windows}\t#{session_attached}'
  tmux new-session -d -s tuimux          # only if no session exists
  tmux list-windows -t <session> -F '#{window_index}\t#{window_name}\t#{window_active}'

TUI mutations:
  tmux select-window -t <session>:<index>
  tmux new-window -t <session>
  tmux kill-window -t <session>:<index>
  tmux new-session -d -s <name>

Native fallback only:
  tmux set-option -gq mouse on
  tmux -u attach-session -t <session>    # outside tmux
  tmux switch-client -t <session>        # inside tmux
```

---

## 4. 인수 기준

- **AC-1** `cargo test --quiet`가 통과한다.
- **AC-2** run mode unit test가 인자 없는 `Cli`를 `RunMode::Dashboard`로 검증한다.
- **AC-3** run mode unit test가 `native_client=true`일 때만 `RunMode::NativeClient`를 검증한다.
- **AC-4** `cargo run --quiet -- --version`은 `tuimux 0.1.8`을 출력한다.
- **AC-5** `cargo run --quiet -- --layout-preview`는 Session/Detach/WINDOWS/New Session UI를 출력한다.
- **AC-6** PTY smoke test에서 인자 없는 `tuimux`가 tuimux UI 문자열을 렌더한다.
- **AC-7** release workflow가 macOS arm64/x86_64 assets와 `SHA256SUMS`를 만든다.
- **AC-8** raw installer가 `v0.1.8` asset을 설치한다.

---

## 5. 위험 및 후속 과제

- main pane은 아직 snapshot/input bridge라 full-screen apps나 wide character fidelity가 완벽하지 않을 수 있다.
- 하지만 그 문제를 default TUI 제거로 해결하면 안 된다.
- 다음 milestone은 control-mode renderer나 tmux popup/native binding 기반으로 shell fidelity와 prefix-free UI를 동시에 만족해야 한다.

---

## 6. 변경 이력

- **0.8 / 2026-06-08**: default run mode를 tuimux ratatui TUI로 복구. plain native tmux client는 hidden `--native-client` fallback으로 이동. 회귀 방지 unit test 추가.
- **0.7 / 2026-06-08**: default interactive path를 real tmux native client로 전환했으나 제품 UI가 사라지는 회귀 발생.
- **0.6 / 2026-06-08**: visible-screen capture, key repeat/release 차단, window close hover 보정.
- **0.5 / 2026-06-08**: capture-pane/send-keys 기반 main pane interaction 시도.
- **0.4 이하**: session/window command scaffold, compact UI preview, installer/release 구축.
