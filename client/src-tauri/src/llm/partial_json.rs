//! Pulls the first `"text"` string out of a JSON answer while it's still being written, so a
//! hint can be shown word by word before the JSON is complete. Escapes may be cut anywhere
//! between pieces (`\` in one piece, `n` in the next; half a `\u` code).

#[derive(Default)]
pub struct TextField {
    text: String,
    done: bool,
    in_string: bool,
    /// The string being read is the `"text"` value.
    target: bool,
    /// Outside strings: a `"text"` key and its colon were just read.
    armed: bool,
    /// The last string read, while it could still turn out to be a key.
    last_string: Option<String>,
    other: String,
    escape: Escape,
    high_surrogate: Option<u32>,
}

#[derive(Default)]
enum Escape {
    #[default]
    None,
    Backslash,
    Unicode(String),
}

impl TextField {
    /// Adds the next piece of the JSON. Returns the text so far when this piece added to it.
    pub fn push(&mut self, piece: &str) -> Option<&str> {
        let before = self.text.len();
        for c in piece.chars() {
            if self.done {
                break;
            }
            if self.in_string {
                self.in_string_char(c);
            } else {
                self.outside_char(c);
            }
        }
        (self.text.len() > before).then_some(self.text.as_str())
    }

    /// The text so far (all of it once the closing quote has arrived).
    #[cfg(test)]
    pub fn text(&self) -> &str {
        &self.text
    }

    fn outside_char(&mut self, c: char) {
        match c {
            '"' => {
                self.in_string = true;
                self.target = std::mem::take(&mut self.armed);
            }
            ':' => self.armed = self.last_string.take().as_deref() == Some("text"),
            c if c.is_whitespace() => {}
            _ => {
                self.last_string = None;
                self.armed = false;
            }
        }
    }

    fn in_string_char(&mut self, c: char) {
        match std::mem::take(&mut self.escape) {
            Escape::Backslash => match c {
                'u' => self.escape = Escape::Unicode(String::new()),
                'n' => self.emit('\n'),
                't' => self.emit('\t'),
                'r' => self.emit('\r'),
                'b' => self.emit('\u{8}'),
                'f' => self.emit('\u{c}'),
                other => self.emit(other), // \" \\ \/
            },
            Escape::Unicode(mut hex) => {
                hex.push(c);
                if hex.len() < 4 {
                    self.escape = Escape::Unicode(hex);
                    return;
                }
                let code = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFD);
                match (code, self.high_surrogate.take()) {
                    (0xD800..=0xDBFF, _) => self.high_surrogate = Some(code),
                    (0xDC00..=0xDFFF, Some(high)) => {
                        let joined = 0x10000 + ((high - 0xD800) << 10) + (code - 0xDC00);
                        self.emit(char::from_u32(joined).unwrap_or('\u{FFFD}'));
                    }
                    (code, _) => self.emit(char::from_u32(code).unwrap_or('\u{FFFD}')),
                }
            }
            Escape::None => match c {
                '\\' => self.escape = Escape::Backslash,
                '"' => {
                    self.in_string = false;
                    if self.target {
                        self.done = true;
                    } else {
                        self.last_string = Some(std::mem::take(&mut self.other));
                    }
                }
                c => self.emit(c),
            },
        }
    }

    fn emit(&mut self, c: char) {
        if self.target {
            self.text.push(c);
        } else {
            self.other.push(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fed(pieces: &[&str]) -> String {
        let mut field = TextField::default();
        for piece in pieces {
            field.push(piece);
        }
        field.text().to_string()
    }

    #[test]
    fn the_text_grows_as_pieces_arrive() {
        let mut field = TextField::default();
        assert_eq!(field.push(r#"{"hints":[{"te"#), None);
        assert_eq!(field.push(r#"xt":"Ask a"#), Some("Ask a"));
        assert_eq!(field.push("bout"), Some("Ask about"));
        assert_eq!(
            field.push(r#" budget","type":"question","text":"no"}]}"#),
            Some("Ask about budget")
        );
        assert_eq!(field.push("more"), None, "done at the closing quote");
        assert_eq!(field.text(), "Ask about budget");
    }

    #[test]
    fn escapes_cut_between_pieces_are_decoded() {
        assert_eq!(
            fed(&[
                r#"{"text":"line\"#,
                r#"nnext \"quoted\" \\ caf\u00"#,
                r#"e9"}"#
            ]),
            "line\nnext \"quoted\" \\ café"
        );
        assert_eq!(
            fed(&[r#"{"text":"smile \ud83d"#, r#"\ude00!"}"#]),
            "smile 😀!"
        );
    }

    #[test]
    fn only_a_text_key_counts() {
        assert_eq!(
            fed(&[r#"{"type":"text","context":"x","text":"real"}"#]),
            "real"
        );
        assert_eq!(fed(&[r#"{"hints":[]}"#]), "");
    }
}
