use ferro_services::line_text;

use super::*;

fn text(snapshot: &TerminalSnapshot, row: usize) -> String {
    line_text(&snapshot.lines[row])
}

/// The text of absolute line `line`, scrolling there and back.
fn line_at(core: &mut TerminalCore, line: i64) -> String {
    core.scroll_to_line(line);
    let snapshot = core.snapshot();
    let row = line - snapshot.first_line;
    core.scroll(i32::MIN);
    assert!(
        (0..snapshot.rows as i64).contains(&row),
        "line {line} not reachable (first line {})",
        snapshot.first_line
    );
    text(&snapshot, row as usize)
}

/// A prompt, a command and its output with shell-integration marks.
fn command(name: &str, output: &[&str], code: i32) -> String {
    let mut s = format!("\x1b]133;A\x07$ \x1b]133;B\x07{name}\r\n\x1b]133;C\x07");
    for line in output {
        s.push_str(line);
        s.push_str("\r\n");
    }
    s.push_str(&format!("\x1b]133;D;{code}\x07"));
    s
}

#[test]
fn text_and_colors_become_spans() {
    let mut core = TerminalCore::new(80, 5, 100);
    core.advance(
        b"\x1b[1;31mred\x1b[0m plain \x1b[38;5;200mix\x1b[38;2;1;2;3mrgb\x1b[0m\r\n\
          \x1b[2;3;4mdim\x1b[22;23;24;7minv\x1b[0m\x1b[93;104mbright\x1b[0m",
    );
    let snapshot = core.snapshot();
    assert_eq!((snapshot.cols, snapshot.rows), (80, 5));
    assert_eq!(snapshot.lines.len(), 5);
    let line = &snapshot.lines[0];
    assert_eq!(line_text(line), "red plain ixrgb");
    assert_eq!(line[0].text, "red");
    assert_eq!(line[0].fg, AnsiColor::Red);
    assert!(line[0].bold);
    assert_eq!(line[1].text, " plain ");
    assert_eq!(line[1].fg, AnsiColor::Default);
    assert!(!line[1].bold);
    assert_eq!(line[2].fg, AnsiColor::Indexed(200));
    assert_eq!(line[3].fg, AnsiColor::Rgb(1, 2, 3));

    let line = &snapshot.lines[1];
    assert_eq!(line[0].text, "dim");
    assert!(line[0].dim && line[0].italic && line[0].underline);
    assert_eq!(line[1].text, "inv");
    assert!(line[1].inverse && !line[1].dim && !line[1].underline);
    assert_eq!(line[2].text, "bright");
    assert_eq!(line[2].fg, AnsiColor::BrightYellow);
    assert_eq!(line[2].bg, AnsiColor::BrightBlue);
    // Empty rows have no spans.
    assert!(snapshot.lines[4].is_empty());
}

#[test]
fn trailing_blanks_are_trimmed_unless_colored() {
    let mut core = TerminalCore::new(20, 3, 100);
    core.advance(b"x   \r\ny\x1b[41m  \x1b[0m   \r\n\x1b[4m  \x1b[0m");
    let snapshot = core.snapshot();
    assert_eq!(text(&snapshot, 0), "x");
    let line = &snapshot.lines[1];
    assert_eq!(line_text(line), "y  ");
    assert_eq!(line[1].bg, AnsiColor::Red);
    assert_eq!(text(&snapshot, 2), "  ");
    assert!(snapshot.lines[2][0].underline);
}

#[test]
fn wide_characters_take_one_char_and_hidden_text_is_blank() {
    let mut core = TerminalCore::new(20, 2, 100);
    core.advance("a中b😀c\r\n\x1b[8msecret\x1b[0m!".as_bytes());
    let snapshot = core.snapshot();
    assert_eq!(text(&snapshot, 0), "a中b😀c");
    assert_eq!(snapshot.lines[0].len(), 1);
    assert_eq!(text(&snapshot, 1), "      !");
    // The cursor counts cells: after "      !" it is in column 7.
    let cursor = snapshot.cursor.unwrap();
    assert_eq!((cursor.row, cursor.col), (1, 7));
}

#[test]
fn combining_characters_stay_with_their_base() {
    let mut core = TerminalCore::new(20, 1, 100);
    core.advance("e\u{301}x".as_bytes());
    assert_eq!(text(&core.snapshot(), 0), "e\u{301}x");
}

#[test]
fn title_bell_and_replies() {
    let mut core = TerminalCore::new(40, 5, 100);
    let out = core.advance(b"\x1b]0;build\x07\x07\x1b]2;build\x07");
    assert_eq!(
        out.events,
        [CoreEvent::Title(Some("build".into())), CoreEvent::Bell]
    );
    assert_eq!(core.snapshot().title.as_deref(), Some("build"));

    // Device status report: the answer must go back to the program.
    let out = core.advance(b"ab\x1b[6n");
    assert_eq!(out.replies, b"\x1b[1;3R");
    let out = core.advance(b"\x1b[c");
    assert!(out.replies.starts_with(b"\x1b[?"));
    // Palette queries are answered, the theme's colors are not.
    let out = core.advance(b"\x1b]4;1;?\x07");
    assert!(String::from_utf8_lossy(&out.replies).contains("rgb:cdcd/0000/0000"));
}

#[test]
fn cursor_shape_visibility_and_modes() {
    let mut core = TerminalCore::new(40, 5, 100);
    let snapshot = core.snapshot();
    assert_eq!(snapshot.cursor.unwrap().shape, CursorShape::Block);
    assert_eq!(snapshot.modes, TerminalModes::default());

    core.advance(b"\x1b[6 q");
    assert_eq!(core.snapshot().cursor.unwrap().shape, CursorShape::Bar);
    core.advance(b"\x1b[4 q");
    assert_eq!(
        core.snapshot().cursor.unwrap().shape,
        CursorShape::Underline
    );
    core.advance(b"\x1b[?25l");
    assert_eq!(core.snapshot().cursor.unwrap().shape, CursorShape::Hidden);
    core.advance(b"\x1b[?25h");

    core.advance(b"\x1b[?1h\x1b[?2004h\x1b[?1000h");
    let modes = core.snapshot().modes;
    assert!(modes.app_cursor && modes.bracketed_paste && modes.mouse_reporting);
    assert!(!modes.alt_screen);
    core.advance(b"\x1b[?1049hfull screen");
    let snapshot = core.snapshot();
    assert!(snapshot.modes.alt_screen);
    assert_eq!(text(&snapshot, 0), "full screen");
    core.advance(b"\x1b[?1049l\x1b[?1l\x1b[?2004l\x1b[?1000l");
    assert_eq!(core.snapshot().modes, TerminalModes::default());
}

#[test]
fn scrolling_the_view_hides_the_cursor_and_moves_first_line() {
    let mut core = TerminalCore::new(20, 3, 100);
    for i in 0..10 {
        core.advance(format!("line {i}\r\n").as_bytes());
    }
    let snapshot = core.snapshot();
    assert_eq!(snapshot.history_len, 8);
    assert_eq!(snapshot.first_line, 8);
    assert_eq!(text(&snapshot, 0), "line 8");
    assert!(snapshot.cursor.is_some());

    core.scroll(3);
    let snapshot = core.snapshot();
    assert_eq!(snapshot.display_offset, 3);
    assert_eq!(snapshot.first_line, 5);
    assert_eq!(text(&snapshot, 0), "line 5");
    assert_eq!(snapshot.cursor, None);

    core.scroll_to_line(0);
    let snapshot = core.snapshot();
    assert_eq!(snapshot.first_line, 0);
    assert_eq!(text(&snapshot, 0), "line 0");
    core.scroll(i32::MIN);
    assert_eq!(core.snapshot().display_offset, 0);
    // Out of range is clamped.
    core.scroll_to_line(-5);
    assert_eq!(core.snapshot().first_line, 0);
    core.scroll_to_line(1000);
    assert_eq!(core.snapshot().display_offset, 0);
}

#[test]
fn shell_integration_records_command_blocks() {
    let mut core = TerminalCore::new(40, 10, 100);
    assert!(!core.prompt_ready());
    let out = core.advance(command("ls", &["a.txt", "b.txt"], 0).as_bytes());
    assert_eq!(out.events, [CoreEvent::CommandFinished(Some(0))]);
    assert!(core.prompt_ready());
    let out = core.advance(command("false", &[], 1).as_bytes());
    assert_eq!(out.events, [CoreEvent::CommandFinished(Some(1))]);
    core.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07");

    let snapshot = core.snapshot();
    assert_eq!(
        snapshot.commands,
        [
            CommandBlock {
                prompt_line: 0,
                output_line: Some(1),
                exit_code: Some(0),
                finished: true
            },
            CommandBlock {
                prompt_line: 3,
                output_line: Some(4),
                exit_code: Some(1),
                finished: true
            },
            CommandBlock {
                prompt_line: 4,
                output_line: None,
                exit_code: None,
                finished: false
            },
        ]
    );
    assert_eq!(text(&snapshot, 0), "$ ls");
    assert_eq!(text(&snapshot, 1), "a.txt");
    assert_eq!(text(&snapshot, 3), "$ false");
    // The marks themselves leave nothing on screen.
    assert_eq!(text(&snapshot, 4), "$");
}

#[test]
fn marks_split_across_reads_give_the_same_blocks() {
    let input = format!(
        "{}{}",
        command("make", &["cc main.c", "done"], 2),
        "\x1b]133;A\x07$ \x1b]133;B\x07"
    );
    let mut whole = TerminalCore::new(40, 10, 100);
    whole.advance(input.as_bytes());
    let expected = whole.snapshot();

    for chunk in [1, 2, 3, 5, 7, 11] {
        let mut core = TerminalCore::new(40, 10, 100);
        let mut events = Vec::new();
        for piece in input.as_bytes().chunks(chunk) {
            events.extend(core.advance(piece).events);
        }
        assert_eq!(core.snapshot(), expected, "chunks of {chunk}");
        assert_eq!(events, [CoreEvent::CommandFinished(Some(2))]);
    }
}

#[test]
fn a_new_prompt_ends_an_unfinished_command_and_duplicates_are_merged() {
    let mut core = TerminalCore::new(40, 10, 100);
    // Ctrl+C: no `D`, the next prompt ends the command.
    core.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07sleep 9\r\n\x1b]133;C\x07^C\r\n");
    // The prompt marked twice (a theme and our script).
    let out = core.advance(b"\x1b]133;A\x07\x1b]133;A\x07$ \x1b]133;B\x07");
    assert!(out.events.is_empty());
    let commands = core.snapshot().commands;
    assert_eq!(commands.len(), 2);
    assert!(commands[0].finished);
    assert_eq!(commands[0].exit_code, None);
    assert_eq!(commands[1].prompt_line, 2);
    // A `D` without a code (cmd-style) finishes it without one.
    let out = core.advance(b"\x1b]133;D\x07");
    assert_eq!(out.events, [CoreEvent::CommandFinished(None)]);
}

#[test]
fn absolute_lines_survive_scrolling_and_the_scrollback_limit() {
    let scrollback = 50;
    let mut core = TerminalCore::new(20, 5, scrollback);
    let mut expected = Vec::new();
    for i in 0..40 {
        // Outputs of varied length, some in one big read that scrolls more
        // than the slack at once.
        let lines: Vec<String> = (0..(i % 4) * 3 + if i == 20 { 600 } else { 0 })
            .map(|n| format!("out {i}.{n}"))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        core.advance(command(&format!("cmd{i}"), &refs, i).as_bytes());
        expected.push(format!("$ cmd{i}"));
    }
    let snapshot = core.snapshot();
    assert_eq!(snapshot.history_len, scrollback);
    // Only commands whose prompt is still in the scrollback are kept.
    assert!(!snapshot.commands.is_empty());
    assert!(snapshot.commands.len() < 40);
    let first = 40 - snapshot.commands.len();
    for (block, i) in snapshot.commands.clone().iter().zip(first..) {
        assert!(block.prompt_line >= 0);
        assert_eq!(block.exit_code, Some(i as i32));
        assert_eq!(line_at(&mut core, block.prompt_line), expected[i]);
        let output = block.output_line.unwrap();
        if i % 4 != 0 {
            assert_eq!(line_at(&mut core, output), format!("out {i}.0"));
        }
    }
}

#[test]
fn a_huge_single_read_keeps_marks_exact() {
    let mut core = TerminalCore::new(10, 4, 100);
    let mut input = command("first", &[], 0);
    // Long lines wrap: 3 000 bytes at 10 columns scroll 300 lines, without
    // a single line feed.
    input.push_str(&"x".repeat(3000));
    input.push_str("\r\n");
    input.push_str(&command("second", &["ok"], 0));
    core.advance(input.as_bytes());
    let snapshot = core.snapshot();
    assert_eq!(snapshot.commands.len(), 1);
    let block = snapshot.commands[0];
    assert_eq!(line_at(&mut core, block.prompt_line), "$ second");
    assert_eq!(line_at(&mut core, block.output_line.unwrap()), "ok");
}

#[test]
fn clearing_the_history_drops_its_commands() {
    let mut core = TerminalCore::new(20, 3, 100);
    for i in 0..5 {
        core.advance(command(&format!("c{i}"), &["x", "y"], 0).as_bytes());
    }
    core.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07");
    assert_eq!(core.snapshot().commands.len(), 6);
    // `clear`: screen and history.
    core.advance(b"\x1b[H\x1b[2J\x1b[3J");
    let snapshot = core.snapshot();
    assert_eq!(snapshot.history_len, 0);
    // The live prompt stays (it moved to history with the screen clear and
    // was then dropped too, or stayed on screen); nothing negative remains.
    assert!(snapshot.commands.iter().all(|b| b.prompt_line >= 0));
    assert!(snapshot.commands.len() <= 1);
    core.advance(command("after", &["z"], 0).as_bytes());
    let snapshot = core.snapshot();
    let block = *snapshot.commands.last().unwrap();
    assert_eq!(line_at(&mut core, block.prompt_line), "$ after");
}

#[test]
fn resizing_keeps_marks_on_their_lines() {
    let mut core = TerminalCore::new(30, 10, 100);
    for i in 0..3 {
        core.advance(command(&format!("c{i}"), &["out"], 0).as_bytes());
    }
    core.resize(30, 4);
    core.resize(30, 12);
    core.resize(30, 6);
    let snapshot = core.snapshot();
    assert_eq!((snapshot.cols, snapshot.rows), (30, 6));
    for (i, block) in snapshot.commands.iter().enumerate() {
        assert_eq!(line_at(&mut core, block.prompt_line), format!("$ c{i}"));
    }
    // The alternate screen does not disturb them either.
    core.advance(b"\x1b[?1049h");
    core.resize(30, 8);
    core.advance(b"\x1b[?1049l");
    let snapshot = core.snapshot();
    for (i, block) in snapshot.commands.iter().enumerate() {
        assert_eq!(line_at(&mut core, block.prompt_line), format!("$ c{i}"));
    }
}

#[test]
fn synchronized_updates_are_flushed() {
    let mut core = TerminalCore::new(20, 3, 100);
    core.advance(b"\x1b[?2026hhidden");
    assert!(core.sync_deadline().is_some());
    assert_eq!(text(&core.snapshot(), 0), "");
    core.flush_sync();
    assert_eq!(text(&core.snapshot(), 0), "hidden");
    // A mark inside an update shows what came before it first.
    core.advance(b"\x1b[?2026hab\r\n\x1b]133;A\x07$ ");
    assert_eq!(core.snapshot().commands[0].prompt_line, 1);
}

#[test]
fn exit_is_shown() {
    let mut core = TerminalCore::new(20, 3, 100);
    assert_eq!(core.snapshot().exited, None);
    core.set_exited(3);
    assert_eq!(core.snapshot().exited, Some(3));
}
