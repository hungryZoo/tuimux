# tuimux PRD (Product Requirements Document)

- **문서 버전**: 0.7 (Draft)
- **작성일**: 2026-06-08
- **상태**: v0.1.7 tmux-native 재설계 기준
- **프로젝트명**: tuimux
- **한 줄 요약**: tmux를 대체하거나 흉내 내지 않고, tmux의 실제 client UX 위에 prefix 부담을 줄이는 최소 도구를 얹는 Rust 기반 wrapper.

---

## 1. 문제 재정의

v0.1.5~v0.1.6의 `capture-pane` + `send-keys` 방식은 실제 shell처럼 보이려 했지만, 사용자가 체감한 결과는 “shell을 따라 하는 가짜 화면”이었다.

실제 문제:

- `ls` 출력/색/커서/폭이 자연스럽지 않다.
- `nano`, `vim`, `less` 같은 alternate-screen/full-screen 앱이 깨진다.
- mouse wheel/copy-mode 흐름이 tmux보다 못하다.
- 한글/CJK wide character width와 터미널 escape semantics를 ratatui text snapshot으로 정확히 재현하기 어렵다.

결론: **tuimux는 terminal emulator가 아니다.** tmux의 철학을 따르면 shell, PTY, alternate screen, mouse, UTF-8/CJK 처리는 tmux client와 terminal이 맡아야 한다.

---

## 2. 제품 원칙

1. **tmux-native first**: 기본 실행은 실제 `tmux -u attach-session` 또는 tmux 내부 `switch-client`다.
2. **No fake shell**: 기본 UX에서 `capture-pane` polling과 `send-keys` replay로 shell을 흉내 내지 않는다.
3. **Mouse-on baseline**: tuimux는 최소한 `tmux set-option -gq mouse on`을 보장해 tmux 기본 mouse/copy-mode 경험보다 나빠지지 않아야 한다.
4. **UTF-8/CJK 보존**: `tmux -u` native client 경로로 한글/이모지/CJK width를 tmux와 terminal에 맡긴다.
5. **Full-screen apps work**: nano/vim/less/htop 같은 앱은 tmux client에서 직접 동작해야 한다.
6. **0.x honest scope**: 아직 custom sidebar overlay/control-mode UI가 완성되지 않았으면 기본 shell 경험을 망가뜨리는 UI를 기본값으로 두지 않는다.

---

## 3. 목표

- **G1. 실제 shell 품질 회복**: `ls`, `clear`, `nano`, mouse wheel, 한글 입력/출력이 일반 tmux client 수준으로 동작한다.
- **G2. tmux 지속성 유지**: session, window, detach/attach는 tmux가 담당한다.
- **G3. prefix 부담 완화의 기반 마련**: v0.1.7은 안정적인 native client를 기본값으로 하고, 이후 tmux control-mode 또는 tmux popup 기반 UI를 단계적으로 추가한다.
- **G4. 부끄럽지 않은 MVP**: tmux mouse-on보다 못한 shell 모조품을 기본 실행 경로에서 제거한다.

---

## 4. 비목표

- v0.1.7에서 custom right sidebar를 실제 shell 위에 완성하지 않는다.
- v0.1.7에서 자체 terminal emulator, PTY multiplexer, VT parser를 구현하지 않는다.
- v0.1.7에서 `capture-pane` renderer를 기본 interactive shell로 사용하지 않는다.
- tmux 설정 전체를 tuimux가 장악하지 않는다. 단, mouse on은 제품 baseline으로 설정한다.

---

## 5. v0.1.7 사용자 경험

### 기본 실행

```sh
tuimux
```

동작:

1. tmux 설치/버전을 확인한다.
2. 기본 session `tuimux`가 없으면 `tmux new-session -d -s tuimux`로 만든다.
3. `tmux set-option -gq mouse on`을 적용한다.
4. tmux 밖이면 `tmux -u attach-session -t tuimux`를 실행한다.
5. tmux 안이면 nested attach를 하지 않고 `tmux switch-client -t tuimux`를 실행한다.

### session 지정

```sh
tuimux --session dev
```

`dev` session을 만들거나 attach/switch한다.

### preview/doctor

- `tuimux --doctor`: 환경 진단.
- `tuimux --layout-preview`: 향후 UI 방향을 non-interactive text로 확인.
- 숨겨진 `--dashboard`: 이전 ratatui dashboard prototype. 기본값이 아니며 실제 shell 용도로 권장하지 않는다.

---

## 6. 향후 UX 방향

v0.1.7 이후 prefix-free UX는 다음 중 하나로만 진행한다.

### Option A. tmux control-mode client

- `tmux -CC` protocol을 사용한다.
- pane `%output` byte stream을 VT parser로 렌더한다.
- 입력/mouse는 protocol에 맞게 전달한다.
- 장점: tmux session/window/pane backend를 유지하면서 custom UI 가능.

### Option B. tmux popup/native bindings

- 기본 shell은 tmux client 그대로 둔다.
- Session picker, window close, new session 같은 조작은 tmux popup/menu/bindings로 제공한다.
- 장점: tmux 철학에 가장 가깝고 shell 품질이 절대 깨지지 않는다.

둘 중 어느 방향이든, `capture-pane` snapshot을 interactive terminal로 쓰는 방식은 폐기한다.

---

## 7. 성공 기준

- `script`/PTY에서 `tuimux --session <test>` 실행 후 실제 tmux client escape sequence가 나타난다.
- session 안에서 `ls`, `echo 한글`, `clear`, `nano`가 tmux client 기준으로 동작한다.
- `tmux show-option -gqv mouse`가 `on`이다.
- `tuimux --version`은 `0.1.7`을 출력한다.
- release installer로 macOS artifacts를 설치할 수 있다.

---

## 8. 변경 이력

- **0.7 / 2026-06-08**: shell emulation 폐기. 기본 실행을 real tmux native client attach/switch로 재설계. mouse on, UTF-8 native client, nano/ls/wheel/CJK 보존을 핵심 기준으로 변경.
- **0.6 / 2026-06-08**: visible screen capture와 key repeat 차단으로 부분 보정했으나 architecture 한계가 남음.
- **0.5 / 2026-06-08**: `capture-pane`/`send-keys` 기반 interactive shell 시도.
- **0.4 이하**: compact right sidebar, session/window command scaffold, installer/release 기반 구축.
