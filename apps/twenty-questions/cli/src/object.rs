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

/// Whether `text` holds a word of the name of `object`, one of three
/// letters or more, as words of its own, singular or plural. A guess the
/// host judges to name the object must: small models call a category,
/// *Is it furniture?*, the object itself.
pub fn shares_a_word(text: &str, object: &str) -> bool {
    name(object)
        .split_whitespace()
        .filter(|word| word.len() >= 3)
        .any(|word| holds(text, word))
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

/// `object` as one says it after *I'm thinking of*: with its own article,
/// lowercased, or else, named bare as a configured object may be, with *a*
/// or *an*, unless it is a proper name of several capitalized words. `owl`
/// is `an owl`, `An egg` is `an egg` and `Big Ben` is `Big Ben`. A small
/// model judges a question about its secret better about *a raven* than
/// *the raven*, which it reads as Poe's poem. The host picks its object
/// already said this way, as in *durian*, which this cannot tell.
pub fn spoken(object: &str) -> String {
    let object = object.trim();
    if object.is_empty() {
        return String::new();
    }
    let article = object
        .split_once(' ')
        .map(|(article, rest)| (article.to_lowercase(), rest));
    match article {
        Some((article, rest)) if ["a", "an", "the"].contains(&article.as_str()) => {
            format!("{article} {rest}")
        }
        Some(_) if proper(object) => object.to_string(),
        _ if object.starts_with(|c: char| "aeiou".contains(c.to_ascii_lowercase())) => {
            format!("an {object}")
        }
        _ => format!("a {object}"),
    }
}

/// `question` put about `object`, as it is [`spoken`], itself: each *it*
/// in it, a word of its own, replaced by the object, capitalized where the
/// *it* was. `Is it alive?` about `a unicycle` is `Is a unicycle alive?`,
/// and about `durian`, `Is durian alive?`. A small model judges a question
/// about its secret far better put this way.
pub fn about(question: &str, object: &str) -> String {
    let object = object.trim();
    if object.is_empty() {
        return question.to_string();
    }
    let capitalized: String = object
        .chars()
        .take(1)
        .flat_map(char::to_uppercase)
        .chain(object.chars().skip(1))
        .collect();
    let mut about = String::new();
    let mut word = String::new();
    let put = |word: &mut String, about: &mut String| {
        match word.eq_ignore_ascii_case("it") {
            true if word.starts_with('I') => about.push_str(&capitalized),
            true => about.push_str(object),
            false => about.push_str(word),
        }
        word.clear();
    };
    for c in question.chars() {
        if c.is_alphanumeric() {
            word.push(c);
        } else {
            put(&mut word, &mut about);
            about.push(c);
        }
    }
    put(&mut word, &mut about);
    about
}

/// Whether `name` is a proper name: each of its words capitalized, but for
/// small words such as *of*, as in *Statue of Liberty*.
fn proper(name: &str) -> bool {
    name.split_whitespace()
        .filter(|word| !["of", "the", "and", "de"].contains(word))
        .all(|word| word.starts_with(char::is_uppercase))
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
    fn a_question_is_put_about_the_object_in_place_of_it() {
        assert_eq!(about("Is it alive?", "a unicycle"), "Is a unicycle alive?");
        assert_eq!(
            about("Can you eat it?", "a banana"),
            "Can you eat a banana?"
        );
        assert_eq!(about("It flies?", "a penguin"), "A penguin flies?");
        assert_eq!(
            about("IT is bigger than it was? Items?", "the Eiffel Tower"),
            "The Eiffel Tower is bigger than the Eiffel Tower was? Items?"
        );
        assert_eq!(
            about("Is its handle long?", "a teapot"),
            "Is its handle long?"
        );
        assert_eq!(about("Is it?", ""), "Is it?");
        // The object is put as it is said after "I'm thinking of".
        assert_eq!(about("Is it a fruit?", "durian"), "Is durian a fruit?");
        assert_eq!(about("Is it tall?", "Big Ben"), "Is Big Ben tall?");
        assert_eq!(about("It swims?", "an octopus"), "An octopus swims?");
    }

    #[test]
    fn an_object_is_spoken_with_its_own_article_or_else_a_or_an() {
        assert_eq!(spoken("a unicorn"), "a unicorn");
        assert_eq!(spoken("An egg"), "an egg");
        assert_eq!(spoken(" the Eiffel Tower "), "the Eiffel Tower");
        // A configured object named bare takes "a" or "an", unless it is a
        // proper name: "the raven" is read as Poe's poem.
        assert_eq!(spoken("chicken"), "a chicken");
        assert_eq!(spoken("Chicken"), "a Chicken");
        assert_eq!(spoken("owl"), "an owl");
        assert_eq!(spoken("Big Ben"), "Big Ben");
        assert_eq!(spoken("Statue of Liberty"), "Statue of Liberty");
        assert_eq!(spoken(""), "");
    }

    #[test]
    fn a_line_shares_a_word_of_the_name_when_it_holds_one_of_three_letters_or_more() {
        assert!(shares_a_word(
            "Is it a cast iron pan?",
            "a cast iron skillet"
        ));
        assert!(shares_a_word("Is it a pot?", "a china tea pot"));
        assert!(shares_a_word("Is it a ravens' wing?", "raven"));
        assert!(!shares_a_word("Is it furniture?", "a loveseat"));
        assert!(!shares_a_word(
            "Is it a pair of shoes?",
            "a pair of scissors"
        ));
        assert!(!shares_a_word("Is it an ox?", "an ox cart"));
        assert!(!shares_a_word("Is it a kettle for tea?", "a teapot"));
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
