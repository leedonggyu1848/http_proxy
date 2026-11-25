//! HTTP content codings in application order and bounded streaming decoding.

use std::io::{self, Read};

const MAX_LAYERS: usize = 4;

/// How to handle an empty element in a comma-separated field value.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum EmptyTokens {
    /// Keep the empty name so decoder validation rejects it.
    Keep,
    /// Ignore empty names when reading a list.
    Skip,
}

/// Yields trimmed non-identity names in header order, including unknown codings.
pub fn tokens<'a>(
    values: impl IntoIterator<Item = &'a str>,
    empty: EmptyTokens,
) -> impl Iterator<Item = &'a str> {
    values
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(move |name| empty == EmptyTokens::Keep || !name.is_empty())
        .filter(|name| !name.eq_ignore_ascii_case("identity"))
}

#[derive(Clone, Copy, Debug)]
enum Codec {
    Gzip,
    Deflate,
    Brotli,
    Zstd,
}

const CODINGS: &[(&str, Codec)] = &[
    ("gzip", Codec::Gzip),
    ("x-gzip", Codec::Gzip),
    ("deflate", Codec::Deflate),
    ("br", Codec::Brotli),
    ("zstd", Codec::Zstd),
];

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Too many Content-Encoding layers")]
    TooManyLayers,
    #[error("Unsupported Content-Encoding: {0:?}")]
    Unsupported(String),
}

/// A validated stack in application order. The empty stack passes input through.
#[derive(Clone, Debug, Default)]
pub struct Encodings(Vec<(&'static str, Codec)>);

impl Encodings {
    pub fn parse<'a>(
        values: impl IntoIterator<Item = &'a str>,
        empty: EmptyTokens,
    ) -> Result<Self, ParseError> {
        let names: Vec<_> = tokens(values, empty).take(MAX_LAYERS + 1).collect();
        if names.len() > MAX_LAYERS {
            return Err(ParseError::TooManyLayers);
        }
        names
            .into_iter()
            .map(|name| {
                CODINGS
                    .iter()
                    .copied()
                    .find(|(known, _)| known.eq_ignore_ascii_case(name))
                    .ok_or_else(|| ParseError::Unsupported(name.to_ascii_lowercase()))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Includes the spelling of recognized aliases, normalized to lowercase.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.0.iter().map(|(name, _)| *name)
    }

    /// Removes the outer encoding first. Each layer can supply at most
    /// `layer_limit` bytes to the next reader, including the final caller.
    pub fn reader<'a>(
        &self,
        input: impl Read + 'a,
        layer_limit: u64,
        brotli_buffer: usize,
    ) -> io::Result<Box<dyn Read + 'a>> {
        let mut reader: Box<dyn Read> = Box::new(input);
        for (_, codec) in self.0.iter().rev() {
            reader = match codec {
                Codec::Gzip => {
                    Box::new(flate2::read::MultiGzDecoder::new(reader).take(layer_limit))
                }
                Codec::Deflate => {
                    Box::new(flate2::read::ZlibDecoder::new(reader).take(layer_limit))
                }
                Codec::Brotli => Box::new(
                    brotli_decompressor::Decompressor::new(reader, brotli_buffer).take(layer_limit),
                ),
                Codec::Zstd => {
                    let mut decoder = zstd::stream::read::Decoder::new(reader)?;
                    // HTTP zstd permits at most an 8 MiB window.
                    decoder.window_log_max(23)?;
                    Box::new(decoder.take(layer_limit))
                }
            };
        }
        Ok(reader)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_headers_and_lists_in_application_order() -> Result<(), ParseError> {
        for headers in [
            &[" GZIP ", "identity, x-Gzip, Br"][..],
            &["GZIP, identity, x-gzip, BR"],
        ] {
            let layers = Encodings::parse(headers.iter().copied(), EmptyTokens::Keep)?;
            assert_eq!(layers.names().collect::<Vec<_>>(), ["gzip", "x-gzip", "br"]);
        }
        assert!(Encodings::parse(["identity", " "], EmptyTokens::Skip)?.is_empty());
        assert!(Encodings::parse(["gzip,br,zstd,deflate"], EmptyTokens::Keep).is_ok());
        assert!(matches!(
            Encodings::parse(["gzip,br,zstd,deflate,br"], EmptyTokens::Skip),
            Err(ParseError::TooManyLayers)
        ));
        Ok(())
    }

    #[test]
    fn keeps_unknown_tokens_for_raw_bodies_and_validates_decoding() -> Result<(), ParseError> {
        let headers = ["identity, CuStOm, "];
        assert_eq!(
            tokens(headers, EmptyTokens::Skip).collect::<Vec<_>>(),
            ["CuStOm"]
        );
        assert!(matches!(
            Encodings::parse(headers, EmptyTokens::Skip),
            Err(ParseError::Unsupported(name)) if name == "custom"
        ));
        assert!(matches!(
            Encodings::parse(["gzip, "], EmptyTokens::Keep),
            Err(ParseError::Unsupported(name)) if name.is_empty()
        ));
        assert_eq!(
            Encodings::parse(["gzip, "], EmptyTokens::Skip)?
                .names()
                .next(),
            Some("gzip")
        );
        Ok(())
    }
}
