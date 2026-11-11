//! Test-only ANSI screen oracle: inspect the real TUI's PTY output, not raw
//! byte substrings (retained rendering may leave matching cells untouched).
use std::io::{self, Read};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let cols: u16 = args
        .next()
        .ok_or("usage: screen-oracle COLS ROWS")?
        .parse()?;
    let rows: u16 = args
        .next()
        .ok_or("usage: screen-oracle COLS ROWS")?
        .parse()?;
    let mut bytes = Vec::new();
    io::stdin().read_to_end(&mut bytes)?;
    let mut parser = vt100::Parser::new(rows, cols, 0);
    // Replaying a resized PTY at only its final dimensions can corrupt wrap
    // metadata. Apply captured host resize boundaries at their byte offsets.
    let mut offset = 0;
    if let Some(resizes) = args.next() {
        let resizes: Vec<(usize, u16, u16)> = serde_json::from_str(&resizes)?;
        if resizes.len() > 4096 {
            return Err("too many resize boundaries".into());
        }
        for (at, cols, rows) in resizes {
            if at < offset || at > bytes.len() || cols == 0 || rows == 0 {
                return Err("invalid resize boundary".into());
            }
            parser.process(&bytes[offset..at]);
            parser.screen_mut().set_size(rows, cols);
            offset = at;
        }
    }
    parser.process(&bytes[offset..]);
    println!("{}", parser.screen().contents());
    Ok(())
}
