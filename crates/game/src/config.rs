//! What every game's configuration file shares: the [`Format`] its
//! `format` key says it is written in, checked as the file is read, and
//! the [`Program`] that reads it, which a refusal names.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::marker::PhantomData;

/// The newest configuration format, which every configuration file the
/// project writes is written in.
pub const CONFIG_FORMAT: u32 = 1;

/// The program that reads a configuration file, as a refusal names it.
/// A configuration that compares, clones or copies its [`Format`] needs a
/// program that does too.
pub trait Program {
    /// The program's name: `social-deduction`.
    const NAME: &'static str;
}

/// A configuration file's `format` key: the format it is written in, as
/// the program `P` reads it. A file without one is format 1. A format
/// newer than [`CONFIG_FORMAT`] is refused as it is read, so that `toml`
/// places the error at the key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Format<P>(u32, PhantomData<fn() -> P>);

impl<P> Format<P> {
    /// [`CONFIG_FORMAT`], the format the project writes.
    pub const NEWEST: Self = Format(CONFIG_FORMAT, PhantomData);
}

impl<P> Default for Format<P> {
    fn default() -> Self {
        Format(1, PhantomData)
    }
}

impl<P> fmt::Debug for Format<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Format").field(&self.0).finish()
    }
}

impl<P> Serialize for Format<P> {
    /// The format's number.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, P: Program> Deserialize<'de> for Format<P> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        match u32::deserialize(deserializer)? {
            0 => Err(D::Error::custom("configuration formats begin at 1")),
            format if format > CONFIG_FORMAT => Err(D::Error::custom(format!(
                "This file is configuration format {format}. \
                 This version of {} reads formats up to {CONFIG_FORMAT}.",
                P::NAME
            ))),
            format => Ok(Format(format, PhantomData)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A program that reads configuration files.
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Chess;

    impl Program for Chess {
        const NAME: &'static str = "chess";
    }

    /// A file holding `format` alone.
    #[derive(Debug, Deserialize, Serialize)]
    struct File {
        #[serde(default)]
        format: Format<Chess>,
    }

    /// The format `text` is read as, or the error it is refused with.
    fn read(text: &str) -> Result<Format<Chess>, String> {
        toml::from_str::<File>(text)
            .map(|file| file.format)
            .map_err(|error| error.message().to_string())
    }

    #[test]
    fn a_file_without_a_format_is_format_1_the_newest() {
        assert_eq!(read(""), Ok(Format::default()));
        assert_eq!(read("format = 1"), Ok(Format::NEWEST));
        assert_eq!(format!("{:?}", Format::<Chess>::NEWEST), "Format(1)");
    }

    #[test]
    fn a_newer_format_is_refused_naming_the_program_and_format_0_is_no_format() {
        assert_eq!(
            read("format = 2"),
            Err("This file is configuration format 2. \
                 This version of chess reads formats up to 1."
                .to_string())
        );
        assert_eq!(
            read("format = 0"),
            Err("configuration formats begin at 1".to_string())
        );
    }

    #[test]
    fn a_format_is_written_as_its_number() {
        let file = File {
            format: Format::NEWEST,
        };
        assert_eq!(toml::to_string(&file).unwrap(), "format = 1\n");
    }
}
