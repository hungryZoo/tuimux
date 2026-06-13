# Skill: paste highlight가 클릭으로 사라지지 않을 때 고치는 법

- **작성일**: 2026-06-14
- **적용 버전**: v0.2 native multiplexer branch
- **관련 파일**: `src/tui.rs`, `src/mux_backend.rs`, `src/terminal.rs`, `scripts/smoke_macos_ui_selection.py`

## 언제 이 스킬을 쓰는가

다음 증상이 보이면 shell paste highlight clear 경로를 먼저 의심한다.

- 붙여넣기 후 글자 배경이 흰색으로 남아 있다.
- 방향키를 누르면 흰 배경이 사라지는데 마우스 클릭으로는 사라지지 않는다.
- 특히 Copy 또는 Cut으로 clipboard에 들어간 텍스트를 context menu Paste로 붙인 뒤 우클릭/클릭에서 highlight가 남는다.
- raw bracketed paste smoke는 통과하는데 실제 context menu paste 후 클릭만 불안정하다.
- child가 mouse protocol 또는 application cursor mode를 켠 뒤 shell로 돌아온 상태에서만 클릭 clear가 실패한다.

## 핵심 원인

tuimux는 host terminal selection이 아니라 child shell의 bracketed paste highlight를 다룬다. paste 직후 terminal body left click은 “highlight를 지우는 버튼”이 아니라, clicked terminal cell로 active input cursor를 이동시키는 동작이어야 한다. zsh 같은 shell은 그 cursor 이동 과정에서 bracketed paste highlight를 자연스럽게 해제한다.

문제는 두 가지였다.

```text
context menu mouse/request
-> paste highlight click clear
-> terminal click/selection routing
```

이 순서에서는 paste 뒤 우클릭이 먼저 context menu request로 소비되어 `paste_highlight_pending` clear 경로까지 내려오지 못한다. 따라서 left click은 될 수 있어도 right click 또는 context-menu 중심 복붙 흐름에서는 흰 배경이 남을 수 있다.

또한 단순 `ESC[D ESC[C` 왕복은 clicked 위치와 무관해서 사용자가 기대하는 left click 의미와 다르다. click 좌표와 현재 cursor 좌표의 visual offset을 계산해 필요한 만큼 `ESC[D` 또는 `ESC[C`를 보내야 한다. 이 raw cursor movement는 application cursor/mouse protocol 상태에 의존하면 안 된다.

## 수정 원칙

paste 뒤 terminal body left click은 먼저 cursor move 후보로 처리한다.

```text
paste click cursor move
-> context menu mouse/request
-> terminal click/selection routing
```

구체적으로는 `src/tui.rs`에서 다음 정책을 유지한다.

- `Event::Mouse`를 받으면 context menu 처리보다 먼저 `should_clear_paste_highlight_for_click()`를 호출한다.
- terminal body left click이면 `terminal_cursor`, pane width, clicked local cell로 visual delta를 계산한다.
- delta가 음수면 raw `ESC[D`, 양수면 raw `ESC[C`를 반복해서 clicked cell로 input cursor를 옮긴다.
- delta가 0이면 fallback으로 `ESC[D ESC[C`를 보낼 수 있지만, 검증은 clicked cell로 실제 cursor가 이동하는 케이스를 반드시 포함한다.
- terminal body left click이 paste cursor move를 수행한 경우 그 click은 소비한다. 그렇지 않으면 같은 mouse up/down이 child로 전달되어 shell 입력줄에 mouse escape가 섞일 수 있다.
- terminal body 밖 UI chrome, rail, context menu 위 click은 clear 대상으로 본다.

## smoke test로 고정할 것

`scripts/smoke_macos_ui_selection.py`에는 raw paste만이 아니라 context menu paste 경로를 꼭 넣는다.

검증 순서:

1. editable text를 drag selection한다.
2. context menu Copy로 clipboard에 복사한 텍스트를 context menu Paste로 붙인 뒤 left click이 clear 입력을 보내는지 확인한다.
3. context menu Cut으로 clipboard에 복사하고 Backspace 삭제가 child까지 가는지 확인한다.
4. clipboard에 남은 Cut 텍스트를 context menu Paste로 붙인다.
5. probe가 흰 배경 paste text를 표시하고 대기하게 한다.
6. right click을 보낸다.
7. child가 clicked cell 방향으로 2회 이상의 cursor movement를 받는지 확인한다.
8. 실제 zsh prompt에서 paste 뒤 left click 후 reverse-video가 사라지고 final cursor position이 clicked column으로 이동하는지 확인한다.

이 테스트는 “paste는 됐지만 click clear가 context menu에서 먹힌다”는 회귀를 잡는다. 단순 raw bracketed paste key sequence만 보내는 테스트로는 context menu 처리 순서 버그를 놓칠 수 있다.

## 재발 방지 체크리스트

- paste 관련 변경 뒤에는 `paste_highlight_pending`이 어디서 켜지고 꺼지는지 먼저 추적한다.
- `send_terminal_key_to_child()`나 synthetic cursor movement가 pending 상태를 불필요하게 끄지 않는지 본다.
- mouse handler 순서를 바꿀 때 context menu, rail click, terminal body click 모두에서 click-clear가 먼저 실행되는지 확인한다.
- `should_clear_paste_highlight_for_click()`를 left click이나 mouse down 한 종류에만 묶지 않는다.
- child mouse protocol active 여부를 paste cursor move 조건으로 쓰지 않는다.
- clear를 `send_terminal_key_event(KeyCode::Left/Right)`로 보내지 않는다. application cursor mode가 켜져 있으면 바이트가 달라질 수 있으므로 raw `ESC[D`/`ESC[C`를 써야 한다.
- paste cursor move용 terminal-body left click은 `mouse up`까지 소비되는지 확인한다.
- smoke에는 raw paste path와 context menu paste path를 둘 다 둔다.

## 검증 루틴

기본 검증:

```sh
cargo fmt -- --check
cargo test --quiet
cargo build --quiet
uv run python -m py_compile scripts/smoke_macos_ui_selection.py
```

macOS PTY smoke:

```sh
uv run python scripts/smoke_macos_ui_selection.py
uv run python scripts/smoke_macos_paste_cursor_move.py
uv run python scripts/smoke_macos_mouse_protocol.py
uv run python scripts/smoke_macos_terminal_chrome.py
uv run python scripts/smoke_macos_scrollback.py
```

실제 shell visual 재현도 필요하다. zsh prompt에서 bracketed paste 후 `ESC[7m...` reverse-video가 생기고, terminal body left click 뒤 같은 payload가 reverse 없이 다시 그려지며 final cursor position이 clicked column으로 이동하고 mouse escape 찌꺼기가 입력줄에 남지 않아야 한다.

## 이번 수정의 결론

Cut/Copy로 복사한 텍스트를 붙인 뒤 흰 배경이 남은 것은 terminal emulator의 색 처리 문제가 아니라, left click을 cursor positioning이 아닌 highlight clear shortcut으로 모델링한 입력 라우팅 문제였다. paste 뒤 terminal body left click에서 clicked cell까지 raw cursor movement를 보내고 그 click을 소비하면, shell은 native terminal처럼 cursor를 이동시키면서 paste highlight를 해제한다.
