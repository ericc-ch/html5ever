//! The XML character productions that the parser and embedders share.
//!
//! A `Name` is the grammar for element and attribute names, entity names, and
//! processing-instruction targets. `Char` is the code points a document may
//! contain at all, and `S` is XML whitespace, which is not ASCII whitespace.
//! Every consumer used to carry its own copy of the range tables, which is how
//! one copy drifts from another
//! (<https://www.w3.org/TR/xml/#NT-Name>,
//! <https://www.w3.org/TR/xml/#NT-NameStartChar>,
//! <https://www.w3.org/TR/xml/#NT-NameChar>,
//! <https://www.w3.org/TR/xml/#NT-Char>,
//! <https://www.w3.org/TR/xml/#NT-S>).

/// Whether `character` may start an XML `Name`
/// (<https://www.w3.org/TR/xml/#NT-NameStartChar>).
pub fn is_name_start_char(character: char) -> bool {
    matches!(character, ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}')
        || ('\u{10000}'..='\u{EFFFF}').contains(&character)
}

/// Whether `character` may continue an XML `Name`
/// (<https://www.w3.org/TR/xml/#NT-NameChar>).
pub fn is_name_char(character: char) -> bool {
    is_name_start_char(character)
        || matches!(character, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{0300}'..='\u{036F}' | '\u{203F}'..='\u{2040}')
}

/// Whether `name` matches the XML `Name` production
/// (<https://www.w3.org/TR/xml/#NT-Name>).
pub fn is_name(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if is_name_start_char(first) => characters.all(is_name_char),
        _ => false,
    }
}

/// Whether `character` is an XML `Char`: the code points a document may
/// contain at all (<https://www.w3.org/TR/xml/#NT-Char>).
pub fn is_char(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}')
        || ('\u{10000}'..='\u{10FFFF}').contains(&character)
}

/// Whether `character` is XML whitespace (`S`), which excludes form feed
/// unlike ASCII whitespace (<https://www.w3.org/TR/xml/#NT-S>).
pub fn is_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\r' | '\n')
}

#[cfg(test)]
mod tests {
    use super::{is_char, is_name, is_name_char, is_name_start_char, is_space};

    /// Every `NameStartChar` range, ends included
    /// (<https://www.w3.org/TR/xml/#NT-NameStartChar>).
    const NAME_START_RANGES: &[(char, char)] = &[
        (':', ':'),
        ('A', 'Z'),
        ('_', '_'),
        ('a', 'z'),
        ('\u{C0}', '\u{D6}'),
        ('\u{D8}', '\u{F6}'),
        ('\u{F8}', '\u{2FF}'),
        ('\u{370}', '\u{37D}'),
        ('\u{37F}', '\u{1FFF}'),
        ('\u{200C}', '\u{200D}'),
        ('\u{2070}', '\u{218F}'),
        ('\u{2C00}', '\u{2FEF}'),
        ('\u{3001}', '\u{D7FF}'),
        ('\u{F900}', '\u{FDCF}'),
        ('\u{FDF0}', '\u{FFFD}'),
        ('\u{10000}', '\u{EFFFF}'),
    ];

    fn neighbours(range: (char, char)) -> impl Iterator<Item = char> {
        let (first, last) = range;
        [first as u32 - 1, last as u32 + 1]
            .into_iter()
            .filter_map(char::from_u32)
    }

    #[test]
    fn every_name_start_range_accepts_only_its_ends_and_between() {
        for range in NAME_START_RANGES {
            let (first, last) = *range;
            assert!(is_name_start_char(first), "{first:?}");
            assert!(is_name_start_char(last), "{last:?}");
            assert!(is_name_char(first) && is_name_char(last));
            for outside in neighbours(*range) {
                assert!(!is_name_start_char(outside), "{outside:?}");
            }
        }
    }

    #[test]
    fn name_char_adds_the_four_infixes() {
        for (first, last) in [
            ('-', '-'),
            ('.', '.'),
            ('0', '9'),
            ('\u{B7}', '\u{B7}'),
            ('\u{300}', '\u{36F}'),
            ('\u{203F}', '\u{2040}'),
        ] {
            assert!(is_name_char(first), "{first:?}");
            assert!(is_name_char(last), "{last:?}");
        }
        for character in ['<', '>', '"', '/', '?', '\u{D7}', '\u{37E}', '\u{200E}'] {
            assert!(!is_name_char(character), "{character:?}");
        }
    }

    #[test]
    fn names_need_a_start_char_and_only_name_chars() {
        assert!(is_name("xml-stylesheet"));
        assert!(is_name("a.b-c:1"));
        assert!(is_name("_"));
        assert!(!is_name(""));
        assert!(!is_name("1a"));
        assert!(!is_name("a b"));
        assert!(!is_name("a<"));
    }

    #[test]
    fn char_accepts_exactly_the_document_code_points() {
        for character in [
            '\t',
            '\n',
            '\r',
            '\u{20}',
            '\u{7F}',
            '\u{D7FF}',
            '\u{E000}',
            '\u{FFFD}',
            '\u{10FFFF}',
        ] {
            assert!(is_char(character), "{character:?}");
        }
        // A surrogate cannot be a `char`, so `is_char` never sees one.
        for character in ['\0', '\u{1F}', '\u{FFFE}'] {
            assert!(!is_char(character), "{character:?}");
        }
    }

    #[test]
    fn space_is_xml_whitespace_not_ascii_whitespace() {
        for character in [' ', '\t', '\r', '\n'] {
            assert!(is_space(character), "{character:?}");
        }
        for character in ['\u{B}', '\u{C}', '\u{85}'] {
            assert!(!is_space(character), "{character:?}");
        }
    }
}
