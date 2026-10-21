//! The secret object, and when a line says its name. Small models are
//! unreliable judges of their own secret, so the game does not leave this
//! to the host: a guess that [`says`] the object's name wins whichever
//! tool the host calls, and a line of the host's that [gives it
//! away](gives_away) before the game is won is not sent.

/// Words that measure an object out rather than name it, as in *a pair of
/// scissors*.
const MEASURES: [&str; 12] = [
    "pair", "set", "piece", "bunch", "cup", "glass", "slice", "bowl", "box", "bottle", "bag",
    "loaf",
];

/// The name `object` goes by: lowercased, without a leading *a*, *an* or
/// *the*, or a measure such as *pair of*. `a teapot` is `teapot`, `a pair
/// of scissors` is `scissors`, `the Statue of Liberty` is `statue of
/// liberty`.
pub fn name(object: &str) -> String {
    let lowered = object.trim().to_lowercase();
    let mut name = ["a ", "an ", "the "]
        .iter()
        .find_map(|article| lowered.strip_prefix(article))
        .unwrap_or(&lowered)
        .trim();
    if let Some((measure, rest)) = name.split_once(" of ")
        && MEASURES.contains(&measure)
    {
        name = rest.trim();
    }
    name.to_string()
}

/// Whether `text` says the name of `object`: the text, lowercased, holds
/// the [`name`] as words of their own, in the singular or the plural. `Is
/// it a teapot?` and `Teapots?` say `a teapot`, and `Scissors?` says `a
/// pair of scissors`; `Is it a category?` does not say `a cat`.
pub fn says(text: &str, object: &str) -> bool {
    holds(text, &name(object))
}

/// Whether `text` gives `object` away, when it answers `asked`: it says
/// the object's name, or the name's last word, its head noun, which
/// `asked` did not say already. `It's a kind of tower.` gives *the Eiffel
/// Tower* away; `Yes, it is a tower.` does not, in answer to *Is it a
/// tower?*.
pub fn gives_away(text: &str, object: &str, asked: &str) -> bool {
    let name = name(object);
    let head = name.rsplit(' ').next().unwrap_or_default();
    holds(text, &name) || holds(text, head) && !holds(asked, head)
}

/// Whether `text`, lowercased, holds `words`, also lowercase, as words of
/// their own, in the singular or the plural: with an *s* or *es* after
/// them, or without the one they end in.
fn holds(text: &str, words: &str) -> bool {
    if words.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    let apart = |c: Option<char>| !c.is_some_and(char::is_alphanumeric);
    let singular = words.strip_suffix('s').filter(|stem| !stem.is_empty());
    std::iter::once(words).chain(singular).any(|words| {
        text.match_indices(words).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = &text[at + words.len()..];
            apart(before)
                && ["", "s", "es"].iter().any(|plural| {
                    after
                        .strip_prefix(plural)
                        .is_some_and(|rest| apart(rest.chars().next()))
                })
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
        assert!(says("Scissors?", "a pair of scissors"));
        assert!(says("Is it a scissor?", "a pair of scissors"));
        assert!(!says("Is it a pair?", "a pair of scissors"));
    }

    #[test]
    fn a_measure_is_no_part_of_the_name() {
        assert_eq!(name("a pair of scissors"), "scissors");
        assert_eq!(name("a cup of coffee"), "coffee");
        assert_eq!(name("the Statue of Liberty"), "statue of liberty");
    }

    #[test]
    fn a_line_gives_the_object_away_by_its_name_or_its_head_noun_unless_asked_about_it() {
        let scissors = "a pair of scissors";
        assert!(gives_away(
            "Scissors are typically made of metal.",
            scissors,
            ""
        ));
        assert!(gives_away("It's a kind of tower.", "the Eiffel Tower", ""));
        assert!(!gives_away(
            "Yes, it is a tower.",
            "the Eiffel Tower",
            "Is it a tower?"
        ));
        assert!(gives_away(
            "Yes, it is the Eiffel Tower.",
            "the Eiffel Tower",
            "Is it a tower?"
        ));
        assert!(!gives_away(
            "No, it is not alive.",
            scissors,
            "Is it alive?"
        ));
    }
}
