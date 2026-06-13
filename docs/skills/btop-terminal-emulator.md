# Skill: btop이 깨질 때 terminal emulator를 고치는 법

- **작성일**: 2026-06-13
- **적용 버전**: v0.2.0-alpha.35
- **관련 파일**: `src/terminal.rs`, `src/tui.rs`, `scripts/smoke_macos_apps.py`, `scripts/smoke_macos_terminal_chrome.py`

## 언제 이 스킬을 쓰는가

다음 증상이 보이면 UI layout보다 terminal emulator/parser 쪽을 먼저 의심한다.

- `btop` 화면이 줄 단위로 흘러가듯 깨진다.
- cpu/proc panel이 제 위치에 그려지지 않고 이전 줄 뒤에 이어 붙는다.
- PTY 크기는 충분한데도 full-screen 앱이 native terminal처럼 보이지 않는다.
- `mouse_protocol_active=true`인데 화면 모델의 rows가 이미 깨져 있다.

## 핵심 원인

이번 btop 문제는 rail 크기나 top/bottom bar만의 문제가 아니었다. btop은 raw output에서 `CSI row;col f` 형태의 HVP(horizontal and vertical position) sequence를 많이 사용했다.

```text
ESC [ 1 ; 1 f
ESC [ 8 ; 1 f
ESC [ 2 ; 1 f
```

터미널 의미상 `CSI row;col f`는 `CSI row;col H` cursor-position과 동일하게 처리되어야 한다. 그런데 `vt100` parser에 그대로 넘기면 이 sequence가 기대대로 반영되지 않아, btop의 절대 위치 렌더링이 무시되고 output이 순차 출력처럼 누적될 수 있었다.

## 수정한 것

### 1. PTY 입력 정규화

`src/terminal.rs`의 `PtyTerminal::drain()`에서 PTY byte stream을 parser에 바로 넘기지 않고 `normalize_terminal_input()`을 거치게 했다.

```rust
let bytes = normalize_terminal_input(&bytes, &mut self.pending_parser_bytes);
self.parser.process(&bytes);
```

정규화 규칙은 좁게 유지한다.

- `ESC [`로 시작하는 CSI sequence만 본다.
- final byte가 `f`이면 `H`로 바꾼다.
- 다른 CSI sequence, OSC sequence, UTF-8 텍스트는 건드리지 않는다.
- escape sequence가 chunk 경계에서 잘리면 `pending_parser_bytes`에 보관했다가 다음 chunk와 이어서 처리한다.

### 2. 회귀 테스트 추가

`src/terminal.rs`에 다음 단위 테스트를 추가했다.

- `terminal_input_normalizes_hvp_cursor_position`
- `terminal_input_keeps_incomplete_csi_between_chunks`
- `terminal_input_leaves_other_csi_sequences_unchanged`

이 테스트들은 `CSI 2;3 f`가 `CSI 2;3 H`로 바뀌고, 실제 parser screen의 row 2 / col 3 위치에 글자가 찍히는지 확인한다.

### 3. boxed rail 복구

`src/tui.rs`의 terminal mode rail을 다시 boxed 형태로 복구했다.

- `Detach` button block
- `WINDOWS` block
- window rows, 3-cell ` X ` close button, `+ new`, STATUS panel, `scroll:<count>`
- Session panel/picker는 제품 UX에서 제거됐다.

다만 top/bottom status bar와 compact top-tab fallback은 되살리지 않았다. child PTY가 rail을 제외한 body 전체를 받고, 넓은 화면에서도 최소 80 columns를 유지해야 btop 같은 앱이 안정적이다.

## 디버깅 절차

1. 먼저 PTY 크기 문제인지 확인한다.
   - 80, 100, 120 columns에서 snapshot을 비교한다.
   - 크기를 키워도 rows가 이미 깨져 있으면 parser/emulator 문제일 가능성이 높다.

2. raw PTY output을 본다.
   - btop 시작 직후 `ESC[?1049h`, `ESC[?25l`, mouse mode enable sequence가 있는지 본다.
   - `ESC[row;col f`가 반복되는지 확인한다.

3. daemon snapshot을 본다.
   - raw output은 정상인데 snapshot rows가 깨져 있으면 renderer보다 screen model/parser 쪽 문제다.

4. parser에 들어가기 전 byte stream을 최소 정규화한다.
   - terminal 의미가 같은 sequence만 바꾼다.
   - 앱별 문자열 patch를 넣지 않는다.

## 검증 루틴

기본 검증:

```sh
PATH=/Users/heonzoo/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH cargo fmt -- --check
PATH=/Users/heonzoo/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH cargo test --quiet
PATH=/Users/heonzoo/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH cargo build --release --quiet
```

macOS smoke:

```sh
python3 scripts/smoke_macos_apps.py --binary target/debug/tuimux --timeout 12
python3 scripts/smoke_macos_terminal_chrome.py --binary target/debug/tuimux --timeout 12
python3 scripts/smoke_macos_resize.py --binary target/debug/tuimux --timeout 12
```

btop snapshot에서 다음을 확인한다.

- `mouse_protocol_active: true`
- 첫 화면 상단에 `cpu` panel이 있고 border glyph가 함께 보인다.
- `proc` panel이 정상 위치에 보인다.
- rows가 process list나 braille graph fragment로 순차 wrapping된 모양이 아니다.

## 재발 방지 체크리스트

- full-screen 앱이 깨지면 먼저 terminal body size와 parser snapshot을 분리해서 본다.
- raw PTY output에 있는 CSI/OSC sequence를 앱 이름 기준으로 patch하지 않는다.
- `CSI f`, `CSI H`처럼 표준상 동등한 sequence는 parser 입력 정규화로 처리한다.
- chunk 경계에서 escape sequence가 잘리는 케이스를 테스트에 넣는다.
- rail/chrome을 바꿀 때 child PTY 최소 80 columns를 유지한다.
- terminal mode rail을 바꾸면 `scripts/smoke_macos_terminal_chrome.py`의 mouse click 좌표와 status text clipping도 같이 점검한다.

## 이번 수정의 결론

btop이 깨진 직접 원인은 “가짜 터미널처럼 보이는 UI”가 아니라, terminal emulator가 btop의 cursor-position sequence 일부를 제대로 screen model에 반영하지 못한 것이었다. `CSI row;col f`를 `CSI row;col H`와 동일하게 처리한 뒤 btop은 daemon snapshot과 실제 TUI smoke에서 정상 렌더링됐다.
