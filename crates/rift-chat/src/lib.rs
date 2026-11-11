//! The look of Rift's messages to operators and players: a small MiniMessage-style markup turned
//! into the engine's `Text`, and the two-line command error.
//!
//! ```text
//! <white>Assigned random nicknames to <accent>17 players</accent>!</white>
//! <gray>Choosing random nickname for 17 players...</gray>
//! <click:run_command:'/nick random admin'><accent>/nick random admin</accent></click>
//! ```
//!
//! Tags: the colours `gray`, `white`, `red`, `green`, `aqua`, `yellow`, `gold`, Rift's own `accent`,
//! `color:#rrggbb` (or `#rrggbb`), and `click:ACTION:'VALUE'` for `run_command`, `suggest_command`,
//! `open_url` and `copy_to_clipboard`. A closing tag (`</accent>`, `</color>`, `</click>`) ends the
//! newest open tag it names; a tag that is not one of these, or has no opening, is shown as typed.
//! Text that came from a player or a file goes through [`escape`] first, so it can never be a tag.

use horizon_client_core::text::{ClickEvent, Style, Text};

pub const WHITE: u32 = 0xFFFFFF;
pub const GRAY: u32 = 0xAAAAAA;
pub const RED: u32 = 0xFF5555;
pub const GREEN: u32 = 0x55FF55;
pub const AQUA: u32 = 0x55FFFF;
pub const YELLOW: u32 = 0xFFFF55;
pub const GOLD: u32 = 0xFFAA00;
/// The colour of what a message is about: a name, a number, a command to click.
pub const ACCENT: u32 = 0xC68346;

/// How many characters of the typed command an error shows before its message.
const CONTEXT: usize = 10;

/// The words a command error opens with.
pub const ERROR_HEADER: &str = "Unknown or incomplete command. See below for error";

fn named(name: &str) -> Option<u32> {
    Some(match name {
        "white" => WHITE,
        "gray" | "grey" => GRAY,
        "red" => RED,
        "green" => GREEN,
        "aqua" => AQUA,
        "yellow" => YELLOW,
        "gold" => GOLD,
        "accent" => ACCENT,
        _ => return None,
    })
}

fn hex(text: &str) -> Option<u32> {
    let digits = text.strip_prefix('#')?;
    (digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(digits, 16).ok())
        .flatten()
}

/// Makes `text` safe to put in markup: it is shown exactly as it is, whatever it holds.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '<' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[derive(Clone)]
struct Open {
    /// What `</...>` may close it by: the tag's own name, or its kind (`color`, `click`).
    name: String,
    kind: &'static str,
    color: Option<u32>,
    click: Option<ClickEvent>,
}

struct Piece {
    text: String,
    color: u32,
    click: Option<ClickEvent>,
}

/// The tag between `<` and `>` starting at `start` (just after the `<`), and the index after its
/// `>`. A `>` inside quotes does not end it.
fn tag_at(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut quote: Option<char> = None;
    let mut i = start;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == '>' => return Some((chars[start..i].iter().collect(), i + 1)),
            // A tag never holds a `<`: that one starts another.
            None if c == '<' => return None,
            None => {}
        }
        i += 1;
    }
    None
}

/// `a:b:'c:d'` as `["a", "b", "c:d"]`.
fn split_args(tag: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut quote: Option<char> = None;
    for c in tag.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => parts.last_mut().expect("one part").push(c),
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == ':' => parts.push(String::new()),
            None => parts.last_mut().expect("one part").push(c),
        }
    }
    parts
}

/// What an opening tag does, when it is one.
fn opening(tag: &str) -> Option<Open> {
    let parts = split_args(tag);
    let name = parts[0].to_ascii_lowercase();
    if let Some(color) = named(&name).or_else(|| hex(&name)) {
        return (parts.len() == 1).then_some(Open {
            name,
            kind: "color",
            color: Some(color),
            click: None,
        });
    }
    match (name.as_str(), parts.as_slice()) {
        ("color" | "colour" | "c", [_, value]) => {
            let value = value.to_ascii_lowercase();
            let color = named(&value).or_else(|| hex(&value))?;
            Some(Open {
                name,
                kind: "color",
                color: Some(color),
                click: None,
            })
        }
        ("click", [_, action, value]) => {
            let value = value.clone();
            let click = match action.to_ascii_lowercase().as_str() {
                "run_command" => ClickEvent::RunCommand(value),
                "suggest_command" => ClickEvent::SuggestCommand(value),
                "open_url" => ClickEvent::OpenUrl(value),
                "copy_to_clipboard" => ClickEvent::CopyToClipboard(value),
                _ => return None,
            };
            Some(Open {
                name: "click".into(),
                kind: "click",
                color: None,
                click: Some(click),
            })
        }
        _ => None,
    }
}

fn parse(markup: &str, base: u32) -> Vec<Piece> {
    let chars: Vec<char> = markup.chars().collect();
    let mut pieces: Vec<Piece> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut text = String::new();
    let style = |stack: &[Open]| -> (u32, Option<ClickEvent>) {
        let color = stack.iter().rev().find_map(|o| o.color).unwrap_or(base);
        let click = stack.iter().rev().find_map(|o| o.click.clone());
        (color, click)
    };
    let flush = |text: &mut String, stack: &[Open], pieces: &mut Vec<Piece>| {
        if !text.is_empty() {
            let (color, click) = style(stack);
            pieces.push(Piece {
                text: std::mem::take(text),
                color,
                click,
            });
        }
    };
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' if matches!(chars.get(i + 1), Some('<' | '\\')) => {
                text.push(chars[i + 1]);
                i += 2;
            }
            '<' => {
                let handled = tag_at(&chars, i + 1).and_then(|(tag, next)| {
                    if let Some(closing) = tag.strip_prefix('/') {
                        let closing = closing.to_ascii_lowercase();
                        let at = stack
                            .iter()
                            .rposition(|o| o.name == closing || o.kind == closing)?;
                        flush(&mut text, &stack, &mut pieces);
                        stack.remove(at);
                    } else {
                        let open = opening(&tag)?;
                        flush(&mut text, &stack, &mut pieces);
                        stack.push(open);
                    }
                    Some(next)
                });
                match handled {
                    Some(next) => i = next,
                    None => {
                        text.push('<');
                        i += 1;
                    }
                }
            }
            c => {
                text.push(c);
                i += 1;
            }
        }
    }
    flush(&mut text, &stack, &mut pieces);
    pieces
}

/// The text of `markup`, white where it names no colour.
pub fn mini(markup: &str) -> Text {
    mini_in(markup, WHITE)
}

/// The text of `markup`, in `base` where it names no colour.
pub fn mini_in(markup: &str, base: u32) -> Text {
    let mut out = Text::empty();
    for piece in parse(markup, base) {
        let hover = match &piece.click {
            Some(ClickEvent::RunCommand(command)) => Some(format!("Click to run {command}")),
            Some(ClickEvent::SuggestCommand(command)) => Some(format!("Click to type {command}")),
            _ => None,
        };
        out.push(Text::styled(
            piece.text,
            Style {
                color: Some(piece.color),
                click: piece.click,
                hover: hover.map(|h| Box::new(Text::colored(h, GRAY))),
                ..Style::default()
            },
        ));
    }
    out
}

/// What `markup` says with no styling at all, for a log line.
pub fn plain(markup: &str) -> String {
    mini(markup).plain()
}

/// The two lines a command that cannot be run answers with: the header, and the typed command up
/// to the word that is wrong (its last 10 characters, after `...` when it was longer), then
/// `<--[` the reason `]`. `words` is how many words of `line` the context covers: the whole
/// command, or up to the argument at fault. The message is markup, red unless it says otherwise.
pub fn error(line: &str, words: usize, message: &str) -> [Text; 2] {
    let typed: Vec<&str> = line
        .trim()
        .trim_start_matches('/')
        .split_whitespace()
        .take(words)
        .collect();
    let joined = typed.join(" ");
    let chars: Vec<char> = joined.chars().collect();
    let context = if chars.len() > CONTEXT {
        let tail: String = chars[chars.len() - CONTEXT..].iter().collect();
        format!("...{}", tail.trim_start())
    } else {
        joined
    };
    let mut second = Text::colored(context, GRAY);
    second.push(Text::colored("<--[", RED));
    second.push(mini_in(message, RED));
    second.push(Text::colored("]", RED));
    [Text::colored(ERROR_HEADER, RED), second]
}

/// `1 player`, `17 players`: `count` and `noun` with an `s` unless it is one.
pub fn count(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colours(text: &Text) -> Vec<(String, Option<u32>)> {
        text.spans
            .iter()
            .map(|s| (s.text.clone(), s.style.color))
            .collect()
    }

    #[test]
    fn a_message_is_white_with_what_it_is_about_in_the_accent() {
        let text = mini("<white>Assigned random nicknames to <accent>17 players</accent>!</white>");
        assert_eq!(text.plain(), "Assigned random nicknames to 17 players!");
        assert_eq!(
            colours(&text),
            [
                ("Assigned random nicknames to ".into(), Some(WHITE)),
                ("17 players".into(), Some(ACCENT)),
                ("!".into(), Some(WHITE)),
            ]
        );
        assert_eq!(ACCENT, 0xC68346);
    }

    #[test]
    fn a_gray_message_and_text_with_no_tag_take_their_base() {
        assert_eq!(
            colours(&mini(
                "<gray>Choosing random nickname for 17 players...</gray>"
            )),
            [(
                "Choosing random nickname for 17 players...".into(),
                Some(GRAY)
            )]
        );
        assert_eq!(colours(&mini("plain")), [("plain".into(), Some(WHITE))]);
        assert_eq!(
            colours(&mini_in("plain", RED)),
            [("plain".into(), Some(RED))]
        );
    }

    #[test]
    fn a_colour_can_be_a_hex_a_name_or_the_color_tag() {
        for tag in [
            "<color:#c68346>x</color>",
            "<#c68346>x</#c68346>",
            "<c:#C68346>x</c>",
        ] {
            assert_eq!(colours(&mini(tag)), [("x".into(), Some(ACCENT))], "{tag}");
        }
        assert_eq!(
            colours(&mini("<color:red>x</color>")),
            [("x".into(), Some(RED))]
        );
    }

    #[test]
    fn a_click_carries_its_command_and_a_hover_that_says_what_it_does() {
        let text = mini(
            "<red>use <click:run_command:'/nick random admin'><accent>/nick random admin</accent></click> now</red>",
        );
        assert_eq!(text.plain(), "use /nick random admin now");
        let link = &text.spans[1];
        assert_eq!(link.text, "/nick random admin");
        assert_eq!(link.style.color, Some(ACCENT));
        assert_eq!(
            link.style.click,
            Some(ClickEvent::RunCommand("/nick random admin".into()))
        );
        assert_eq!(
            link.style.hover.as_ref().map(|h| h.plain()),
            Some("Click to run /nick random admin".into())
        );
        // Around it, neither click nor hover.
        assert_eq!(text.spans[0].style.click, None);
        assert_eq!(text.spans[2].style.click, None);
        assert_eq!(text.spans[2].style.color, Some(RED));
        // And in the JSON the client gets.
        assert_eq!(
            text.component()["extra"][1]["click_event"],
            horizon_client_core::text::Text::styled(
                "",
                Style {
                    click: Some(ClickEvent::RunCommand("/nick random admin".into())),
                    ..Style::default()
                }
            )
            .component()["click_event"]
        );
    }

    #[test]
    fn the_other_click_actions_are_read() {
        for (tag, want) in [
            (
                "<click:suggest_command:'/tell Bob '>x</click>",
                ClickEvent::SuggestCommand("/tell Bob ".into()),
            ),
            (
                "<click:open_url:'https://example.org/a>b'>x</click>",
                ClickEvent::OpenUrl("https://example.org/a>b".into()),
            ),
            (
                "<click:copy_to_clipboard:'abc'>x</click>",
                ClickEvent::CopyToClipboard("abc".into()),
            ),
        ] {
            assert_eq!(mini(tag).spans[0].style.click, Some(want), "{tag}");
        }
    }

    #[test]
    fn a_tag_that_is_not_one_is_shown_as_typed() {
        for text in [
            "<--[x]",
            "a < b",
            "<nick>",
            "<click:fly:'x'>y</click>",
            "</gray>",
            "<gray",
            "5 <3 you",
        ] {
            assert_eq!(plain(text), text, "{text}");
        }
        // An unclosed tag runs to the end.
        assert_eq!(
            colours(&mini("<accent>open")),
            [("open".into(), Some(ACCENT))]
        );
    }

    #[test]
    fn escaped_text_is_never_a_tag() {
        let typed = r"@a<red>\<gray>";
        let text = mini(&format!("<white>{}</white>", escape(typed)));
        assert_eq!(text.plain(), typed);
        assert_eq!(text.spans.len(), 1);
        assert_eq!(text.spans[0].style.color, Some(WHITE));
    }

    #[test]
    fn closing_tags_end_the_newest_tag_they_name() {
        let text = mini("<gray>a<accent>b<red>c</red>d</accent>e</gray>f");
        assert_eq!(
            colours(&text),
            [
                ("a".into(), Some(GRAY)),
                ("b".into(), Some(ACCENT)),
                ("c".into(), Some(RED)),
                ("d".into(), Some(ACCENT)),
                ("e".into(), Some(GRAY)),
                ("f".into(), Some(WHITE)),
            ]
        );
    }

    #[test]
    fn a_command_error_is_the_header_then_the_typed_tail_and_the_reason() {
        let [header, context] = error(
            "/nick random @a",
            3,
            "No player was found: operators are skipped, use <accent><click:run_command:'/nick random admin'>/nick random admin</click></accent> to target them",
        );
        assert_eq!(
            header.plain(),
            "Unknown or incomplete command. See below for error"
        );
        assert_eq!(header.spans[0].style.color, Some(RED));
        assert_eq!(
            context.plain(),
            "...random @a<--[No player was found: operators are skipped, use /nick random admin to target them]"
        );
        assert_eq!(context.spans[0].style.color, Some(GRAY));
        assert_eq!(context.spans[0].text, "...random @a");
        assert_eq!(context.spans[1].text, "<--[");
        assert_eq!(context.spans[1].style.color, Some(RED));
        let link = context
            .spans
            .iter()
            .find(|s| s.text == "/nick random admin")
            .unwrap();
        assert_eq!(link.style.color, Some(ACCENT));
        assert_eq!(
            link.style.click,
            Some(ClickEvent::RunCommand("/nick random admin".into()))
        );
        assert_eq!(context.spans.last().unwrap().text, "]");
        assert_eq!(context.spans.last().unwrap().style.color, Some(RED));
    }

    #[test]
    fn the_context_is_the_last_ten_characters_up_to_the_word_at_fault() {
        let line = "sculkstorm give Ja";
        assert_eq!(
            error(line, 3, "Player was not found")[1].plain(),
            "...rm give Ja<--[Player was not found]"
        );
        // Up to the second word only.
        assert_eq!(
            error("nick Zed Captain", 2, "x")[1].plain(),
            "nick Zed<--[x]",
            "ten characters or fewer is shown whole"
        );
        // Spaces are folded and the slash is not shown.
        assert_eq!(
            error("  /nick   off  ", 5, "x")[1].plain(),
            "nick off<--[x]"
        );
        assert_eq!(error("", 1, "x")[1].plain(), "<--[x]");
    }

    #[test]
    fn a_count_is_singular_for_one() {
        assert_eq!(count(1, "player"), "1 player");
        assert_eq!(count(0, "player"), "0 players");
        assert_eq!(count(17, "name"), "17 names");
    }
}
