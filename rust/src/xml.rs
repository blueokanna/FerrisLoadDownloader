//! Minimal, safe XML parser used for DASH MPD manifests.
//!
//! This is deliberately **not** a general-purpose XML library. It parses
//! only the subset of XML that DASH MPD manifests use (elements,
//! attributes, text content) while hardening against the classic XML
//! attack classes:
//!
//! - **XXE / billion-laughs**: `<!DOCTYPE`, `<!ENTITY` and `<!ELEMENT`
//!   declarations are rejected outright; there is no entity expansion.
//! - **Deep nesting**: element depth is capped (`MAX_DEPTH`).
//! - **Unbounded input**: the caller enforces a byte limit; the parser
//!   additionally refuses comments/CDATA that run past the buffer.
//!
//! Only well-formed XML with a single root element is accepted.

use std::collections::BTreeMap;
use std::string::{String, ToString};

/// Maximum element nesting depth (protects the call stack).
pub const MAX_DEPTH: usize = 64;
/// Maximum length of an attribute name or value.
const MAX_ATTR_LEN: usize = 4096;

/// A parsed XML element tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// Element name (namespace prefix preserved as-is).
    pub name: String,
    /// Attributes in document order (BTreeMap loses order but is
    /// deterministic; DASH MPD does not depend on attribute order).
    pub attributes: BTreeMap<String, String>,
    /// Child elements.
    pub children: Vec<Element>,
    /// Concatenated text content of this element (excluding children).
    pub text: String,
}

impl Element {
    /// The value of an attribute, if present.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(String::as_str)
    }

    /// The first direct child with the given name.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    /// All direct children with the given name.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |child| child.name == name)
    }
}

/// An XML parse error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlError {
    message: String,
}

impl core::fmt::Display for XmlError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for XmlError {}

fn xml_error(message: impl Into<String>) -> XmlError {
    XmlError {
        message: message.into(),
    }
}

/// Parse a complete XML document into an element tree.
///
/// Rejects DOCTYPE/ENTITY declarations (XXE defense), non-UTF-8 input,
/// malformed tags, and overly deep nesting. `bytes` must contain exactly
/// one root element; trailing garbage is an error.
pub fn parse_xml(bytes: &[u8]) -> Result<Element, XmlError> {
    let text = std::str::from_utf8(bytes).map_err(|_| xml_error("XML is not valid UTF-8"))?;
    let mut parser = Parser {
        input: text,
        pos: 0,
        depth: 0,
    };
    parser.skip_prolog();
    let root = parser.parse_element()?;
    parser.skip_whitespace();
    if parser.pos < parser.input.len() {
        return Err(xml_error("trailing content after the root element"));
    }
    Ok(root)
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.pos += ch.len_utf8();
            } else {
                break;
            }
        }
    }

    /// Skip the XML declaration `<?xml ...?>` and comments/whitespace
    /// before the root element.
    fn skip_prolog(&mut self) {
        loop {
            self.skip_whitespace();
            if self.input[self.pos..].starts_with("<?xml") {
                if let Some(end) = self.input[self.pos..].find("?>") {
                    self.pos += end + 2;
                    continue;
                }
            }
            break;
        }
    }

    /// Expect `<!DOCTYPE` or `<!ENTITY` and reject it (XXE defense).
    fn reject_doctype(&mut self) -> Result<(), XmlError> {
        if self.input[self.pos..].starts_with("<!DOCTYPE")
            || self.input[self.pos..].starts_with("<!ENTITY")
            || self.input[self.pos..].starts_with("<!ELEMENT")
            || self.input[self.pos..].starts_with("<!ATTLIST")
        {
            let end = self.input[self.pos..]
                .find('>')
                .map(|i| self.pos + i + 1)
                .unwrap_or(self.input.len());
            let decl = &self.input[self.pos..end];
            return Err(xml_error(format!(
                "DOCTYPE/ENTITY declarations are not allowed: {}",
                decl.chars().take(40).collect::<String>()
            )));
        }
        Ok(())
    }

    fn parse_element(&mut self) -> Result<Element, XmlError> {
        if self.depth >= MAX_DEPTH {
            return Err(xml_error(format!(
                "XML nesting exceeds the maximum depth of {}",
                MAX_DEPTH
            )));
        }
        self.skip_whitespace();
        self.reject_doctype()?;
        if self.peek() != Some('<') {
            return Err(xml_error("expected an element start tag"));
        }
        self.pos += 1; // consume '<'
        self.skip_whitespace();
        let name = self.parse_name()?;
        if name.is_empty() {
            return Err(xml_error("empty element name"));
        }

        let mut attributes = BTreeMap::new();
        let mut self_closing = false;
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some('/') => {
                    self.pos += 1;
                    self.skip_whitespace();
                    if self.peek() != Some('>') {
                        return Err(xml_error("malformed self-closing tag"));
                    }
                    self.pos += 1;
                    self_closing = true;
                    break;
                }
                Some('>') => {
                    self.pos += 1;
                    break;
                }
                Some(_) => {
                    let attr_name = self.parse_name()?;
                    if attr_name.is_empty() {
                        return Err(xml_error("invalid attribute name"));
                    }
                    self.skip_whitespace();
                    if self.peek() != Some('=') {
                        return Err(xml_error("attribute is missing '='"));
                    }
                    self.pos += 1;
                    self.skip_whitespace();
                    let value = self.parse_attribute_value()?;
                    if attributes.insert(attr_name.clone(), value).is_some() {
                        return Err(xml_error(format!("duplicate attribute {attr_name:?}")));
                    }
                }
                None => return Err(xml_error("unterminated start tag")),
            }
        }

        if self_closing {
            return Ok(Element {
                name,
                attributes,
                children: Vec::new(),
                text: String::new(),
            });
        }

        self.depth += 1;
        let mut children = Vec::new();
        let mut text = String::new();
        loop {
            self.skip_whitespace();
            if self.pos >= self.input.len() {
                return Err(xml_error(format!("unterminated element <{name}>")));
            }
            if self.input[self.pos..].starts_with("</") {
                // End tag.
                let end_start = self.pos;
                self.pos += 2;
                let end_name = self.parse_name()?;
                self.skip_whitespace();
                if self.peek() != Some('>') {
                    return Err(xml_error("malformed end tag"));
                }
                self.pos += 1;
                if end_name != name {
                    return Err(xml_error(format!(
                        "mismatched end tag </{end_name}> expected </{name}> (at byte {end_start})"
                    )));
                }
                self.depth -= 1;
                return Ok(Element {
                    name,
                    attributes,
                    children,
                    text,
                });
            }
            if self.input[self.pos..].starts_with("<!--") {
                let close = self.input[self.pos..]
                    .find("-->")
                    .ok_or_else(|| xml_error("unterminated comment"))?;
                self.pos += close + 3;
                continue;
            }
            if self.input[self.pos..].starts_with("<![CDATA[") {
                let close = self.input[self.pos..]
                    .find("]]>")
                    .ok_or_else(|| xml_error("unterminated CDATA section"))?;
                let content = &self.input[self.pos + 9..self.pos + close];
                text.push_str(content);
                self.pos += close + 3;
                continue;
            }
            if self.input[self.pos..].starts_with("<!") {
                // Reject any other declaration (DOUBLE defense).
                return Err(xml_error("XML declarations are not allowed"));
            }
            if self.peek() == Some('<') {
                let child = self.parse_element()?;
                children.push(child);
                continue;
            }
            // Text content (up to the next '<').
            let text_start = self.pos;
            while self.pos < self.input.len() && !self.input[self.pos..].starts_with('<') {
                let ch = self.input[self.pos..].chars().next().unwrap();
                self.pos += ch.len_utf8();
            }
            let raw = &self.input[text_start..self.pos];
            text.push_str(&unescape_text(raw)?);
        }
    }

    fn parse_name(&mut self) -> Result<String, XmlError> {
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input[self.pos..].chars().next().unwrap();
            if ch.is_whitespace() || matches!(ch, '>' | '/' | '=') {
                break;
            }
            self.pos += ch.len_utf8();
        }
        Ok(self.input[start..self.pos].to_string())
    }

    fn parse_attribute_value(&mut self) -> Result<String, XmlError> {
        let quote = self
            .peek()
            .ok_or_else(|| xml_error("missing attribute value"))?;
        if quote != '"' && quote != '\'' {
            return Err(xml_error("attribute value must be quoted"));
        }
        self.pos += 1;
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input[self.pos..].chars().next().unwrap();
            if ch == quote {
                let raw = &self.input[start..self.pos];
                self.pos += 1;
                if raw.len() > MAX_ATTR_LEN {
                    return Err(xml_error("attribute value too long"));
                }
                return unescape_attribute(raw);
            }
            self.pos += ch.len_utf8();
        }
        Err(xml_error("unterminated attribute value"))
    }
}

/// Decode the five predefined XML entities in text content.
fn unescape_text(raw: &str) -> Result<String, XmlError> {
    if !raw.contains('&') {
        return Ok(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        rest = &rest[index..];
        if let Some(end) = rest.find(';') {
            let entity = &rest[..=end];
            match entity {
                "&amp;" => out.push('&'),
                "&lt;" => out.push('<'),
                "&gt;" => out.push('>'),
                "&quot;" => out.push('"'),
                "&apos;" => out.push('\''),
                _ => {
                    // Reject unknown entities (no DTD to resolve them).
                    return Err(xml_error(format!(
                        "unknown or unsupported XML entity: {}",
                        entity.chars().take(16).collect::<String>()
                    )));
                }
            }
            rest = &rest[end + 1..];
        } else {
            return Err(xml_error("unterminated XML entity reference"));
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// Decode entities in an attribute value (same set; quotes matter here).
fn unescape_attribute(raw: &str) -> Result<String, XmlError> {
    unescape_text(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_document() {
        let xml = br#"<MPD><Period><AdaptationSet contentType="video"><Representation id="1" bandwidth="800000" width="854" height="480"><BaseURL>https://cdn.example/480.mp4</BaseURL></Representation></AdaptationSet></Period></MPD>"#;
        let root = parse_xml(xml).expect("parse");
        assert_eq!(root.name, "MPD");
        let period = root.child("Period").expect("period");
        let adaptation = period.child("AdaptationSet").expect("adaptation");
        assert_eq!(adaptation.attr("contentType"), Some("video"));
        let representation = adaptation.child("Representation").expect("representation");
        assert_eq!(representation.attr("bandwidth"), Some("800000"));
        assert_eq!(representation.attr("width"), Some("854"));
        let base = representation.child("BaseURL").expect("baseurl");
        assert_eq!(base.text, "https://cdn.example/480.mp4");
    }

    #[test]
    fn parses_self_closing_and_attributes() {
        let xml = br#"<A x="1" y='two'><B/></A>"#;
        let root = parse_xml(xml).expect("parse");
        assert_eq!(root.name, "A");
        assert_eq!(root.attr("x"), Some("1"));
        assert_eq!(root.attr("y"), Some("two"));
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].name, "B");
    }

    #[test]
    fn rejects_doctype_xxe() {
        let xml = br#"<!DOCTYPE foo [<!ENTITY xxe SYSTEM "file:///etc/passwd">]><A>&xxe;</A>"#;
        assert!(parse_xml(xml).is_err());
    }

    #[test]
    fn rejects_deep_nesting() {
        let mut xml = String::new();
        for _ in 0..(MAX_DEPTH + 10) {
            xml.push_str("<A>");
        }
        for _ in 0..(MAX_DEPTH + 10) {
            xml.push_str("</A>");
        }
        assert!(parse_xml(xml.as_bytes()).is_err());
    }

    #[test]
    fn rejects_unterminated_element() {
        assert!(parse_xml(b"<A><B></A>").is_err());
        assert!(parse_xml(b"<A>").is_err());
    }

    #[test]
    fn handles_entities_and_text() {
        let xml = br#"<A>1 &lt; 2 &amp;&amp; 3</A>"#;
        let root = parse_xml(xml).expect("parse");
        assert_eq!(root.text, "1 < 2 && 3");
    }
}
