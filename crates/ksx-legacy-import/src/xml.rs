//! Small shared helpers over `quick-xml` pull parsing.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// Collect an element's attributes as owned (name, value) pairs.
/// Namespace declarations (`xmlns*`) are dropped. Malformed attributes yield
/// `Err(reason)` so the caller can warn and skip the element.
pub(crate) fn attrs(el: &BytesStart<'_>) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for attr in el.attributes() {
        let attr = attr.map_err(|e| format!("malformed attribute: {e}"))?;
        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
        if key == "xmlns" || key.starts_with("xmlns:") {
            continue;
        }
        let value = attr
            .unescape_value()
            .map_err(|e| format!("malformed attribute value: {e}"))?
            .into_owned();
        out.push((key, value));
    }
    Ok(out)
}

/// Look up an attribute by exact (case-sensitive) name.
pub(crate) fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Read the text content of the element just opened by `start`, consuming
/// events up to and including its end tag. Nested elements are skipped.
pub(crate) fn element_text(
    reader: &mut Reader<&[u8]>,
    start: &BytesStart<'_>,
) -> Result<String, String> {
    let mut text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Text(t)) => {
                text.push_str(&t.unescape().map_err(|e| format!("malformed text: {e}"))?);
            }
            Ok(Event::CData(t)) => {
                text.push_str(&String::from_utf8_lossy(&t.into_inner()));
            }
            Ok(Event::Start(inner)) => {
                let name = inner.name().to_owned();
                reader
                    .read_to_end(name)
                    .map_err(|e| format!("unclosed nested element: {e}"))?;
            }
            Ok(Event::End(end)) if end.name() == start.name() => return Ok(text),
            Ok(Event::End(_)) => return Err("mismatched end tag".to_owned()),
            Ok(Event::Eof) => return Err("unexpected end of file inside element".to_owned()),
            Ok(_) => {}
            Err(e) => return Err(format!("XML error: {e}")),
        }
    }
}

/// Render an element + its (already collected) attributes for warning messages,
/// e.g. `button id="99"`.
pub(crate) fn describe(name: &str, attrs: &[(String, String)]) -> String {
    let mut out = String::from(name);
    for (k, v) in attrs {
        out.push_str(&format!(" {k}=\"{v}\""));
    }
    out
}
