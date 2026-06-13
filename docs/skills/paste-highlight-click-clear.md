# Skill: paste highlight가 클릭으로 사라지지 않을 때 고치는 법

- **작성일**: 2026-06-14
- **적용 버전**: v0.2 native multiplexer branch
- **관련 파일**: `src/tui.rs`, `scripts/smoke_macos_ui_selection.py`

## 언제 이 스킬을 쓰는가

다음 증상이 보이면 shell paste highlight clear 경로를 먼저 의심한다.

- 붙여넣기 후 글자 배경이 흰색으로 남아 있다.
- 방향키를 누르면 흰 배경이 사라지는데 마우스 클릭으로는 사라지지 않는다.
- 특히 Copy 또는 Cut으로 clipboard에 들어간 텍스트를 context menu Paste로 붙인 뒤 우클릭/클릭에서 highlight가 남는다.
- raw bracketed paste smoke는 통과하는데 실제 context menu paste 후 클릭만 불안정하다.

## 핵심 원인

tuimux는 host terminal selection이 아니라 child shell의 bracketed paste highlight를 지우기 위해 `paste_highlight_pending`을 둔다. paste 직후 다음 mouse click에서 active PTY에 `Left`/`Right` key를 보내면 zsh 같은 shell이 paste highlight를 해제한다.

문제는 mouse event 처리 순서였다.

```text
context menu mouse/request
-> paste highlight click clear
-> terminal click/selection routing
```

이 순서에서는 paste 뒤 우클릭이 먼저 context menu request로 소비되어 `paste_highlight_pending` clear 경로까지 내려오지 못한다. 따라서 left click은 될 수 있어도 right click 또는 context-menu 중심 복붙 흐름에서는 흰 배경이 남을 수 있다.

## 수정 원칙

mouse down/up은 먼저 paste highlight clear 후보로 처리한다.

```text
paste highlight click clear
-> context menu mouse/request
-> terminal click/selection routing
```

구체적으로는 `src/tui.rs`에서 다음 정책을 유지한다.

- `Event::Mouse`를 받으면 context menu 처리보다 먼저 `should_clear_paste_highlight_for_click()`를 호출한다.
- clear 대상은 `MouseEventKind::Down(_)`와 `MouseEventKind::Up(_)` 전체다. left click뿐 아니라 right click도 native terminal의 “다른 곳 클릭하면 highlight 해제” 감각에 맞게 clear해야 한다. 터미널/트랙패드 조합에 따라 down/up 중 한쪽만 관측되는 경우에도 clear가 살아야 한다.
- terminal body 안에서 child mouse protocol이 켜진 pane은 기존처럼 clear를 건너뛴다. mouse app이 직접 mouse input을 쓰는 중이면 tuimux가 cursor key를 끼워 넣으면 안 된다.
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
7. child가 `ESC[D ESC[C` 또는 application cursor mode의 `ESCOD ESCOC`를 받는지 확인한다.

이 테스트는 “paste는 됐지만 click clear가 context menu에서 먹힌다”는 회귀를 잡는다. 단순 raw bracketed paste key sequence만 보내는 테스트로는 context menu 처리 순서 버그를 놓칠 수 있다.

## 재발 방지 체크리스트

- paste 관련 변경 뒤에는 `paste_highlight_pending`이 어디서 켜지고 꺼지는지 먼저 추적한다.
- `send_terminal_key_to_child()`나 synthetic cursor movement가 pending 상태를 불필요하게 끄지 않는지 본다.
- mouse handler 순서를 바꿀 때 context menu, rail click, terminal body click 모두에서 click-clear가 먼저 실행되는지 확인한다.
- `should_clear_paste_highlight_for_click()`를 left click이나 mouse down 한 종류에만 묶지 않는다.
- child mouse protocol active pane에는 clear cursor key를 보내지 않는다.
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
uv run python scripts/smoke_macos_mouse_protocol.py
uv run python scripts/smoke_macos_terminal_chrome.py
uv run python scripts/smoke_macos_scrollback.py
```

## 이번 수정의 결론

Cut으로 복사한 텍스트를 붙인 뒤 흰 배경이 남은 것은 terminal emulator의 색 처리 문제가 아니라, paste highlight clear가 context menu mouse event보다 늦게 실행되거나 click 이벤트 종류를 너무 좁게 본 입력 라우팅 문제였다. mouse down/up에서 먼저 clear를 시도하고 right click도 clear 대상으로 포함하자, Copy/Cut clipboard context paste 후 클릭에서도 child가 cursor key clear sequence를 받게 됐다.
