# Skill: 좋은 터미널처럼 copy, cut, paste, click, drag를 통합하는 법

- **작성일**: 2026-06-14
- **적용 버전**: v0.2 native multiplexer branch
- **관련 파일**: `src/tui.rs`, `src/mux_backend.rs`, `src/terminal.rs`, `scripts/smoke_macos_ui_selection.py`, `scripts/smoke_macos_paste_cursor_move.py`

## 언제 이 스킬을 쓰는가

다음 증상이 보이면 개별 shortcut이나 mouse handler를 따로 고치지 말고 전체 interaction model을 먼저 점검한다.

- 복사는 되는데 잘라내기가 삭제하지 않는다.
- 잘라낸 텍스트를 붙여넣은 뒤 흰 배경 paste highlight가 클릭으로 사라지지 않는다.
- 단축키 paste와 우클릭 menu Paste가 서로 다르게 동작한다.
- 클릭이 입력 커서를 움직이지 않고 selection 시작처럼만 동작한다.
- child mouse tracking이 켜진 앱 뒤에 일반 shell 클릭/드래그가 섞인다.
- context menu가 열려 있는 상태에서 Ctrl-V 같은 shortcut이 먹히거나 삼켜진다.

## 핵심 모델

tuimux의 목표 동작은 native terminal에 가깝게 다음 규칙으로 고정한다.

- **클릭**: terminal body left click은 입력 커서를 clicked cell로 이동한다.
- **선택 해제 클릭**: 이미 selection이 있는 상태의 plain left click은 selection만 해제하고 child에는 cursor movement나 mouse event를 보내지 않는다.
- **드래그**: left down 뒤 drag가 발생한 경우에만 tuimux text selection을 만든다.
- **키보드 선택**: active input cursor가 보이는 terminal mode에서 plain Shift+방향키는 text editor처럼 selection을 확장한다. 이 선택은 mouse drag와 달리 cursor boundary 기반이다.
- **우클릭**: host terminal menu 대신 TUI context menu를 열고 Cut, Copy, Paste, Cancel을 제공한다.
- **복사**: mouse menu, Ctrl-C, Ctrl-Shift-C, Cmd-Shift-C, Win-Shift-C 모두 같은 selected text extraction과 system clipboard write를 사용한다.
- **잘라내기**: copy 후 editable input-line selection이면 cursor 이동과 Backspace로 child 입력줄에서 삭제한다. 편집 불가능한 화면 selection이면 copy 후 selection만 해제한다.
- **붙여넣기**: host paste event, raw bracketed paste, Ctrl-V/Ctrl-Shift-V/Cmd-Shift-V/Win-Shift-V, context menu Paste 모두 active PTY paste로 들어간다.
- **선택 대체**: editable selection이 있는 상태에서 Backspace/Delete/일반 문자/Ctrl-V paste가 들어오면 먼저 selection을 삭제하고, 필요한 경우 replacement input을 보낸다.

## 구현 포인트

`src/tui.rs`에서 interaction을 다음 흐름으로 유지한다.

1. Left mouse down은 즉시 selection으로 확정하지 않고 `pending_left_down`에 저장한다.
2. Pending 상태에서 drag가 오면 `begin_selection()` 후 `update_selection()`으로 selection을 시작한다.
3. Pending 상태에서 mouse up이 오면 drag가 아니므로 click이다. 기존 selection을 해제하는 click이면 여기서 끝낸다. 그 외에는 child mouse protocol이 켜져 있으면 down/up을 child로 전달하고, 아니면 `move_input_cursor_to_cell()`로 input cursor를 이동한다.
4. Paste는 `send_terminal_paste()`, `paste_clipboard()`, context menu Paste 모두 `paste_text()`로 모은다.
5. `paste_text()`는 `terminal_mode = true`, `delete_selection_for_replacement()`, `mux.send_paste(text)`, `paste_highlight_pending = true` 순서로 처리한다.
6. Context menu가 열려 있어도 copy/cut/paste shortcut은 같은 action으로 처리한다. 메뉴가 shortcut을 삼키게 두면 “붙여넣기가 안 된다”는 회귀가 생긴다.
7. Raw `^C`/`^V` byte도 crossterm `KeyModifiers::CONTROL` 이벤트와 같은 shortcut으로 본다. pseudo terminal smoke에서는 이 형태가 직접 들어온다.
8. Paste 뒤 terminal body left click은 `handle_paste_highlight_mouse()`에서 먼저 처리해 clicked cell 방향 raw cursor movement를 보내고 mouse event 누수를 막는다.
9. Shift+방향키는 `SelectionKind::KeyboardBoundary`로 저장한다. drag selection의 inclusive cell 좌표를 재사용하면 Shift+Left 한 번에 커서 왼쪽 글자와 오른쪽 글자가 함께 잡히거나, 한 글자 selection을 표현하지 못하는 회귀가 생긴다.
10. Cmd/Win 계열 shortcut은 crossterm에서 `SUPER`, `META`, `HYPER` 중 하나로 들어올 수 있으므로 `launcher_modifier_bits()`로 묶어 처리한다. terminal이 지원하면 keyboard enhancement의 disambiguate flag로 이 modifier들을 더 안정적으로 받는다.

## keyboard selection 규칙

키보드 선택은 화면 cell이 아니라 cursor boundary 사이의 범위다.

- `anchor`와 `focus`를 `row * pane_width + col` boundary index로 바꾼 뒤, 작은 값부터 큰 값 직전 cell까지만 `SelectionRange`로 변환한다.
- Shift+Left 한 번은 `focus = cursor - 1`이므로 선택 range가 `cursor - 1` 한 cell이어야 한다.
- Shift+Right 한 번은 `focus = cursor + 1`이므로 선택 range가 `cursor` 한 cell이어야 한다.
- Shift+Up/Down은 active pane width만큼 focus boundary를 움직인다. PTY에는 Up/Down key를 보내지 말고 raw `ESC[D`/`ESC[C` 반복으로 cursor를 움직여 shell history가 바뀌지 않게 한다.
- KeyboardBoundary selection 삭제/대체는 snapshot의 `terminal_cursor`가 한 frame 늦을 수 있으므로 실제 편집 cursor를 `selection.focus`로 계산한다.
- cursor가 숨겨져 있으면 fullscreen child app일 가능성이 높으므로 Shift+방향키를 tuimux selection으로 가로채지 않는다.

## soft wrap selection 규칙

화면 줄이 자동 wrap된 입력줄을 드래그할 때는 grid row 단위가 아니라 logical text selection으로 처리한다.

- selected text extraction은 `vt100::Screen::contents_between()`을 사용한다. 이 API는 `row_wrapped()`가 true인 행 사이에는 `\n`을 넣지 않고, 명시적 줄바꿈 행 사이에만 newline을 보존한다.
- selection end가 오른쪽 빈 padding까지 가도 trailing blank cell은 clipboard text에 들어가지 않는다.
- selection highlight는 선택 범위 때문에 빈 padding row/col을 새로 렌더링하지 않는다. 실제 contents나 non-default style이 있는 cell 범위 안에서만 reverse-video가 보인다.
- editable selection 삭제는 `(row * pane_width + col)` 선형 좌표로 계산한다. 따라서 한 입력줄이 두 화면줄에 걸쳐도 선택 텍스트를 화면 좌표 위에서 걸어간 끝 위치까지 cursor를 맞춘 뒤 Backspace를 보낸다.
- `selected_text`에 실제 `\n`이 포함되어도 편집 대상으로 다룬다. newline은 삭제해야 할 문자 1개이자 “다음 화면 행의 col 0으로 이동”으로 해석한다. 그래서 사용자가 임의로 여러 줄을 드래그한 뒤 Backspace/Delete/문자/Cut/Paste를 누르면 텍스트 에디터처럼 선택 범위가 먼저 삭제된다.

## 테스트로 고정할 것

필수 smoke:

```sh
cargo fmt -- --check
cargo test --quiet -- --test-threads=1
cargo build --quiet
uv run python scripts/smoke_macos_ui_selection.py
uv run python scripts/smoke_macos_paste_cursor_move.py
uv run python scripts/smoke_macos_mouse_protocol.py
uv run python scripts/smoke_macos_terminal_chrome.py
uv run python scripts/smoke_macos_scrollback.py
```

`scripts/smoke_macos_ui_selection.py`가 반드시 검증해야 하는 항목:

- drag selection이 mouse-up 이후 reverse-video로 남는다.
- right-click menu Copy와 Ctrl-C가 같은 텍스트를 `pbpaste`로 남긴다.
- right-click menu Cut은 clipboard copy 후 editable selection이면 Backspace 삭제 입력을 child로 보낸다.
- Backspace, Delete, 일반 문자 입력이 editable selection을 삭제하거나 대체한다.
- Ctrl-V paste가 shell 명령으로 실행된다.
- Ctrl-V paste가 editable selection을 먼저 삭제하고 replacement text를 넣는다.
- Ctrl-Shift-X cut이 editable selection을 clipboard로 복사하고 line buffer에서는 삭제한다.
- Shift-Left keyboard selection이 실제 pseudo terminal 입력에서 editable target을 선택하고 Backspace로 삭제한다.
- soft wrap된 editable selection을 빈 칸까지 드래그해도 replacement가 target만 대체한다.
- hard newline이 포함된 editable selection을 Backspace로 지우면 newline까지 포함해 target만 삭제된다.
- context menu Paste가 child bracketed paste wrapper와 paste highlight clear 경로를 보존한다.
- Copy/Cut으로 얻은 clipboard text를 붙여넣은 뒤 left click이 paste highlight를 지우고 cursor movement를 보낸다.

`scripts/smoke_macos_paste_cursor_move.py`가 반드시 검증해야 하는 항목:

- 실제 zsh prompt에서 paste 직후 reverse-video highlight가 관측된다.
- terminal body left click 후 highlight가 사라진다.
- 최종 cursor column이 clicked column으로 이동한다.
- paste가 아닌 일반 left click도 입력 커서를 clicked column으로 이동한다.
- mouse escape byte가 shell 입력줄에 남지 않는다.

## line editor probe 설계

텍스트 편집 기능은 child가 받은 byte 개수만 보면 안 된다. `scripts/smoke_macos_ui_selection.py`의 editable selection probe는 raw mode child 안에 작은 line editor를 두고 다음을 직접 시뮬레이션한다.

- 초기 buffer: `prefix + target + suffix`
- cursor: line 끝
- `ESC[D`/`ESCOD`: cursor left
- `ESC[C`/`ESCOC`: cursor right
- `DEL`/Backspace: cursor 앞 글자 삭제
- printable byte: cursor 위치에 insert
- `ESC[3~`: cursor 위치 글자 delete

최종 buffer가 정확히 `prefix + replacement + suffix`일 때만 OK를 출력한다. 예를 들어 Backspace/Delete/Ctrl-Shift-X는 replacement가 빈 문자열이므로 `prefix + suffix`가 되어야 하고, 일반 문자와 Ctrl-V paste는 각각 입력된 replacement가 target 자리를 차지해야 한다. 이 방식은 “Backspace가 target 길이만큼 왔다” 같은 약한 검증보다 강하다.

## 회귀 방지 체크리스트

- mouse handler 순서를 바꿨다면 pending left down, paste highlight clear, context menu, child mouse forwarding 순서를 다시 검토한다.
- paste 경로를 새로 추가했다면 `paste_text()`를 우회하지 않는다.
- selection 삭제/대체를 고쳤다면 Backspace/Delete/문자/Ctrl-V 네 경로를 모두 테스트한다.
- Shift+방향키를 고쳤다면 한 글자 keyboard selection, soft-wrap을 가로지르는 keyboard range, editable delete pipeline 연결을 단위 테스트한다.
- soft wrap 선택을 고쳤다면 copy path의 `selected_text_from_screen()` 단위 테스트와 editable replacement smoke를 같이 확인한다.
- context menu를 고쳤다면 shortcut key가 메뉴에 삼켜지지 않는지 테스트한다.
- Cmd/Win shortcut을 고쳤다면 `SUPER`, `META`, `HYPER` + Shift의 C/V/X/Left/Right 단위 테스트를 같이 갱신한다.
- raw terminal output에서 문자열이 ratatui diff로 조각날 수 있으므로 smoke는 화면 byte 연속성만 믿지 말고 child file/probe 결과도 사용한다.
- child mouse protocol이 켜진 상태에서 simple click forwarding과 normal drag selection이 동시에 통과해야 한다.

## 이번 수정의 결론

문제는 clipboard나 terminal color 자체가 아니라 interaction 경로가 여러 갈래로 흩어진 데 있었다. click, drag, context menu, shortcut, host paste를 하나의 규칙으로 다시 묶고, paste replacement와 paste-highlight clear를 같은 `paste_text()` 이후 흐름으로 고정해야 native terminal 같은 감각이 유지된다.
