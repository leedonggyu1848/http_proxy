//! XML body previews: a bounded, non-resolving parse into a node list that
//! the Pretty printer and the Tree view share.
//!
//! The parser keeps the byte range of every node so both views show the
//! original text. It never reads a DTD or expands entity references, so
//! external entities and entity-expansion bombs are shown as written.

mod pretty;
pub(in crate::content::format) mod tree;

pub(super) use pretty::pretty;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use std::ops::Range;

/// Deepest element nesting read before the preview stops.
pub(super) const MAX_DEPTH: usize = 256;
/// Most nodes read before the preview stops.
pub(super) const MAX_NODES: usize = 10_000;

/// Reason XML input could not be parsed or stopped before its end.
#[derive(Debug, thiserror::Error)]
pub enum XmlFormatError {
    #[error("{source} at byte {offset}")]
    Syntax {
        #[source]
        source: quick_xml::Error,
        offset: usize,
    },
    #[error("more than {max_nodes} nodes")]
    TooManyNodes { max_nodes: usize },
    #[error("nested deeper than {max_depth} elements")]
    TooDeep { max_depth: usize },
    #[error("<{name}> is not closed")]
    UnclosedElement { name: String },
    #[error("no elements found")]
    NoElements,
}

/// What a node of the document is.
#[derive(Clone, Debug, Eq, PartialEq)]
enum NodeKind {
    Element(Element),
    Text,
    /// `&name;` or `&#…;`, kept unexpanded.
    Entity,
    CData,
    Comment,
    /// `<?xml …?>` and other processing instructions, and `<!DOCTYPE …>`.
    Instruction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Element {
    /// Qualified name, such as `soap:Body`.
    name: Range<usize>,
    /// The closing tag; `None` for `<a/>` and for elements cut off before it.
    end_tag: Option<Range<usize>>,
    self_closing: bool,
    children: Vec<usize>,
    /// Whether `xml:space="preserve"` applies, so whitespace is content.
    preserve: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Node {
    kind: NodeKind,
    /// The node in the source; the opening tag for elements.
    range: Range<usize>,
}

/// An XML preview read as far as it is well formed and within limits.
#[derive(Debug)]
pub(super) struct Document<'a> {
    text: &'a str,
    nodes: Vec<Node>,
    roots: Vec<usize>,
    /// Why reading stopped before the end of a well-formed document.
    incomplete: Option<XmlFormatError>,
}

impl<'a> Document<'a> {
    /// Reads `text` until it ends, is malformed, or exceeds [`MAX_DEPTH`] or
    /// [`MAX_NODES`]. Fails only when no element was read at all.
    pub(super) fn parse(text: &'a str) -> Result<Self, XmlFormatError> {
        let mut parser = Parser {
            document: Document {
                text,
                nodes: Vec::new(),
                roots: Vec::new(),
                incomplete: None,
            },
            open: Vec::new(),
        };
        parser.run();
        let document = parser.document;
        if document
            .nodes
            .iter()
            .any(|node| matches!(node.kind, NodeKind::Element(_)))
        {
            Ok(document)
        } else {
            Err(document.incomplete.unwrap_or(XmlFormatError::NoElements))
        }
    }

    /// Notice for a document that was not read to its end.
    pub(super) fn incomplete_notice(&self) -> Option<String> {
        self.incomplete
            .as_ref()
            .map(|reason| format!("XML is incomplete ({reason}); showing the part read so far."))
    }

    fn node(&self, index: usize) -> Option<&Node> {
        self.nodes.get(index)
    }

    fn slice(&self, range: &Range<usize>) -> &'a str {
        self.text.get(range.clone()).unwrap_or_default()
    }
}

struct Parser<'a> {
    document: Document<'a>,
    /// Indexes of the elements whose closing tag has not been read yet.
    open: Vec<usize>,
}

impl Parser<'_> {
    fn run(&mut self) {
        let text = self.document.text;
        let mut reader = Reader::from_str(text);
        let config = reader.config_mut();
        config.check_end_names = true;
        config.expand_empty_elements = false;
        config.trim_text(false);
        let offset = |position: u64| usize::try_from(position).unwrap_or(text.len());
        loop {
            let start = offset(reader.buffer_position());
            let event = match reader.read_event() {
                Ok(Event::Eof) => {
                    self.finish_eof();
                    return;
                }
                Ok(event) => event,
                Err(source) => {
                    self.stop(XmlFormatError::Syntax {
                        source,
                        offset: offset(reader.error_position()),
                    });
                    return;
                }
            };
            let range = start..offset(reader.buffer_position());
            if let Err(reason) = self.add(&event, range) {
                self.stop(reason);
                return;
            }
        }
    }

    fn add(&mut self, event: &Event<'_>, range: Range<usize>) -> Result<(), XmlFormatError> {
        if let Event::End(_) = event {
            if let Some(NodeKind::Element(element)) = self
                .open
                .pop()
                .and_then(|index| self.document.nodes.get_mut(index))
                .map(|node| &mut node.kind)
            {
                element.end_tag = Some(range);
            }
            return Ok(());
        }
        if self.document.nodes.len() >= MAX_NODES {
            return Err(XmlFormatError::TooManyNodes {
                max_nodes: MAX_NODES,
            });
        }
        let kind = match event {
            Event::Start(start) | Event::Empty(start) => {
                if self.open.len() >= MAX_DEPTH {
                    return Err(XmlFormatError::TooDeep {
                        max_depth: MAX_DEPTH,
                    });
                }
                let self_closing = matches!(event, Event::Empty(_));
                NodeKind::Element(self.element(start, range.start, self_closing))
            }
            Event::Text(_) => NodeKind::Text,
            Event::GeneralRef(_) => NodeKind::Entity,
            Event::CData(_) => NodeKind::CData,
            Event::Comment(_) => NodeKind::Comment,
            Event::Decl(_) | Event::PI(_) | Event::DocType(_) | Event::End(_) | Event::Eof => {
                NodeKind::Instruction
            }
        };
        let opens = matches!(event, Event::Start(_));
        let index = self.document.nodes.len();
        self.document.nodes.push(Node { kind, range });
        let siblings = match self.open.last().copied() {
            Some(parent) => match self
                .document
                .nodes
                .get_mut(parent)
                .map(|node| &mut node.kind)
            {
                Some(NodeKind::Element(parent)) => &mut parent.children,
                _ => &mut self.document.roots,
            },
            None => &mut self.document.roots,
        };
        siblings.push(index);
        if opens {
            self.open.push(index);
        }
        Ok(())
    }

    fn element(&self, start: &BytesStart<'_>, at: usize, self_closing: bool) -> Element {
        let name_start = at + 1;
        let inherited = self
            .open
            .last()
            .and_then(|&parent| self.document.node(parent))
            .is_some_and(|node| matches!(&node.kind, NodeKind::Element(e) if e.preserve));
        let space = start
            .attributes()
            .flatten()
            .find(|attribute| attribute.key.as_ref() == b"xml:space")
            .map(|attribute| attribute.value.into_owned());
        let preserve = match space.as_deref() {
            Some(b"preserve") => true,
            Some(b"default") => false,
            _ => inherited,
        };
        Element {
            name: name_start..name_start + start.name().as_ref().len(),
            end_tag: None,
            self_closing,
            children: Vec::new(),
            preserve,
        }
    }

    fn finish_eof(&mut self) {
        if let Some(name) = self
            .open
            .last()
            .and_then(|&index| self.document.node(index))
            .and_then(|node| match &node.kind {
                NodeKind::Element(element) => Some(self.document.slice(&element.name)),
                _ => None,
            })
        {
            self.stop(XmlFormatError::UnclosedElement {
                name: name.to_owned(),
            });
        }
    }

    fn stop(&mut self, reason: XmlFormatError) {
        self.document.incomplete = Some(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(document: &Document<'_>) -> Vec<&'static str> {
        document
            .nodes
            .iter()
            .map(|node| match node.kind {
                NodeKind::Element(_) => "element",
                NodeKind::Text => "text",
                NodeKind::Entity => "entity",
                NodeKind::CData => "cdata",
                NodeKind::Comment => "comment",
                NodeKind::Instruction => "instruction",
            })
            .collect()
    }

    #[test]
    fn node_ranges_cover_the_source() -> Result<(), XmlFormatError> {
        let text =
            "<?xml version=\"1.0\"?>\n<a x='1'><!-- c --><b>t &amp; u</b><![CDATA[<raw>]]><c/></a>";
        let document = Document::parse(text)?;
        let mut rebuilt = String::new();
        for node in &document.nodes {
            rebuilt.push_str(document.slice(&node.range));
            if let NodeKind::Element(Element {
                end_tag: Some(end), ..
            }) = &node.kind
            {
                // Closing tags follow their last child; check them separately.
                assert!(document.slice(end).starts_with("</"));
            }
        }
        assert_eq!(
            rebuilt,
            "<?xml version=\"1.0\"?>\n<a x='1'><!-- c --><b>t &amp; u<![CDATA[<raw>]]><c/>"
        );
        assert_eq!(
            kinds(&document),
            [
                "instruction",
                "text",
                "element",
                "comment",
                "element",
                "text",
                "entity",
                "text",
                "cdata",
                "element"
            ]
        );
        assert!(document.incomplete.is_none());
        Ok(())
    }

    #[test]
    fn stops_at_truncated_or_malformed_input() -> Result<(), XmlFormatError> {
        let truncated = Document::parse("<rss><channel><title>Hi</title><item")?;
        assert!(truncated.incomplete.is_some());
        let unclosed = Document::parse("<rss><channel><title>Hi</title>")?;
        assert!(matches!(
            unclosed.incomplete,
            Some(XmlFormatError::UnclosedElement { ref name }) if name == "channel"
        ));
        assert_eq!(
            unclosed
                .incomplete
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("<channel> is not closed")
        );
        let mismatched = Document::parse("<a><b></a>")?;
        assert!(matches!(
            mismatched.incomplete,
            Some(XmlFormatError::Syntax { .. })
        ));
        assert!(
            mismatched
                .incomplete
                .as_ref()
                .map(ToString::to_string)
                .is_some_and(|reason| reason.contains("byte"))
        );
        assert!(matches!(
            Document::parse("plain text"),
            Err(XmlFormatError::NoElements)
        ));
        assert!(matches!(
            Document::parse(""),
            Err(XmlFormatError::NoElements)
        ));
        Ok(())
    }

    #[test]
    fn limits_depth_and_node_count() -> Result<(), XmlFormatError> {
        let deep = "<a>".repeat(MAX_DEPTH + 10);
        let document = Document::parse(&deep)?;
        assert_eq!(document.nodes.len(), MAX_DEPTH);
        assert!(matches!(
            document.incomplete,
            Some(XmlFormatError::TooDeep {
                max_depth: MAX_DEPTH
            })
        ));
        assert!(
            document
                .incomplete
                .as_ref()
                .map(ToString::to_string)
                .is_some_and(|reason| reason.contains("nested deeper"))
        );
        let wide = format!("<r>{}</r>", "<i/>".repeat(MAX_NODES + 10));
        let document = Document::parse(&wide)?;
        assert_eq!(document.nodes.len(), MAX_NODES);
        assert!(matches!(
            document.incomplete,
            Some(XmlFormatError::TooManyNodes {
                max_nodes: MAX_NODES
            })
        ));
        assert!(
            document
                .incomplete
                .as_ref()
                .map(ToString::to_string)
                .is_some_and(|reason| reason.contains("nodes"))
        );
        Ok(())
    }

    #[test]
    fn never_resolves_dtd_or_external_entities() -> Result<(), XmlFormatError> {
        let text = concat!(
            "<!DOCTYPE r [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">",
            "<!ENTITY a \"aaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;\">]>",
            "<r>&xxe;&b;</r>"
        );
        let document = Document::parse(text)?;
        assert_eq!(
            kinds(&document),
            ["instruction", "element", "entity", "entity"]
        );
        let entities = document
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Entity)
            .map(|node| document.slice(&node.range))
            .collect::<Vec<_>>();
        assert_eq!(entities, ["&xxe;", "&b;"]);
        assert!(document.incomplete.is_none());
        Ok(())
    }

    #[test]
    fn tracks_xml_space_preserve() -> Result<(), XmlFormatError> {
        let document =
            Document::parse("<a xml:space=\"preserve\"><b><c xml:space='default'/></b></a>")?;
        let preserve = document
            .nodes
            .iter()
            .filter_map(|node| match &node.kind {
                NodeKind::Element(element) => Some(element.preserve),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(preserve, [true, true, false]);
        Ok(())
    }
}
