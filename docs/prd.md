# tuimux PRD (Product Requirements Document)

- **문서 버전**: 0.8 (Draft)
- **작성일**: 2026-06-08
- **상태**: v0.1.8 TUI 기본값 복구 기준
- **프로젝트명**: tuimux
- **한 줄 요약**: tmux backend를 쓰되, 사용자가 실행하면 반드시 tuimux의 prefix-free/mouse-first TUI가 보이는 Rust 기반 wrapper.

---

## 1. 문제 재정의

v0.1.7은 `capture-pane` + `send-keys` shell emulation의 한계를 피하려고 기본 실행을 real tmux client attach로 바꿨다. 그러나 그 결과 사용자가 기대한 tuimux 제품 UI가 사라지고 “그냥 tmux가 실행되는” 치명적인 회귀가 발생했다.

실제 제품 문제:

- `tuimux`를 실행하면 tuimux TUI chrome/sidebar가 보여야 한다.
- tmux-native 품질도 중요하지만, 아무 TUI 없이 tmux만 띄우면 tuimux의 존재 이유가 없다.
- 0.x MVP는 아직 완벽한 terminal emulator가 아니더라도, 제품 기본값은 tuimux UI여야 한다.

결론: **default는 tuimux TUI, plain tmux client는 opt-in fallback**이어야 한다.

---

## 2. 제품 원칙

1. **TUI first**: 인자 없이 `tuimux`를 실행하면 항상 tuimux ratatui UI가 열린다.
2. **tmux backend 유지**: session/window/detach/create/close는 실제 tmux 명령으로 수행한다.
3. **정직한 0.x 품질**: main pane snapshot/input bridge의 한계를 숨기지 않되, UI 자체를 제거하지 않는다.
4. **No surprise plain tmux**: plain `tmux attach` 같은 화면은 명시적 fallback 옵션에서만 실행한다.
5. **Mouse-first UX**: right sidebar, session dialog, window close, new window/session 조작은 mouse-first로 유지한다.
6. **향후 개선 방향**: shell fidelity는 control-mode renderer 또는 tmux-native popup/binding 통합으로 개선한다.

---

## 3. 목표

- **G1. 제품 UI 복구**: `tuimux` 기본 실행 시 Session/Detach/WINDOWS sidebar와 session dialog가 보인다.
- **G2. 실 tmux 조작 유지**: session/window 생성, 전환, close는 fake state가 아니라 tmux server에 반영된다.
- **G3. 회귀 방지**: 테스트로 기본 run mode가 plain native client가 아님을 고정한다.
- **G4. fallback 보존**: plain tmux client 경로는 hidden `--native-client` 옵션으로만 남긴다.

---

## 4. 비목표

- v0.1.8에서 완전한 VT parser/control-mode terminal renderer를 완성하지 않는다.
- v0.1.8에서 plain tmux client를 기본 UX로 삼지 않는다.
- shell fidelity 문제를 “TUI 제거”로 해결하지 않는다.

---

## 5. v0.1.8 사용자 경험

### 기본 실행

```sh
tuimux
```

동작:

1. tmux 설치/버전을 확인한다.
2. ratatui alternate screen을 열고 tuimux TUI를 표시한다.
3. session이 없으면 기본 session `tuimux`를 생성한다.
4. 오른쪽 sidebar에 현재 session, Detach, windows, `+ new`를 표시한다.
5. session dialog는 header 없이 열리며 session 선택, `New Session`, `Detach`를 제공한다.
6. main pane 클릭 시 terminal input mode로 들어가며 F12로 navigation mode로 돌아온다.

### fallback

```sh
tuimux --native-client --session dev
```

명시적으로 요청한 경우에만 plain tmux client attach/switch 경로를 실행한다.

### preview/doctor

- `tuimux --doctor`: 환경 진단.
- `tuimux --layout-preview`: non-interactive text layout preview.
- `tuimux --version`: package version 출력.

---

## 6. 향후 UX 방향

v0.1.8 이후 main pane fidelity는 다음 중 하나로 개선한다.

### Option A. tmux control-mode client

- `tmux -CC` protocol을 사용한다.
- pane `%output` byte stream을 VT parser로 렌더한다.
- 입력/mouse는 protocol에 맞게 전달한다.

### Option B. tmux popup/native bindings

- 기본 shell 품질은 tmux client에 맡기되, picker/menu/sidebar에 해당하는 UX를 tmux popup/menu/bindings로 제공한다.

---

## 7. 성공 기준

- `cargo test --quiet`가 통과한다.
- `cargo run --quiet -- --version`은 `tuimux 0.1.8`을 출력한다.
- `cargo run --quiet -- --layout-preview`에 `Session`, `Detach`, `WINDOWS`, `New Session`이 나타난다.
- PTY smoke test에서 인자 없는 `tuimux` 실행 시 plain tmux attach escape가 아니라 tuimux UI 문자열이 나타난다.
- `--native-client`는 opt-in으로만 plain tmux client를 실행한다.
- release installer로 macOS v0.1.8 artifacts를 설치할 수 있다.

---

## 8. 변경 이력

- **0.8 / 2026-06-08**: v0.1.7 default plain tmux client 회귀를 수정. default를 ratatui tuimux TUI로 복구하고 plain native client는 hidden `--native-client` fallback으로 이동.
- **0.7 / 2026-06-08**: shell emulation 한계를 피하려고 default를 real tmux native client로 변경했으나, 제품 UI가 사라지는 회귀를 만들었음.
- **0.6 / 2026-06-08**: visible screen capture와 key repeat 차단으로 부분 보정.
- **0.5 / 2026-06-08**: `capture-pane`/`send-keys` 기반 interactive shell 시도.
- **0.4 이하**: compact right sidebar, session/window command scaffold, installer/release 기반 구축.
