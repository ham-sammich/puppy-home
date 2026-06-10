//! Validates the vt100 screen-parsing path that the terminal grid renderer
//! consumes: text placement, SGR colors, and cursor movement.
//!
//! (The live PTY/ConPTY spawn is exercised by `portable-pty` upstream and only
//! works in a real interactive Windows session — a headless agent sandbox has no
//! console host, so we don't spawn a process here.)

#[test]
fn vt100_parses_text_color_and_cursor() {
    let mut p = vt100::Parser::new(5, 20, 0);

    // "Hi" then a red "X" (SGR 31), then reset.
    p.process(b"Hi\x1b[31mX\x1b[0m");
    let s = p.screen();
    assert_eq!(s.cell(0, 0).unwrap().contents(), "H");
    assert_eq!(s.cell(0, 1).unwrap().contents(), "i");
    let x = s.cell(0, 2).unwrap();
    assert_eq!(x.contents(), "X");
    assert_eq!(
        x.fgcolor(),
        vt100::Color::Idx(1),
        "SGR 31 → red (palette idx 1)"
    );
    assert_eq!(
        s.cursor_position(),
        (0, 3),
        "cursor advances past written cells"
    );

    // Absolute cursor move (CUP) to row 3, col 5 (1-based) then write.
    p.process(b"\x1b[3;5HZ");
    let s = p.screen();
    assert_eq!(s.cell(2, 4).unwrap().contents(), "Z");
    assert_eq!(s.cursor_position(), (2, 5));

    // Truecolor background (SGR 48;2;r;g;b) is preserved on the cell.
    p.process(b"\x1b[48;2;10;20;30mB");
    let s = p.screen();
    let b = s.cell(2, 5).unwrap();
    assert_eq!(b.contents(), "B");
    assert_eq!(b.bgcolor(), vt100::Color::Rgb(10, 20, 30));
}
