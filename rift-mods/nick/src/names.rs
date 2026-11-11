//! What a nickname may be.
//!
//! A nickname follows Mojang's naming conventions: ASCII letters, digits and underscores, up to 16
//! characters. Mojang's own minimum is 3; an operator may give a nickname of 1 or 2 (the config's
//! `min-length` says), which only a legacy account has. The shape also keeps a nickname safe in a
//! request path and fit to be a profile name over a head (`Host::set_name_tag`).

/// The words `/nick` takes as subcommands, so none of them can be a nickname: `/nick off` turns a
/// nickname off, it does not name anyone "off".
pub const RESERVED: [&str; 6] = ["off", "random", "info", "pool", "list", "reload"];

/// Why a name is not a nickname.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooShort { min: usize },
    TooLong { max: usize },
    BadCharacter(char),
    Reserved,
}

impl NameError {
    /// What an operator is told about `name`.
    pub fn says(&self, name: &str) -> String {
        match self {
            Self::Empty => "A nickname cannot be empty.".to_owned(),
            Self::TooShort { min } => {
                format!("{name} is too short for a nickname: at least {min} characters.")
            }
            Self::TooLong { max } => {
                format!("{name} is too long for a nickname: at most {max} characters.")
            }
            Self::BadCharacter(c) => format!(
                "{name} cannot be a nickname: {c:?} is not allowed, only letters, digits and underscores are."
            ),
            Self::Reserved => format!("{name} is a /nick word, so it cannot be a nickname."),
        }
    }
}

/// Whether `name` has the shape of an account's name: 1 to 16 ASCII letters, digits or
/// underscores.
pub fn is_account_name(name: &str) -> bool {
    (1..=16).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Whether `name` may be a nickname of `min` to `max` characters.
pub fn check(name: &str, min: usize, max: usize) -> Result<(), NameError> {
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    // The characters first: past them every character is one byte, and `len` is the length.
    if let Some(c) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '_'))
    {
        return Err(NameError::BadCharacter(c));
    }
    if name.len() < min {
        return Err(NameError::TooShort { min });
    }
    if name.len() > max {
        return Err(NameError::TooLong { max });
    }
    if RESERVED.iter().any(|w| w.eq_ignore_ascii_case(name)) {
        return Err(NameError::Reserved);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_follow_the_conventions_pass_with_one_or_two_characters_allowed() {
        // The last has exactly 16.
        for name in [
            "Captain",
            "a_b_c",
            "X",
            "ab",
            "Steve_123",
            "sixteen_chars_ok",
        ] {
            assert_eq!(check(name, 1, 16), Ok(()), "{name}");
        }
    }

    #[test]
    fn names_that_do_not_follow_them_are_refused_and_say_why() {
        assert_eq!(check("", 1, 16), Err(NameError::Empty));
        assert_eq!(
            check("seventeen_chars_x", 1, 16),
            Err(NameError::TooLong { max: 16 })
        );
        assert_eq!(check("ab", 3, 16), Err(NameError::TooShort { min: 3 }));
        assert_eq!(check("Cap tain", 1, 16), Err(NameError::BadCharacter(' ')));
        assert_eq!(
            check("Capt\u{e9}n", 1, 16),
            Err(NameError::BadCharacter('\u{e9}'))
        );
        assert_eq!(check("§cRed", 1, 16), Err(NameError::BadCharacter('§')));
        assert_eq!(check("a/b", 1, 16), Err(NameError::BadCharacter('/')));
        assert_eq!(check("Cap-tain", 1, 16), Err(NameError::BadCharacter('-')));
    }

    #[test]
    fn the_words_of_the_command_are_not_nicknames_in_any_case() {
        for word in ["off", "OFF", "Random", "info", "POOL", "list", "Reload"] {
            assert_eq!(check(word, 1, 16), Err(NameError::Reserved), "{word}");
        }
        // Part of a word is fine.
        assert_eq!(check("offline", 1, 16), Ok(()));
        assert_eq!(check("admin", 1, 16), Ok(()));
    }

    #[test]
    fn what_an_operator_is_told_names_the_nickname_and_the_rule() {
        assert_eq!(
            NameError::TooLong { max: 16 }.says("seventeen_chars_x"),
            "seventeen_chars_x is too long for a nickname: at most 16 characters."
        );
        assert!(NameError::BadCharacter(' ').says("a b").contains("' '"));
        assert_eq!(
            NameError::Reserved.says("off"),
            "off is a /nick word, so it cannot be a nickname."
        );
    }

    #[test]
    fn an_account_name_has_a_minecraft_names_shape() {
        assert!(is_account_name("Notch") && is_account_name("a") && is_account_name("_x_"));
        assert!(!is_account_name("") && !is_account_name("has space"));
        assert!(!is_account_name("seventeen_chars_x") && !is_account_name("a/b"));
    }
}
