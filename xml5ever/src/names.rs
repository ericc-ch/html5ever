//! The XML character productions that the parser and embedders share.
//!
//! A `Name` is the grammar for element and attribute names, entity names, and
//! processing-instruction targets. Every consumer used to carry its own copy
//! of the range table, which is how one copy drifts from another
//! (<https://www.w3.org/TR/xml/#NT-Name>,
//! <https://www.w3.org/TR/xml/#NT-NameStartChar>,
//! <https://www.w3.org/TR/xml/#NT-NameChar>).

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

    #[test]
    fn name_start_and_name_char_ranges() {
        assert!(is_name_start_char('a'));
        assert!(is_name_start_char(':'));
        assert!(is_name_start_char('_'));
        assert!(is_name_start_char('\u{C0}'));
        assert!(is_name_start_char('\u{10000}'));
        assert!(!is_name_start_char('1'));
        assert!(!is_name_start_char('-'));
        assert!(!is_name_start_char('.'));
        assert!(!is_name_start_char('<'));

        assert!(is_name_char('1'));
        assert!(is_name_char('-'));
        assert!(is_name_char('.'));
        assert!(is_name_char('a'));
        assert!(is_name_char('\u{B7}'));
        assert!(!is_name_char('<'));
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
    fn char_and_space_exclude_what_xml_excludes() {
        assert!(is_char('\t'));
        assert!(is_char('\u{10FFFF}'));
        assert!(!is_char('\0'));
        assert!(!is_char('\u{FFFE}'));

        assert!(is_space(' '));
        assert!(is_space('\r'));
        assert!(!is_space('\u{C}'));
    }
}
