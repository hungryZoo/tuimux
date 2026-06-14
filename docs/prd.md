# tuimux PRD

- **문서 버전**: 4.0
- **대상 릴리스**: v0.2.0-alpha.39
- **작성일**: 2026-06-13

## 1. 제품 방향

tuimux는 prefix를 외우지 않고 mouse-first로 다룰 수 있는 terminal multiplexer다. v0.2.0-alpha.39의 기본 실행 경로는 tmux wrapper가 아니라 Rust-native daemon-backed multiplexer이며, 하나의 persistent mux 안에서 window list와 PTY를 tuimux daemon이 직접 소유한다.

tmux는 안정적이지만 사용자가 원하는 native selection, clipboard, mouse, visual fidelity를 tuimux UI 안에서 세밀하게 제어하기 어렵다. 따라서 tmux C 코드는 참고하되, tuimux runtime은 Rust로 직접 구현한다.

제품 UX는 split pane이 아니라 terminal mode의 오른쪽 window rail과 navigation mode의 오른쪽 window 목록에서 active terminal window를 골라 쓰는 방식이다. split pane은 default UI와 daemon protocol에서 deprecated이며 native mux core에서도 layout state를 갖지 않는다.

## 2. 사용자 문제

- terminal pane이 “진짜 터미널”처럼 느껴지지 않으면 full-screen 앱이 깨진다.
- mouse drag 후 선택이 사라지면 복사 흐름이 macOS Terminal과 다르게 느껴진다.
- 우클릭 복사/붙여넣기가 TUI 안에서 끊기면 shell 사용 중 복붙 흐름이 계속 마찰을 만든다.
- 여러 화면줄에 걸친 입력 선택이 Backspace/Delete/문자 입력으로 지워지지 않으면 텍스트 에디터처럼 쓸 수 없다.
- Ctrl-C가 항상 child program으로 전달되면 선택 텍스트 복사와 process interrupt가 충돌한다.
- UI를 닫을 때 shell/window state까지 사라지면 multiplexer로 믿고 쓰기 어렵다.
- shell이 `exit`로 종료됐는데 window가 stale 화면으로 남으면 실제 terminal이 아니라 frozen preview처럼 느껴진다.
- alternate-screen 앱이 종료된 뒤 잔상이나 alternate-screen text가 primary scrollback에 남으면 native terminal과 다르게 느껴진다.
- child 앱이 terminal title이나 OSC 52 clipboard copy/query를 쓰는데 UI가 무시하면 window list와 복붙 흐름이 native terminal보다 둔하게 느껴진다.
- 한 화면을 여러 pane으로 나누는 방식은 현재 목표가 아니며, window 목록 중심 UX가 더 단순하고 안정적이다.
- 기본 화면에서 tuimux chrome이 보이지 않으면 사용자는 그냥 shell만 감싼 wrapper처럼 느끼고, window 조작이 어디 있는지 알 수 없다.
- tmux 설정이나 runtime 설치가 필수이면 tuimux 자체 제품 경험을 제어하기 어렵다.

## 3. 릴리스 목표

- 기본 `tuimux` 실행에서 tmux 없이 native shell window를 연다.
- daemon이 단일 window list와 PTY를 소유하고 UI client는 Unix socket으로 attach한다.
- UI detach/종료 후 다시 attach하면 shell state가 유지된다.
- 같은 daemon에 여러 client가 동시에 연결될 수 있다.
- navigation mode에서 오른쪽 window 목록을 보고 `Tab`/arrow key로 window를 전환하고, `n`/`x`로 window를 만들고 닫을 수 있다.
- child가 OSC 0/1/2로 terminal title을 설정하면 오른쪽 window 목록에 해당 title을 표시한다.
- child shell이 자체 종료되면 non-last window는 목록에서 제거하고, 마지막 window는 새 shell로 대체한다.
- legacy split pane hotkey는 core state를 바꾸지 않는다.
- terminal mode는 넓은 화면에서 boxed 오른쪽 rail만 표시하고 위/아래 status bar는 만들지 않는다. 좁은 화면의 compact top tab fallback은 잠시 비활성화해 `btop`, `htop`, `nano` 같은 앱에 정직한 PTY 크기를 준다.
- terminal emulator는 btop이 쓰는 `CSI row;col f` HVP cursor-position sequence를 절대 위치 이동으로 처리한다.
- terminal mode에서도 `Alt-N`, `Alt-Left`/`Alt-Right`로 window 작업을 할 수 있다.
- shell scrollback을 mouse wheel, `PageUp`/`PageDown`, `Home`, `End`로 볼 수 있다.
- mouse selection은 mouse-up 이후 유지되며 선택된 텍스트는 daemon이 active PTY screen에서 추출한다.
- terminal body left click은 drag가 아니면 active input cursor를 clicked cell로 이동하고, drag는 selection을 만든다.
- active input cursor가 보일 때 plain Shift+방향키는 text editor처럼 keyboard selection을 확장한다. Shift+Up/Down은 shell history가 아니라 현재 terminal body 폭 기준의 시각적 줄 이동으로 동작한다.
- selection이 있을 때 Ctrl-C/Ctrl-Shift-C/Cmd-Shift-C/Win-Shift-C는 system clipboard copy로 동작하고, selection이 없을 때 plain Ctrl-C는 child interrupt로 남는다.
- Ctrl-V/Ctrl-Shift-V/Cmd-Shift-V/Win-Shift-V는 system clipboard paste로 동작한다.
- terminal mode의 Home/End와 Cmd-Shift-Left/Cmd-Shift-Right/Win-Shift-Left/Win-Shift-Right는 input line start/end 이동으로 동작한다.
- 우클릭은 TUI context menu를 열고 Cut, Copy, Paste, Cancel을 제공한다.
- Cmd-Shift-X/Win-Shift-X는 editable selection이면 선택 텍스트를 system clipboard에 복사한 뒤 커서를 선택 끝으로 이동하고 Backspace로 삭제하며, 삭제할 수 없는 출력 화면 선택이면 복사 후 tuimux selection을 해제한다.
- editable selection이 있을 때 Backspace/Delete는 선택 영역을 삭제하고, 일반 문자 입력과 Ctrl-V paste는 선택 영역을 삭제한 뒤 replacement text를 입력한다. 이 동작은 soft wrap과 hard newline을 포함한 여러 줄 선택에서도 유지되어야 하며, 빈칸까지 드래그한 tail은 삭제 대상이 아니다.
- child의 OSC 52 clipboard copy 요청은 macOS system clipboard로 이어지고, paste query는 clipboard text를 PTY response로 돌려받는다.
- host paste는 paste event 또는 raw bracketed-paste key sequence로 처리한다.
- 붙여넣기 직후 쉘이 표시한 paste highlight는 다음 일반 mouse click에서 드래그 선택 해제처럼 사라진다. down/up 이벤트 종류와 child mouse/application-cursor mode와 무관해야 하며, 우클릭 context menu 요청도 이 clear 경로보다 먼저 paste highlight를 지워야 한다.
- child가 mouse tracking을 켠 경우 simple left click과 wheel은 child로 보내고 normal drag는 tuimux selection으로 쓴다.
- child가 명시적으로 출력한 truecolor foreground/background/default reset은 부모 환경의 `NO_COLOR`와 무관하게 native terminal color로 보존한다.
- host terminal resize는 active child PTY까지 전달되어 full-screen 앱과 shell이 새 rows/cols를 관측한다.
- terminal row는 viewport 폭까지 명시적으로 렌더해 이전 frame의 긴 줄 glyph가 다음 frame에 남지 않게 한다.
- alternate-screen output은 active 상태에서만 보이고 종료 후 primary screen/scrollback으로 누수되지 않는다.
- macOS Apple Silicon 프리릴리즈를 먼저 배포한다.

## 4. 비목표

- daemon 재시작/host reboot 후 window 복구.
- tmux layout string, tmux command 호환성, plugin 호환성.
- split-pane UX.
- client별 독립 cursor/viewport 정책.
- Windows/Linux asset은 이번 프리릴리즈에 포함하지 않는다.

## 5. 성공 기준

- `cargo fmt -- --check`와 `cargo test --quiet` 통과.
- macOS ARM release build 성공.
- macOS terminal-chrome smoke에서 기본 화면 boxed rail, child body 실행, rail `+ new` click, `F12` handoff가 통과한다.
- `tuimux --doctor`가 tmux 부재를 실패로 보지 않는다.
- detach 후 reattach smoke에서 shell 환경값이 유지된다.
- navigation mode window 전환/생성/종료가 split-pane 조작 대신 동작한다.
- native mux core에 split layout tree가 남지 않고 window마다 terminal body를 채우는 single PTY pane만 유지된다.
- scrollback daemon regression test가 통과한다.
- host bracketed paste setup/restore가 적용되어 paste event가 active pane으로 전달된다.
- 열린 client가 있는 상태에서 두 번째 client가 snapshot/window/scrollback command를 수행하고 세 번째 client가 shutdown할 수 있다.
- `btop`, `htop`, `nano`, `llmfit --help`가 native terminal surface에서 실행된다.
- daemon snapshot에서 btop의 cpu/proc panel과 mouse protocol state가 정상으로 관측된다.
- drag selection이 mouse-up 이후 화면에 reverse-video highlight로 남고 Ctrl-C/Ctrl-Shift-C/Cmd-Shift-C/Win-Shift-C copy shortcut 정책과 `pbpaste` smoke test가 통과한다.
- Ctrl-V/Ctrl-Shift-V/Cmd-Shift-V/Win-Shift-V paste shortcut, editable selection 위 Ctrl-V replacement, Shift+방향키 keyboard selection, Home/End/Cmd-Shift-Left/Cmd-Shift-Right/Win-Shift-Left/Win-Shift-Right line-boundary shortcut regression test가 통과한다.
- 붙여넣기 뒤 일반 mouse click이 쉘의 paste highlight pending 상태를 해제하고 기존 drag selection/context menu 흐름을 깨지 않는다. terminal body left click은 clear 전용으로 소비되어 child 입력줄에 mouse escape를 남기지 않는다.
- UI selection lifecycle과 daemon selected-text/highlight regression test가 통과한다.
- macOS PTY UI smoke에서 drag selection, right-click context menu Cut/Copy, Cut의 Backspace child 전달, Backspace/delete/text/Ctrl-V replacement editable selection, Ctrl-C clipboard copy, foreground child SIGINT 미전달, context menu Paste와 Ctrl-V Paste 실행, child bracketed paste wrapper 보존이 통과한다.
- macOS window-flow smoke에서 detach/reattach shell state 유지와 window-list workflow가 통과한다.
- macOS no-tmux smoke에서 tmux 없는 PATH의 default TUI/doctor 성공과 `--native-client` 실패가 통과한다.
- `--layout-preview`가 split-pane/resize 샘플이 아닌 terminal body + window-list preview를 출력한다.
- macOS mouse-protocol smoke에서 child SGR mouse tracking 중 normal left click forwarding과 normal drag selection이 통과한다.
- macOS scrollback smoke에서 실제 TUI의 mouse wheel, `PageUp`, `Home`, `End` active terminal history navigation과 scrollback 중 paste bottom 복귀가 통과한다.
- macOS truecolor smoke에서 `NO_COLOR=1` 부모 환경에서도 child `38;2`/`48;2` SGR과 default reset이 실제 TUI output에 보존된다.
- macOS resize smoke에서 host PTY resize 후 child가 `SIGWINCH`와 integrated rail을 제외한 새 `32x100` terminal body size를 관측한다.
- macOS alternate-screen smoke와 daemon regression에서 alternate-screen active/exit, primary screen 복귀, primary scrollback 격리가 통과한다.
- macOS child-exit smoke에서 마지막 shell 종료 후 replacement shell이 명령을 받고, non-last shell 종료 후 window list에서 제거된다.
- macOS window-title smoke에서 child OSC title이 오른쪽 window list에 표시된다.
- macOS OSC 52 clipboard smoke에서 child copy 요청 후 `pbpaste`가 요청한 텍스트를 반환한다.
- macOS OSC 52 paste smoke에서 child paste query가 macOS clipboard text를 PTY response로 돌려받는다.
- GitHub prerelease에 macOS Apple Silicon tarball과 `SHA256SUMS`가 게시된다.

## 6. 다음 단계

다음 큰 제품 단계는 daemon restart persistence, Linux/Windows backend, client별 독립 viewport/cursor 정책, 더 넓은 terminal app compatibility suite다.
