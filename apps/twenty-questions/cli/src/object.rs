//! The secret object, and when a line says its name. Small models are
//! unreliable judges of their own secret, so the game does not leave this
//! to the host: a guess that says the object's name wins whichever tool
//! the host calls, and a line of the host's that says it before the game
//! is won is not sent.

/// The name `object` goes by: lowercased, without a leading *a*, *an* or
/// *the*. `a teapot` is `teapot`, `the Statue of Liberty` is `statue of
/// liberty`.
pub fn name(object: &str) -> String {
    let lowered = object.trim().to_lowercase();
    ["a ", "an ", "the "]
        .iter()
        .find_map(|article| lowered.strip_prefix(article))
        .unwrap_or(&lowered)
        .trim()
        .to_string()
}

/// Whether `text` says the name of `object`: the text, lowercased, holds
/// the [`name`] as words of their own, or in the plural, with an *s* or
/// *es*. `Is it a teapot?` and `Teapots?` say `a teapot`; `Is it a
/// category?` does not say `a cat`.
pub fn says(text: &str, object: &str) -> bool {
    let name = name(object);
    if name.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    let apart = |c: Option<char>| !c.is_some_and(char::is_alphanumeric);
    text.match_indices(&name).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = &text[at + name.len()..];
        apart(before)
            && ["", "s", "es"].iter().any(|plural| {
                after
                    .strip_prefix(plural)
                    .is_some_and(|rest| apart(rest.chars().next()))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_object_goes_by_its_name_without_a_leading_article() {
        assert_eq!(name("a teapot"), "teapot");
        assert_eq!(name("An Egg"), "egg");
        assert_eq!(name("the Statue of Liberty"), "statue of liberty");
        assert_eq!(name("durian"), "durian");
        assert_eq!(name(" a banjo "), "banjo");
        // An article that is the whole name stays.
        assert_eq!(name("The"), "the");
    }

    #[test]
    fn a_line_says_the_name_as_words_of_its_own_whatever_the_case() {
        assert!(says("Is it a teapot?", "a teapot"));
        assert!(says("TEAPOT!", "a teapot"));
        assert!(says("teapot", "a teapot"));
        assert!(says("Are we talking about teapots?", "a teapot"));
        assert!(says("Is it one of the sandwiches?", "a sandwich"));
        assert!(says(
            "Is it the Statue of Liberty?",
            "the Statue of Liberty"
        ));
        assert!(!says("Is it a category of things?", "a cat"));
        assert!(!says("Can it communicate?", "a cat"));
        assert!(!says("Is it a teapotty kind of thing?", "a teapot"));
        assert!(!says("Is it found in a kitchen?", "a teapot"));
        assert!(!says("Is it a couch?", "a sofa"));
        assert!(!says("Anything at all.", "the"));
        assert!(!says("Anything at all.", "a "));
        assert!(!says("Anything at all.", "  "));
    }
}
