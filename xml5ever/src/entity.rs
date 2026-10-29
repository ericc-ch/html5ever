//! Internal general entity expansion for the XML frontend.
//!
//! xml5ever tokenizes but never defines entities: without this step every
//! `&name;` stays literal text. Replacement text that contains markup is
//! spliced into the source so the XML parser builds the corresponding nodes
//! (<https://www.w3.org/TR/xml/#intern-replacement>).
//!
//! Includes the internal general entities declared in the document instance.
//! External general entities are never included; a reference to one reports an
//! error. Parameter entities are not processed, so their references stay
//! literal.
//!
//! Billion-laughs protection: entity expansion is bounded three ways.
//! Declaration count caps HashMap memory; depth caps the call stack
//! (`expand_replacement` recurses per nesting level); the byte budget caps
//! total output (a doubling chain otherwise turns kilobytes into gigabytes).
//! Exhaustion of any of the three surfaces a parse error and leaves the
//! rest literal so the document fails closed into a `parsererror` instead
//! of a silently wrong tree.
//!
//! Only the first error is recorded: one is enough to fail the document,
//! and a body of a million undefined references must not allocate a
//! million messages.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::names::is_name;

const MAX_ENTITY_DECLS: usize = 10_000;
const MAX_ENTITY_DEPTH: usize = 64;
const MAX_EXPANDED_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENTITY_OVERHEAD: usize = 1024;

/// Expands internal general entities declared in the document type's
/// internal subset, returning the input borrowed when there is no subset to
/// expand. Parse errors (undefined references, budget exhaustion) are
/// collected for the sink to drain into a `parsererror` document
/// (<https://www.w3.org/TR/xml/#sec-references>).
pub(crate) fn expand_internal_general_entities(input: &str) -> (Cow<'_, str>, Vec<String>) {
    let mut errors = Vec::new();
    let Some((body_start, entities)) = scan_internal_general_entities(input) else {
        return (Cow::Borrowed(input), errors);
    };
    let mut output = String::with_capacity(input.len());
    output.push_str(&input[..body_start]);
    let mut budget = MAX_EXPANDED_BYTES;
    let completed = expand_content(
        &input[body_start..],
        &entities,
        &mut output,
        &mut budget,
        &mut errors,
    );
    if !completed {
        report(
            &mut errors,
            format_args!("entity expansion budget exhausted"),
        );
    }
    (Cow::Owned(output), errors)
}

/// Records `message` unless an earlier error already failed the document.
fn report(errors: &mut Vec<String>, message: std::fmt::Arguments<'_>) {
    if errors.is_empty() {
        errors.push(message.to_string());
    }
}

/// The byte offset of the document type declaration's `<!DOCTYPE`, which
/// may only follow the prolog's optional byte order mark, XML declaration,
/// comments, processing instructions, and whitespace
/// (<https://www.w3.org/TR/xml/#NT-prolog>). A `<!DOCTYPE` inside a comment,
/// a processing instruction, or anywhere after the first other markup is
/// not a declaration, so it declares no entities.
///
/// The keyword matches ASCII case-insensitively to agree with the
/// tokenizer's `eat` (which uses `eq_ignore_ascii_case`): the XML spec wants
/// uppercase-only `DOCTYPE`
/// (<https://www.w3.org/TR/xml/#NT-doctypedecl>), but both layers stay
/// lenient together so entity expansion and tokenizing agree.
fn find_doctype_keyword(input: &str) -> Option<usize> {
    let mut pos = input
        .strip_prefix('\u{feff}')
        .map_or(0, |_| '\u{feff}'.len_utf8());
    loop {
        let rest = &input[pos..];
        let markup = rest.trim_start_matches(is_xml_whitespace);
        pos += rest.len() - markup.len();
        if let Some(body) = markup.strip_prefix("<!--") {
            pos += "<!--".len() + body.find("-->")? + "-->".len();
        } else if let Some(body) = markup.strip_prefix("<?") {
            pos += "<?".len() + body.find("?>")? + "?>".len();
        } else if markup
            .get(.."<!DOCTYPE".len())
            .is_some_and(|keyword| keyword.eq_ignore_ascii_case("<!DOCTYPE"))
        {
            return Some(pos);
        } else {
            return None;
        }
    }
}

fn scan_internal_general_entities(input: &str) -> Option<(usize, HashMap<String, String>)> {
    let doctype = find_doctype_keyword(input)?;
    let after_keyword = doctype + "<!DOCTYPE".len();
    // `<!DOCTYPE` requires whitespace before the name
    // (<https://www.w3.org/TR/xml/#NT-doctypedecl>).
    if !input[after_keyword..]
        .chars()
        .next()
        .is_some_and(is_xml_whitespace)
    {
        return None;
    }
    let mut quote = None;
    let mut subset_start = None;
    for (index, character) in input[after_keyword..].char_indices() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => quote = Some(character),
            '[' => {
                subset_start = Some(after_keyword + index + 1);
                break;
            },
            '>' => return None,
            _ => {},
        }
    }
    let subset_start = subset_start?;
    let subset = &input[subset_start..];
    let end = find_subset_end(subset)?;
    let after_bracket = subset_start + end + 1;
    let mut body_start = after_bracket;
    while body_start < input.len() {
        let rest = &input[body_start..];
        if rest.starts_with('>') {
            body_start += 1;
            break;
        }
        let Some(next) = rest.chars().next() else {
            break;
        };
        body_start += next.len_utf8();
    }
    Some((
        body_start,
        parse_internal_general_entities(&input[subset_start..subset_start + end]),
    ))
}

/// Finds the `]` ending the internal subset, skipping quoted literals,
/// comments, and processing instructions. The first `]` outside those ends
/// the subset; `[` nesting is not tracked (internal-subset markup cannot
/// contain a bare `[` outside a literal).
fn find_subset_end(subset: &str) -> Option<usize> {
    let bytes = subset.as_bytes();
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if b == b'"' || b == b'\'' {
            quote = Some(b);
            i += 1;
            continue;
        }
        if subset[i..].starts_with("<!--") {
            match subset[i..].find("-->") {
                Some(end) => {
                    i += end + 3;
                    continue;
                },
                None => return None,
            }
        }
        if subset[i..].starts_with("<?") {
            match subset[i..].find("?>") {
                Some(end) => {
                    i += end + 2;
                    continue;
                },
                None => return None,
            }
        }
        if b == b']' {
            return Some(i);
        }
        // Advance by char to stay on UTF-8 boundaries.
        let ch_len = subset[i..].chars().next().map_or(1, |c| c.len_utf8());
        i += ch_len;
    }
    None
}

fn parse_internal_general_entities(subset: &str) -> HashMap<String, String> {
    let mut entities = HashMap::new();
    let mut pos = 0;
    while pos < subset.len() {
        let rest = &subset[pos..];
        if let Some(inside) = rest.strip_prefix("<!--") {
            match inside.find("-->") {
                Some(end) => pos += 4 + end + 3,
                None => break,
            }
            continue;
        }
        if rest.starts_with("<?") {
            match rest.find("?>") {
                Some(end) => pos += end + 2,
                None => break,
            }
            continue;
        }
        if let Some(after) = rest.strip_prefix("<!ENTITY") {
            // `S` is required after the keyword; `<!ENTITYfoo>` is not a
            // declaration (<https://www.w3.org/TR/xml/#sec-entity-decl>).
            // `%` introduces a parameter entity, which we skip.
            match after.chars().next() {
                Some(c) if is_xml_whitespace(c) || c == '%' => {},
                _ => {
                    pos += skip_markup(rest);
                    continue;
                },
            }
            let (consumed, entity) = parse_general_entity_decl(after);
            if let Some((name, value)) = entity {
                // First declaration wins
                // (<https://www.w3.org/TR/xml/#sec-entity-decl>). Beyond the
                // cap further declarations are ignored: their references stay
                // literal, exactly like undefined entities.
                if entities.len() < MAX_ENTITY_DECLS {
                    entities.entry(name).or_insert(value);
                }
            }
            pos += "<!ENTITY".len() + consumed;
            continue;
        }
        if rest.starts_with('<') {
            pos += skip_markup(rest);
            continue;
        }
        let Some(next) = rest.chars().next() else {
            break;
        };
        pos += next.len_utf8();
    }
    entities
}

fn parse_general_entity_decl(decl: &str) -> (usize, Option<(String, String)>) {
    let consumed = skip_decl_body(decl);
    let body = &decl[..consumed];
    let mut chars = body.chars().peekable();
    while chars.peek().is_some_and(|c| is_xml_whitespace(*c)) {
        chars.next();
    }
    if chars.peek() == Some(&'%') {
        return (consumed, None);
    }
    let mut name = String::new();
    while let Some(c) = chars.peek().copied() {
        if is_xml_whitespace(c) {
            break;
        }
        name.push(c);
        chars.next();
    }
    if name.is_empty() || !is_name(&name) {
        return (consumed, None);
    }
    while chars.peek().is_some_and(|c| is_xml_whitespace(*c)) {
        chars.next();
    }
    // External IDs start with the SYSTEM/PUBLIC keywords; anything else must
    // be the quoted internal value
    // (<https://www.w3.org/TR/xml/#sec-entity-decl>).
    let rest: String = chars.clone().collect();
    if rest.starts_with("SYSTEM")
        && rest["SYSTEM".len()..]
            .chars()
            .next()
            .is_none_or(is_xml_whitespace)
    {
        return (consumed, None);
    }
    if rest.starts_with("PUBLIC")
        && rest["PUBLIC".len()..]
            .chars()
            .next()
            .is_none_or(is_xml_whitespace)
    {
        return (consumed, None);
    }
    let Some(quote) = chars.next() else {
        return (consumed, None);
    };
    if quote != '"' && quote != '\'' {
        return (consumed, None);
    }
    let mut value = String::new();
    for c in chars.by_ref() {
        if c == quote {
            // An unparsed (`NDATA`) entity is not an internal general entity:
            // trailing content after the literal (beyond the `>` terminator)
            // disqualifies it
            // (<https://www.w3.org/TR/xml/#sec-entity-decl>).
            let trailing: String = chars.clone().collect();
            let trailing = trailing.trim_start_matches(is_xml_whitespace);
            let trailing = trailing
                .strip_suffix('>')
                .unwrap_or(trailing)
                .trim_end_matches(is_xml_whitespace);
            if !trailing.is_empty() {
                return (consumed, None);
            }
            return (consumed, Some((name, value)));
        }
        value.push(c);
    }
    (consumed, None)
}

/// Copies `text` into `output` when the remaining budget covers it. Returns
/// false without writing anything when it does not, so an oversized
/// replacement is never half-copied into the tree; the caller stops and the
/// top level reports the exhaustion.
fn push_budgeted(output: &mut String, text: &str, budget: &mut usize) -> bool {
    if text.len() > *budget {
        return false;
    }
    output.push_str(text);
    *budget -= text.len();
    true
}

/// How one `&...;` reference resolved.
enum RefExpansion {
    /// The reference was expanded; the caller advances by this many bytes.
    Consumed(usize),
    /// Not an internal general entity; the caller copies the `&` literally.
    Literal,
    /// The budget ran out; the caller stops and fails closed.
    Exhausted,
}

/// Decodes the character references an attribute literal would decode, so the
/// result can be re-encoded for the literal's delimiter without changing its
/// value (<https://www.w3.org/TR/xml/#AVNormalize>). Anything that is not a
/// well-formed character or predefined reference stays literal.
fn decode_char_refs(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        decoded.push_str(&rest[..amp]);
        let candidate = &rest[amp..];
        let replacement = candidate.find(';').and_then(|semi| {
            let body = &candidate[1..semi];
            let character = match body {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "apos" => Some('\''),
                "quot" => Some('"'),
                _ => body
                    .strip_prefix('#')
                    .and_then(|digits| match digits.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => digits.parse::<u32>().ok(),
                    })
                    .and_then(char::from_u32),
            };
            character.map(|character| (character, semi + 1))
        });
        match replacement {
            Some((character, consumed)) => {
                decoded.push(character);
                rest = &candidate[consumed..];
            },
            None => {
                decoded.push('&');
                rest = &candidate[1..];
            },
        }
    }
    decoded.push_str(rest);
    Cow::Owned(decoded)
}

/// Writes `text` into a quoted attribute literal: `&` and the active quote
/// become character references, so replacement text cannot start a new
/// reference or end the literal early
/// (<https://www.w3.org/TR/xml/#AVNormalize>).
fn push_literal(output: &mut String, text: &str, quote: char, budget: &mut usize) -> bool {
    let decoded = decode_char_refs(text);
    let mut encoded = String::with_capacity(decoded.len());
    for character in decoded.chars() {
        if character == '&' {
            encoded.push_str("&amp;");
        } else if character == quote {
            encoded.push_str(if character == '"' { "&#34;" } else { "&#39;" });
        } else {
            encoded.push(character);
        }
    }
    push_budgeted(output, &encoded, budget)
}

fn expand_content(
    content: &str,
    entities: &HashMap<String, String>,
    output: &mut String,
    budget: &mut usize,
    errors: &mut Vec<String>,
) -> bool {
    let mut pos = 0;
    while pos < content.len() {
        let mark = output.len();
        let rest = &content[pos..];
        if rest.starts_with("<!--") || rest.starts_with("<![CDATA[") || rest.starts_with("<?") {
            let consumed = if rest.starts_with("<!--") {
                rest.find("-->").map_or(rest.len(), |end| end + 3)
            } else if rest.starts_with("<![CDATA[") {
                rest.find("]]>").map_or(rest.len(), |end| end + 3)
            } else {
                rest.find("?>").map_or(rest.len(), |end| end + 2)
            };
            if !push_budgeted(output, &rest[..consumed], budget) {
                output.truncate(mark);
                output.push_str(&content[pos..]);
                return false;
            }
            pos += consumed;
            continue;
        }
        if rest.starts_with('<') {
            let consumed = skip_markup(rest);
            if !expand_tag(&rest[..consumed], entities, output, budget, errors) {
                output.truncate(mark);
                output.push_str(&content[pos..]);
                return false;
            }
            pos += consumed;
            continue;
        }
        if rest.starts_with('&') {
            match expand_entity_ref(rest, entities, output, budget, errors, 0) {
                RefExpansion::Consumed(consumed) => pos += consumed,
                RefExpansion::Literal => {
                    // Malformed references stay literal here; the XML
                    // tokenizer's char-ref path reports the well-formedness
                    // error downstream. Undefined valid-shaped `&name;`
                    // already pushed an error.
                    if !push_budgeted(output, "&", budget) {
                        output.truncate(mark);
                        output.push_str(&content[pos..]);
                        return false;
                    }
                    pos += 1;
                },
                RefExpansion::Exhausted => {
                    output.truncate(mark);
                    output.push_str(&content[pos..]);
                    return false;
                },
            }
            continue;
        }
        let next = rest.find(['<', '&']).unwrap_or(rest.len());
        if !push_budgeted(output, &rest[..next], budget) {
            output.truncate(mark);
            output.push_str(&content[pos..]);
            return false;
        }
        pos += next;
    }
    true
}

/// Copies a tag verbatim while expanding entity references inside quoted
/// attribute values (<https://www.w3.org/TR/xml/#AVNormalize>). A reference's
/// replacement is re-encoded for the literal, so its quotes and ampersands
/// cannot re-delimit the value. Markup in replacement text is *not*
/// re-tokenized here: attribute values cannot contain `<`
/// (<https://www.w3.org/TR/xml/#forbidden>), and a `<`-bearing value stays
/// literal for the tokenizer to reject. Returns false when the budget ran out.
fn expand_tag(
    tag: &str,
    entities: &HashMap<String, String>,
    output: &mut String,
    budget: &mut usize,
    errors: &mut Vec<String>,
) -> bool {
    let mut quote: Option<char> = None;
    let mut i = 0;
    let mut text_start = 0;
    while i < tag.len() {
        let rest = &tag[i..];
        let c = rest.chars().next().unwrap();
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == '&' {
                if !push_budgeted(output, &tag[text_start..i], budget) {
                    return false;
                }
                let mut expanded = String::new();
                match expand_entity_ref(rest, entities, &mut expanded, budget, errors, 0) {
                    RefExpansion::Consumed(consumed) => {
                        if !push_literal(output, &expanded, q, budget) {
                            return false;
                        }
                        i += consumed;
                        text_start = i;
                        continue;
                    },
                    RefExpansion::Literal => {
                        if !push_budgeted(output, "&", budget) {
                            return false;
                        }
                        i += 1;
                        text_start = i;
                        continue;
                    },
                    RefExpansion::Exhausted => return false,
                }
            }
            i += c.len_utf8();
            continue;
        }
        if c == '"' || c == '\'' {
            quote = Some(c);
        }
        i += c.len_utf8();
    }
    push_budgeted(output, &tag[text_start..], budget)
}

fn expand_entity_ref(
    rest: &str,
    entities: &HashMap<String, String>,
    output: &mut String,
    budget: &mut usize,
    errors: &mut Vec<String>,
    depth: usize,
) -> RefExpansion {
    let Some(body) = rest.strip_prefix('&') else {
        return RefExpansion::Literal;
    };
    if body.starts_with('#') {
        return RefExpansion::Literal;
    }
    let Some(name_end) = body.find(';') else {
        return RefExpansion::Literal;
    };
    let name = &body[..name_end];
    if !is_name(name) {
        return RefExpansion::Literal;
    }
    if is_predefined_entity(name) {
        return RefExpansion::Literal;
    }
    let Some(value) = entities.get(name).cloned() else {
        // Undeclared general entity: well-formedness violation, fail closed
        // (<https://www.w3.org/TR/xml/#sec-references>).
        report(errors, format_args!("undefined entity &{name};"));
        return RefExpansion::Literal;
    };
    // Each top-level reference gets a fresh cycle set; it guards only within
    // one expansion, so diamond (non-cyclic) sharing across siblings
    // re-expands from scratch and the output budget (not `seen`) bounds it.
    let mut seen = HashSet::new();
    seen.insert(name.to_owned());
    if !expand_replacement(&value, entities, output, budget, errors, &mut seen, depth) {
        return RefExpansion::Exhausted;
    }
    RefExpansion::Consumed(name_end + 2)
}

fn expand_replacement(
    value: &str,
    entities: &HashMap<String, String>,
    output: &mut String,
    budget: &mut usize,
    errors: &mut Vec<String>,
    seen: &mut HashSet<String>,
    depth: usize,
) -> bool {
    // The value is cloned at the call site so `entities` is never borrowed
    // while `output` grows; replacement text only ever appends.
    let mut pos = 0;
    while pos < value.len() {
        if *budget == 0 {
            return false;
        }
        let rest = &value[pos..];
        if let Some(amp) = rest.find('&') {
            if !push_budgeted(output, &rest[..amp], budget) {
                return false;
            }
            let ref_rest = &rest[amp..];
            match expand_nested_ref(ref_rest, entities, output, budget, errors, seen, depth) {
                RefExpansion::Consumed(consumed) => pos += amp + consumed,
                RefExpansion::Literal => {
                    if !push_budgeted(output, "&", budget) {
                        return false;
                    }
                    pos += amp + 1;
                },
                RefExpansion::Exhausted => return false,
            }
        } else {
            if !push_budgeted(output, rest, budget) {
                return false;
            }
            break;
        }
    }
    true
}

fn expand_nested_ref(
    rest: &str,
    entities: &HashMap<String, String>,
    output: &mut String,
    budget: &mut usize,
    errors: &mut Vec<String>,
    seen: &mut HashSet<String>,
    depth: usize,
) -> RefExpansion {
    let Some(body) = rest.strip_prefix('&') else {
        return RefExpansion::Literal;
    };
    if body.starts_with('#') {
        return RefExpansion::Literal;
    }
    let Some(name_end) = body.find(';') else {
        return RefExpansion::Literal;
    };
    let name = &body[..name_end];
    // Depth caps the call stack; the `seen` set still rejects cycles below it.
    // The budget covers output bytes (via `push_budgeted`), but each nested
    // expansion also costs traversal work (HashSet insert/remove plus string
    // allocation per reference), so every expansion prepays a flat overhead:
    // without it an attacker trades bounded output for unbounded CPU.
    if *budget == 0 || !is_name(name) || is_predefined_entity(name) {
        return RefExpansion::Literal;
    }
    if depth >= MAX_ENTITY_DEPTH {
        report(
            errors,
            format_args!("entity nesting deeper than {MAX_ENTITY_DEPTH} at &{name};"),
        );
        return RefExpansion::Literal;
    }
    let Some(value) = entities.get(name).cloned() else {
        report(errors, format_args!("undefined entity &{name};"));
        return RefExpansion::Literal;
    };
    if !seen.insert(name.to_owned()) {
        report(errors, format_args!("cyclic entity &{name};"));
        return RefExpansion::Literal;
    }
    *budget = budget.saturating_sub(MAX_ENTITY_OVERHEAD);
    let completed = expand_replacement(&value, entities, output, budget, errors, seen, depth + 1);
    seen.remove(name);
    if !completed {
        return RefExpansion::Exhausted;
    }
    RefExpansion::Consumed(name_end + 2)
}

fn skip_decl_body(decl: &str) -> usize {
    let mut quote = None;
    for (index, character) in decl.char_indices() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => quote = Some(character),
            '>' => return index + character.len_utf8(),
            _ => {},
        }
    }
    decl.len()
}

fn skip_markup(rest: &str) -> usize {
    let mut quote = None;
    for (index, character) in rest.char_indices().skip(1) {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => quote = Some(character),
            '>' => return index + character.len_utf8(),
            _ => {},
        }
    }
    rest.len()
}

fn is_xml_whitespace(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\r' | '\n')
}

/// Predefined XML entities handled by the tokenizer's char-ref path, not by
/// general-entity expansion (<https://www.w3.org/TR/xml/#sec-predefined-ent>).
fn is_predefined_entity(name: &str) -> bool {
    matches!(name, "amp" | "lt" | "gt" | "apos" | "quot")
}

#[cfg(test)]
mod tests {
    use super::{expand_internal_general_entities, MAX_EXPANDED_BYTES};

    fn expand(input: &str) -> String {
        expand_internal_general_entities(input).0.into_owned()
    }

    fn doctype_with(decls: &str, body: &str) -> String {
        format!("<!DOCTYPE r [{decls}]><r>{body}</r>")
    }

    #[test]
    fn legitimate_entities_still_expand() {
        let input = doctype_with("<!ENTITY name \"world\">", "hello &name;!");
        let expanded = expand(&input);
        assert!(
            expanded.contains("hello world!"),
            "unexpected expansion: {expanded:?}"
        );
    }

    #[test]
    fn attribute_value_entities_expand() {
        let input = doctype_with("<!ENTITY a \"X\">", "<r attr=\"&a;\"/>");
        let expanded = expand(&input);
        assert!(
            expanded.contains("attr=\"X\""),
            "attribute value not expanded: {expanded:?}"
        );
    }

    #[test]
    fn first_declaration_wins() {
        let input = doctype_with("<!ENTITY a \"1\"><!ENTITY a \"2\">", "&a;");
        let expanded = expand(&input);
        assert!(
            expanded.contains(">1<"),
            "last-wins, expected first: {expanded:?}"
        );
    }

    #[test]
    fn ndata_entities_do_not_expand() {
        let input = doctype_with("<!ENTITY logo \"x\" NDATA png>", "&logo;");
        let expanded = expand(&input);
        assert!(
            expanded.contains("&logo;"),
            "unparsed entity expanded: {expanded:?}"
        );
    }

    #[test]
    fn undefined_entities_report_errors() {
        let input = doctype_with("", "&nosuch;");
        let (expanded, errors) = super::expand_internal_general_entities(&input);
        assert!(expanded.contains("&nosuch;"));
        assert!(
            errors.iter().any(|e| e.contains("nosuch")),
            "missing undefined-entity error: {errors:?}"
        );
    }

    #[test]
    fn push_budgeted_floors_to_a_char_boundary() {
        let mut out = String::new();
        let mut budget = 1;
        assert!(!super::push_budgeted(&mut out, "éé", &mut budget));
        assert_eq!(out, "");
        assert_eq!(budget, 1);
    }

    fn errors_of(input: &str) -> Vec<String> {
        super::expand_internal_general_entities(input).1
    }

    #[test]
    fn nesting_past_the_depth_cap_fails_closed() {
        let mut decls = String::from("<!ENTITY a0 \"x\">");
        for level in 1..=super::MAX_ENTITY_DEPTH + 1 {
            decls.push_str(&format!("<!ENTITY a{level} \"&a{};\">", level - 1));
        }
        let top = super::MAX_ENTITY_DEPTH + 1;
        let errors = errors_of(&doctype_with(&decls, &format!("&a{top};")));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("nesting"), "{errors:?}");
        let shallow = errors_of(&doctype_with(&decls, "&a3;"));
        assert!(shallow.is_empty(), "{shallow:?}");
    }

    #[test]
    fn only_the_first_error_is_recorded() {
        let body = "&nosuch;".repeat(10_000);
        let errors = errors_of(&doctype_with("", &body));
        assert_eq!(errors, ["undefined entity &nosuch;"]);
    }

    #[test]
    fn doctype_inside_a_comment_or_pi_declares_nothing() {
        for input in [
            "<!-- <!DOCTYPE r [<!ENTITY a \"X\">]> --><r>&a;</r>",
            "<?pi <!DOCTYPE r [<!ENTITY a \"X\">]> ?><r>&a;</r>",
            "<r><!DOCTYPE r [<!ENTITY a \"X\">]>&a;</r>",
        ] {
            assert_eq!(expand(input), input);
        }
    }

    #[test]
    fn doctype_after_prolog_markup_still_declares() {
        let input = "\u{feff}<?xml version=\"1.0\"?>\n<!-- c --><?pi x?> <!doctype r [<!ENTITY a \"X\">]><r>&a;</r>";
        assert!(expand(input).ends_with("<r>X</r>"), "{}", expand(input));
    }

    #[test]
    fn entity_cycles_stay_literal() {
        let input = doctype_with("<!ENTITY a \"&b;\"><!ENTITY b \"&a;\">", "&a;");
        let expanded = expand(&input);
        assert!(
            expanded.len() < 1024,
            "cycle exploded: {} bytes",
            expanded.len()
        );
    }

    // Ten doubling levels turn ~100 bytes into ~100KB legitimately; without
    // a budget the same shape at depth 30+ exhausts memory (billion laughs).
    #[test]
    fn exponential_entity_chain_stays_bounded() {
        let mut decls = String::from("<!ENTITY a0 \"x\">");
        for level in 1..30 {
            decls.push_str(&format!(
                "<!ENTITY a{level} \"&a{};&a{};\">",
                level - 1,
                level - 1
            ));
        }
        let input = doctype_with(&decls, "&a29;");
        let start = std::time::Instant::now();
        let expanded = expand(&input);
        let elapsed = start.elapsed();
        assert!(
            expanded.len() <= input.len() + MAX_EXPANDED_BYTES,
            "expansion escaped the budget: {} bytes",
            expanded.len()
        );
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "expansion took too long: {elapsed:?}"
        );
    }

    // A replacement containing the literal's own delimiter must not split the
    // attribute in two; it is re-encoded as character references.
    #[test]
    fn quote_in_replacement_cannot_re_delimit_a_literal() {
        let input = doctype_with("<!ENTITY q '\" b=\"c'>", "<r a=\"&q;\"/>");
        let expanded = expand(&input);
        assert!(
            expanded.contains("a=\"&#34; b=&#34;c\""),
            "literal not encoded: {expanded:?}"
        );
    }

    // A decoded ampersand must stay data, not start a new reference.
    #[test]
    fn ampersand_in_replacement_stays_literal_in_attributes() {
        let input = doctype_with("<!ENTITY q \"&#38;\">", "<r a=\"&q;\"/>");
        let expanded = expand(&input);
        assert!(
            expanded.contains("a=\"&amp;\""),
            "ampersand not encoded: {expanded:?}"
        );
    }

    // Predefined references the tokenizer resolves are left alone.
    #[test]
    fn predefined_references_in_literals_stay_unchanged() {
        let input = doctype_with("", "<r a=\"&amp;&#38;\"/>");
        let expanded = expand(&input);
        assert!(
            expanded.contains("a=\"&amp;&#38;\""),
            "predefined reference rewritten: {expanded:?}"
        );
    }
}
