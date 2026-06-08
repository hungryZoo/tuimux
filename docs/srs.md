# tuimux SRS (Software Requirements Specification)

- **문서 버전**: 0.7 (Draft)
- **작성일**: 2026-06-08
- **상태**: v0.1.7 tmux-native 재설계 구현 명세
- **프로젝트명**: tuimux
- **상위 문서**: [docs/prd.md](./prd.md) (PRD v0.7)
- **한 줄 요약**: v0.1.7은 `capture-pane`/`send-keys` 기반 shell emulation을 기본 경로에서 제거하고, 실제 `tmux -u attach-session`/`switch-client`를 실행한다.

---

## 1. 범위

### 포함

- `tuimux` 기본 실행 시 native tmux client attach/switch.
- session 자동 생성.
- tmux mouse mode 자동 활성화.
- UTF-8 native client flag `-u` 사용.
- `--session <NAME>`으로 target session 지정.
- `--doctor`, `--version`, `--layout-preview` 유지.
- 이전 ratatui dashboard는 숨겨진 `--dashboard` prototype으로만 유지.

### 제외

- 기본 interactive shell에서 `capture-pane` polling renderer 사용.
- 기본 interactive shell에서 `send-keys` replay input 사용.
- 자체 terminal emulator/VT parser 구현.
- full `tmux -CC` custom client 구현.
- right sidebar overlay를 실제 shell 위에 완성하는 것.

---

## 2. 기능 요구사항

### 2.1 CLI

- **FR-CLI-1 [M]** `tuimux --help`는 기본 native attach UX와 `--session` 옵션을 설명한다.
- **FR-CLI-2 [M]** `tuimux --version`은 package version을 출력한다.
- **FR-CLI-3 [M]** `tuimux --doctor`는 tmux 설치/버전/TERM/truecolor/TMUX 여부를 점검한다.
- **FR-CLI-4 [M]** `tuimux --layout-preview`는 non-interactive preview를 출력한다.
- **FR-CLI-5 [M]** `tuimux --session <NAME>`은 native client target session을 지정한다.
- **FR-CLI-6 [M]** `tuimux --dashboard`는 hidden experimental option이며 기본 사용자 경로가 아니다.

### 2.2 Native tmux client

- **FR-NATIVE-1 [M]** 인자 없이 실행하면 target session 이름은 `tuimux`다.
- **FR-NATIVE-2 [M]** target session이 없으면 `tmux new-session -d -s <session>`을 실행한다.
- **FR-NATIVE-3 [M]** attach/switch 전 `tmux set-option -gq mouse on`을 실행한다.
- **FR-NATIVE-4 [M]** tmux 밖에서 실행하면 `tmux -u attach-session -t <session>`을 실행한다.
- **FR-NATIVE-5 [M]** tmux 안에서 실행하면 nested attach 대신 `tmux switch-client -t <session>`을 실행한다.
- **FR-NATIVE-6 [M]** default path는 crossterm raw mode, ratatui alternate screen, `capture-pane` renderer를 시작하지 않는다.
- **FR-NATIVE-7 [M]** tmux client의 exit status가 실패하면 tuimux도 실패로 종료한다.

### 2.3 Shell fidelity

- **FR-SHELL-1 [M]** `ls --color`, prompt, ANSI escape, cursor, resize는 tmux client/terminal에 그대로 맡긴다.
- **FR-SHELL-2 [M]** `nano`, `vim`, `less`, `htop` 같은 full-screen/alternate-screen 앱은 native tmux client 안에서 직접 동작해야 한다.
- **FR-SHELL-3 [M]** mouse wheel은 tmux mouse mode/copy-mode 동작을 따른다.
- **FR-SHELL-4 [M]** 한글/CJK/emoji width 처리는 `tmux -u`와 사용자의 terminal에 맡긴다.

---

## 3. tmux command 명세

```text
ensure session:
  tmux list-sessions -F '#{session_name}\t#{session_windows}\t#{session_attached}'
  tmux new-session -d -s <session>     # only if absent

mouse baseline:
  tmux set-option -gq mouse on

outside tmux:
  tmux -u attach-session -t <session>

inside tmux:
  tmux switch-client -t <session>
```

---

## 4. 인수 기준

- **AC-1** `cargo test --quiet`가 통과한다.
- **AC-2** argv unit test가 `set-option -gq mouse on`, `-u attach-session -t <session>`, `switch-client -t <session>`를 검증한다.
- **AC-3** `cargo run --quiet -- --version`은 `tuimux 0.1.7`을 출력한다.
- **AC-4** 실제 tmux sandbox에서 `tuimux --session <test>`가 session을 만들고 attach 가능해야 한다.
- **AC-5** sandbox session에서 `tmux show-option -gqv mouse`가 `on`이어야 한다.
- **AC-6** session에 보낸 `echo 한글-tuimux` 결과가 UTF-8로 보존되어 capture되어야 한다.
- **AC-7** `nano` 실행 시 native tmux client/screen 경로를 사용하므로 기존 ratatui snapshot renderer를 거치지 않는다.
- **AC-8** release workflow가 macOS arm64/x86_64 assets와 `SHA256SUMS`를 만든다.
- **AC-9** raw installer가 `v0.1.7` asset을 설치한다.

---

## 5. 위험 및 후속 과제

- custom sidebar UX는 v0.1.7에서 후퇴한 것이 아니라, shell 품질을 회복하기 위해 기본 경로에서 제외한 것이다.
- prefix-free UI는 다음 milestone에서 tmux popup/menu/binding 또는 full control-mode client 중 하나로 재도전해야 한다.
- 다시 `capture-pane` snapshot을 interactive shell로 승격하면 같은 문제가 반복된다.

---

## 6. 변경 이력

- **0.7 / 2026-06-08**: default interactive path를 real tmux native client로 전환. `mouse on`, `tmux -u attach-session`, inside-tmux `switch-client`, `--session` 옵션 명세 추가. capture/send shell emulation을 기본 경로에서 제거.
- **0.6 / 2026-06-08**: visible-screen capture, key repeat/release 차단, window close hover 보정.
- **0.5 / 2026-06-08**: capture-pane/send-keys 기반 main pane interaction 시도.
- **0.4 이하**: session/window command scaffold, compact UI preview, installer/release 구축.
