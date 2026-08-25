use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::{parse_document, Attribute, QualName};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::{Ref, RefCell};
use std::rc::Rc;
use url::Url;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteResourceKind {
    Image,
    CssBackground,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteResourceSource {
    ImgSrc,
    ImgSrcset,
    StyleAttribute,
    StyleBlock,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteResourceSignal {
    TinyDimensions,
    Hidden,
    QueryParameters,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RemoteResource {
    pub kind: RemoteResourceKind,
    pub source: RemoteResourceSource,
    pub url: String,
    pub scheme: String,
    pub host: Option<String>,
    pub declared_width: Option<u32>,
    pub declared_height: Option<u32>,
    pub hidden: bool,
    pub signals: Vec<RemoteResourceSignal>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HtmlRemoteEvidence {
    pub remote_resources: Vec<RemoteResource>,
    pub local_references: Vec<String>,
    pub links: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct Node {
    name: Option<String>,
    qual_name: Option<QualName>,
    attrs: Vec<(String, String)>,
    text: String,
}

struct DomSink {
    document: Rc<RefCell<Node>>,
    nodes: RefCell<Vec<Rc<RefCell<Node>>>>,
}

impl Default for DomSink {
    fn default() -> Self {
        let document = Rc::new(RefCell::new(Node::default()));
        Self {
            document,
            nodes: RefCell::new(Vec::new()),
        }
    }
}

impl DomSink {
    fn elements(&self) -> Vec<Node> {
        self.nodes
            .borrow()
            .iter()
            .filter_map(|node| {
                let node = node.borrow();
                node.name.is_some().then(|| node.clone())
            })
            .collect()
    }
}

impl TreeSink for DomSink {
    type Handle = Rc<RefCell<Node>>;
    type Output = Self;
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) -> Self {
        self
    }

    fn get_document(&self) -> Self::Handle {
        self.document.clone()
    }

    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        target.clone()
    }

    fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
        Rc::ptr_eq(x, y)
    }

    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> Self::ElemName<'a> {
        Ref::map(target.borrow(), |node| {
            node.qual_name.as_ref().expect("element name")
        })
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        _flags: ElementFlags,
    ) -> Self::Handle {
        let handle = Rc::new(RefCell::new(Node {
            name: Some(name.local.to_string()),
            qual_name: Some(name),
            attrs: attrs
                .into_iter()
                .map(|attr| (attr.name.local.to_string(), attr.value.to_string()))
                .collect(),
            ..Node::default()
        }));
        self.nodes.borrow_mut().push(handle.clone());
        handle
    }

    fn create_comment(&self, _text: StrTendril) -> Self::Handle {
        let handle = Rc::new(RefCell::new(Node::default()));
        self.nodes.borrow_mut().push(handle.clone());
        handle
    }

    fn create_pi(&self, _target: StrTendril, _value: StrTendril) -> Self::Handle {
        self.create_comment(StrTendril::new())
    }

    fn append_before_sibling(&self, _sibling: &Self::Handle, _new_node: NodeOrText<Self::Handle>) {}

    fn append_based_on_parent_node(
        &self,
        _element: &Self::Handle,
        _prev_element: &Self::Handle,
        _new_node: NodeOrText<Self::Handle>,
    ) {
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        if let NodeOrText::AppendText(text) = child {
            parent.borrow_mut().text.push_str(&text);
        }
    }

    fn append_doctype_to_document(
        &self,
        _name: StrTendril,
        _public_id: StrTendril,
        _system_id: StrTendril,
    ) {
    }

    fn add_attrs_if_missing(&self, _target: &Self::Handle, _attrs: Vec<Attribute>) {}
    fn remove_from_parent(&self, _target: &Self::Handle) {}
    fn reparent_children(&self, _node: &Self::Handle, _new_parent: &Self::Handle) {}
    fn mark_script_already_started(&self, _node: &Self::Handle) {}
}

fn parse_html(input: &str) -> DomSink {
    parse_document(DomSink::default(), Default::default())
        .from_utf8()
        .read_from(&mut input.as_bytes())
        .expect("HTML parser I/O cannot fail for an in-memory string")
}

fn attr(node: &Node, name: &str) -> Option<String> {
    node.attrs
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn style_property<'a>(style: &'a str, name: &str) -> Option<&'a str> {
    style.split(';').find_map(|part| {
        let (key, value) = part.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

fn dimensions(node: &Node) -> (Option<u32>, Option<u32>, bool) {
    let style = attr(node, "style").unwrap_or_default().to_ascii_lowercase();
    let value = |name: &str| {
        attr(node, name)
            .and_then(|value| value.trim().trim_end_matches("px").parse().ok())
            .or_else(|| {
                style_property(&style, name)
                    .and_then(|value| value.trim_end_matches("px").parse().ok())
            })
    };
    let hidden = style_property(&style, "display").is_some_and(|v| v == "none")
        || style_property(&style, "visibility").is_some_and(|v| v == "hidden")
        || style_property(&style, "opacity").is_some_and(|v| v == "0");
    (value("width"), value("height"), hidden)
}

fn remote_url(url: &str) -> Option<(String, Option<String>, bool)> {
    let parsed = if let Some(rest) = url.strip_prefix("//") {
        Url::parse(&format!("https:{rest}")).ok()?
    } else {
        Url::parse(url).ok()?
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let scheme = if url.starts_with("//") {
        "protocol-relative"
    } else {
        parsed.scheme()
    };
    Some((
        scheme.to_string(),
        parsed.host_str().map(str::to_string),
        parsed.query().is_some(),
    ))
}

fn add_resource(
    output: &mut HtmlRemoteEvidence,
    kind: RemoteResourceKind,
    source: RemoteResourceSource,
    url: &str,
    dimensions: (Option<u32>, Option<u32>, bool),
) {
    if let Some((scheme, host, has_query)) = remote_url(url) {
        let mut signals = Vec::new();
        if dimensions.0 == Some(1) || dimensions.1 == Some(1) {
            signals.push(RemoteResourceSignal::TinyDimensions);
        }
        if dimensions.2 {
            signals.push(RemoteResourceSignal::Hidden);
        }
        if has_query {
            signals.push(RemoteResourceSignal::QueryParameters);
        }
        output.remote_resources.push(RemoteResource {
            kind,
            source,
            url: url.to_string(),
            scheme,
            host,
            declared_width: dimensions.0,
            declared_height: dimensions.1,
            hidden: dimensions.2,
            signals,
        });
    } else if url.starts_with("cid:")
        || url.starts_with("data:")
        || url.starts_with('#')
        || !url.contains(':')
    {
        output.local_references.push(url.to_string());
    }
}

fn urls_in_css(value: &str) -> impl Iterator<Item = &str> {
    value.match_indices("url(").filter_map(|(start, _)| {
        let rest = &value[start + 4..];
        let end = rest.find(')')?;
        Some(rest[..end].trim().trim_matches(['\'', '"']))
    })
}

/// Analyze decoded HTML without fetching or resolving any resource.
pub fn analyze_html_remote_evidence(input: &str) -> HtmlRemoteEvidence {
    let dom = parse_html(input);
    let mut output = HtmlRemoteEvidence {
        remote_resources: Vec::new(),
        local_references: Vec::new(),
        links: Vec::new(),
    };
    for node in dom.elements() {
        let name = node
            .name
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let dimensions = dimensions(&node);
        if name == "img" {
            if let Some(src) = attr(&node, "src") {
                add_resource(
                    &mut output,
                    RemoteResourceKind::Image,
                    RemoteResourceSource::ImgSrc,
                    &src,
                    dimensions,
                );
            }
            if let Some(srcset) = attr(&node, "srcset") {
                for candidate in srcset
                    .split(',')
                    .filter_map(|item| item.split_whitespace().next())
                {
                    add_resource(
                        &mut output,
                        RemoteResourceKind::Image,
                        RemoteResourceSource::ImgSrcset,
                        candidate,
                        dimensions,
                    );
                }
            }
        }
        if let Some(style) = attr(&node, "style") {
            for url in urls_in_css(&style) {
                add_resource(
                    &mut output,
                    RemoteResourceKind::CssBackground,
                    RemoteResourceSource::StyleAttribute,
                    url,
                    dimensions,
                );
            }
        }
        if name == "style" {
            for url in urls_in_css(&node.text) {
                add_resource(
                    &mut output,
                    RemoteResourceKind::CssBackground,
                    RemoteResourceSource::StyleBlock,
                    url,
                    (None, None, false),
                );
            }
        }
        if name == "a" {
            if let Some(href) = attr(&node, "href") {
                if remote_url(&href).is_some() {
                    output.links.push(href);
                }
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn corpus_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../experiments/html-remote-evidence-probe")
    }

    #[test]
    fn committed_html_corpus_matches_its_goldens() {
        let fixtures = corpus_root().join("fixtures");
        let oracles = corpus_root().join("oracles");
        let mut count = 0;
        for entry in fs::read_dir(fixtures).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|value| value.to_str()) != Some("html") {
                continue;
            }
            let name = path.file_stem().unwrap().to_str().unwrap();
            let actual = analyze_html_remote_evidence(&fs::read_to_string(&path).unwrap());
            let expected: HtmlRemoteEvidence = serde_json::from_slice(
                &fs::read(oracles.join(format!("{name}.expected.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(actual, expected, "fixture {name}");
            count += 1;
        }
        assert_eq!(count, 30);
    }

    #[test]
    fn arbitrary_malformed_html_does_not_panic_or_fetch() {
        let alphabet = [
            "<img src=\"https://img.example.invalid/a\">",
            "<style>a{background:url(http://css.example.invalid/x)}</style>",
            "<div><p><img src='cid:x'>",
            "<!-- https://comment.example.invalid/x -->",
            "&lt;img src=https://text.example.invalid/x&gt;",
        ];
        for round in 0..200 {
            let input = (0..(round % 11 + 1))
                .map(|index| alphabet[(round + index) % alphabet.len()])
                .collect::<String>();
            let _ = analyze_html_remote_evidence(&input);
        }
    }

    #[test]
    fn repeated_elements_have_stable_observations_and_owned_names_drop() {
        let input = (0..1_000)
            .map(|_| "<div><img src=\"https://img.example.invalid/pixel\"></div>")
            .collect::<String>();
        for _ in 0..20 {
            let result = analyze_html_remote_evidence(&input);
            assert_eq!(result.remote_resources.len(), 1_000);
        }

        let weak_name = {
            let dom = parse_html("<div><img src=\"https://img.example.invalid/x\"></div>");
            let handle = dom.nodes.borrow()[0].clone();
            std::rc::Rc::downgrade(&handle)
        };
        assert!(weak_name.upgrade().is_none());
    }

    #[test]
    fn html5ever_reconstruction_does_not_duplicate_image_observations() {
        let result = analyze_html_remote_evidence(
            "<table><tr><td><p><b><img src=\"https://img.example.invalid/a\"></table><p><i><img src=\"https://img.example.invalid/b\">",
        );
        assert_eq!(
            result
                .remote_resources
                .iter()
                .map(|resource| resource.url.as_str())
                .collect::<Vec<_>>(),
            [
                "https://img.example.invalid/a",
                "https://img.example.invalid/b"
            ]
        );
    }

    #[test]
    fn links_are_not_remote_resources() {
        let result = analyze_html_remote_evidence(
            r#"<a href="https://click.example.invalid/open?id=1">link</a>"#,
        );
        assert!(result.remote_resources.is_empty());
        assert_eq!(result.links, ["https://click.example.invalid/open?id=1"]);
    }
}
