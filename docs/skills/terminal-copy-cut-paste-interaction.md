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
- **드래그**: left down 뒤 drag가 발생한 경우에만 tuimux text selection을 만든다.
- **우클릭**: host terminal menu 대신 TUI context menu를 열고 Cut, Copy, Paste, Cancel을 제공한다.
- **복사**: mouse menu, Ctrl-C, Ctrl-Shift-C, Cmd-Shift-C 모두 같은 selected text extraction과 system clipboard write를 사용한다.
- **잘라내기**: copy 후 editable input-line selection이면 cursor 이동과 Backspace로 child 입력줄에서 삭제한다. 편집 불가능한 화면 selection이면 copy 후 selection만 해제한다.
- **붙여넣기**: host paste event, raw bracketed paste, Ctrl-V/Ctrl-Shift-V/Cmd-Shift-V, context menu Paste 모두 active PTY paste로 들어간다.
- **선택 대체**: editable selection이 있는 상태에서 Backspace/Delete/일반 문자/Ctrl-V paste가 들어오면 먼저 selection을 삭제하고, 필요한 경우 replacement input을 보낸다.

## 구현 포인트

`src/tui.rs`에서 interaction을 다음 흐름으로 유지한다.

1. Left mouse down은 즉시 selection으로 확정하지 않고 `pending_left_down`에 저장한다.
2. Pending 상태에서 drag가 오면 `begin_selection()` 후 `update_selection()`으로 selection을 시작한다.
3. Pending 상태에서 mouse up이 오면 drag가 아니므로 click이다. child mouse protocol이 켜져 있으면 down/up을 child로 전달하고, 아니면 `move_input_cursor_to_cell()`로 input cursor를 이동한다.
4. Paste는 `send_terminal_paste()`, `paste_clipboard()`, context menu Paste 모두 `paste_text()`로 모은다.
5. `paste_text()`는 `terminal_mode = true`, `delete_selection_for_replacement()`, `mux.send_paste(text)`, `paste_highlight_pending = true` 순서로 처리한다.
6. Context menu가 열려 있어도 copy/cut/paste shortcut은 같은 action으로 처리한다. 메뉴가 shortcut을 삼키게 두면 “붙여넣기가 안 된다”는 회귀가 생긴다.
7. Raw `^C`/`^V` byte도 crossterm `KeyModifiers::CONTROL` 이벤트와 같은 shortcut으로 본다. pseudo terminal smoke에서는 이 형태가 직접 들어온다.
8. Paste 뒤 terminal body left click은 `handle_paste_highlight_mouse()`에서 먼저 처리해 clicked cell 방향 raw cursor movement를 보내고 mouse event 누수를 막는다.

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
- context menu를 고쳤다면 shortcut key가 메뉴에 삼켜지지 않는지 테스트한다.
- raw terminal output에서 문자열이 ratatui diff로 조각날 수 있으므로 smoke는 화면 byte 연속성만 믿지 말고 child file/probe 결과도 사용한다.
- child mouse protocol이 켜진 상태에서 simple click forwarding과 normal drag selection이 동시에 통과해야 한다.

## 이번 수정의 결론

문제는 clipboard나 terminal color 자체가 아니라 interaction 경로가 여러 갈래로 흩어진 데 있었다. click, drag, context menu, shortcut, host paste를 하나의 규칙으로 다시 묶고, paste replacement와 paste-highlight clear를 같은 `paste_text()` 이후 흐름으로 고정해야 native terminal 같은 감각이 유지된다.
