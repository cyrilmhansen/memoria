use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::{parse_document, Attribute, ExpandedName, QualName};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use url::Url;

#[derive(Clone, Debug, Default)]
struct Node {
    name: Option<String>,
    qual_name: Option<&'static QualName>,
    attrs: Vec<(String, String)>,
    parent: Option<usize>,
    text: String,
}

#[derive(Default)]
struct DomSink {
    nodes: RefCell<Vec<Node>>,
}

impl DomSink {
    fn elements(&self) -> Vec<(usize, Node)> {
        self.nodes
            .borrow()
            .iter()
            .enumerate()
            .filter_map(|(index, node)| node.name.as_ref().map(|_| (index, node.clone())))
            .collect()
    }
}

impl TreeSink for DomSink {
    type Handle = usize;
    type Output = Self;
    type ElemName<'a> = ExpandedName<'a>;

    fn finish(self) -> Self {
        self
    }
    fn get_document(&self) -> usize {
        0
    }
    fn get_template_contents(&self, target: &usize) -> usize {
        *target
    }
    fn same_node(&self, x: &usize, y: &usize) -> bool {
        x == y
    }
    fn elem_name(&self, target: &usize) -> ExpandedName<'_> {
        let nodes = self.nodes.borrow();
        nodes[*target].qual_name.expect("element name").expanded()
    }
    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, _flags: ElementFlags) -> usize {
        let handle = self.nodes.borrow().len();
        let name = Box::leak(Box::new(name));
        self.nodes.borrow_mut().push(Node {
            name: Some(name.local.to_string()),
            qual_name: Some(name),
            attrs: attrs
                .into_iter()
                .map(|attr| (attr.name.local.to_string(), attr.value.to_string()))
                .collect(),
            ..Node::default()
        });
        handle
    }
    fn create_comment(&self, _text: StrTendril) -> usize {
        let handle = self.nodes.borrow().len();
        self.nodes.borrow_mut().push(Node::default());
        handle
    }
    fn create_pi(&self, _target: StrTendril, _value: StrTendril) -> usize {
        self.create_comment(StrTendril::new())
    }
    fn append_before_sibling(&self, _sibling: &usize, _new_node: NodeOrText<usize>) {}
    fn append_based_on_parent_node(
        &self,
        _element: &usize,
        _prev_element: &usize,
        _new_node: NodeOrText<usize>,
    ) {
    }
    fn parse_error(&self, _msg: Cow<'static, str>) {}
    fn set_quirks_mode(&self, _mode: QuirksMode) {}
    fn append(&self, parent: &usize, child: NodeOrText<usize>) {
        match child {
            NodeOrText::AppendNode(child) => {
                self.nodes.borrow_mut()[child].parent = Some(*parent);
            }
            NodeOrText::AppendText(text) => self.nodes.borrow_mut()[*parent].text.push_str(&text),
        }
    }
    fn append_doctype_to_document(
        &self,
        _name: StrTendril,
        _public_id: StrTendril,
        _system_id: StrTendril,
    ) {
    }
    fn add_attrs_if_missing(&self, _target: &usize, _attrs: Vec<Attribute>) {}
    fn remove_from_parent(&self, _target: &usize) {}
    fn reparent_children(&self, _node: &usize, _new_parent: &usize) {}
    fn mark_script_already_started(&self, _node: &usize) {}
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Resource {
    kind: String,
    source: String,
    url: String,
    scheme: String,
    host: Option<String>,
    declared_width: Option<u32>,
    declared_height: Option<u32>,
    hidden: bool,
    signals: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Observation {
    remote_resources: Vec<Resource>,
    local_references: Vec<String>,
    links: Vec<String>,
}

fn parse_html(input: &str) -> DomSink {
    parse_document(DomSink::default(), Default::default())
        .from_utf8()
        .read_from(&mut input.as_bytes())
        .expect("HTML parsing failed")
}

fn attr(node: &Node, name: &str) -> Option<String> {
    node.attrs
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
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

fn style_property<'a>(style: &'a str, name: &str) -> Option<&'a str> {
    style.split(';').find_map(|part| {
        let (key, value) = part.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

fn remote(url: &str) -> Option<(String, Option<String>, String)> {
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
    }
    .to_string();
    Some((
        scheme,
        parsed.host_str().map(str::to_string),
        parsed
            .query()
            .map(|_| "query-parameters".to_string())
            .unwrap_or_default(),
    ))
}

fn add_resource(
    out: &mut Observation,
    kind: &str,
    source: &str,
    url: &str,
    dims: (Option<u32>, Option<u32>, bool),
) {
    if let Some((scheme, host, query_signal)) = remote(url) {
        let mut signals = Vec::new();
        if dims.0 == Some(1) || dims.1 == Some(1) {
            signals.push("tiny-dimensions".into());
        }
        if dims.2 {
            signals.push("hidden".into());
        }
        if !query_signal.is_empty() {
            signals.push(query_signal);
        }
        out.remote_resources.push(Resource {
            kind: kind.into(),
            source: source.into(),
            url: url.into(),
            scheme,
            host,
            declared_width: dims.0,
            declared_height: dims.1,
            hidden: dims.2,
            signals,
        });
    } else if url.starts_with("cid:")
        || url.starts_with("data:")
        || url.starts_with('#')
        || !url.contains(':')
    {
        out.local_references.push(url.into());
    }
}

fn urls_in_css(value: &str) -> impl Iterator<Item = &str> {
    value.match_indices("url(").filter_map(|(start, _)| {
        let rest = &value[start + 4..];
        let end = rest.find(')')?;
        Some(rest[..end].trim().trim_matches(['\'', '"']))
    })
}

fn observe(input: &str) -> Observation {
    let dom = parse_html(input);
    let mut out = Observation {
        remote_resources: Vec::new(),
        local_references: Vec::new(),
        links: Vec::new(),
    };
    for (index, node) in dom.elements() {
        let name = node
            .name
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let dims = dimensions(&node);
        if name == "img" {
            if let Some(src) = attr(&node, "src") {
                add_resource(&mut out, "image", "img-src", &src, dims);
            }
            if let Some(srcset) = attr(&node, "srcset") {
                for candidate in srcset
                    .split(',')
                    .filter_map(|item| item.split_whitespace().next())
                {
                    add_resource(&mut out, "image", "img-srcset", candidate, dims);
                }
            }
        }
        if let Some(style) = attr(&node, "style") {
            for url in urls_in_css(&style) {
                add_resource(&mut out, "css-background", "style-attribute", url, dims);
            }
        }
        if name == "style" {
            for url in urls_in_css(&node.text) {
                add_resource(
                    &mut out,
                    "css-background",
                    "style-block",
                    url,
                    (None, None, false),
                );
            }
        }
        if name == "a" {
            if let Some(href) = attr(&node, "href") {
                if remote(&href).is_some() {
                    out.links.push(href);
                }
            }
        }
        let _ = index;
    }
    out
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.get(1).map(String::as_str) == Some("--audit-sanitizer") {
        let input = r#"<div onclick="bad()"><script>bad()</script><form action="https://evil.example.invalid"><img src="https://img.example.invalid/pixel"></form><a href="https://click.example.invalid">link</a></div>"#;
        let clean = ammonia::Builder::default().clean(input).to_string();
        println!(
            "remote_img_preserved={}",
            clean.contains("https://img.example.invalid/pixel")
        );
        println!("script_preserved={}", clean.contains("<script"));
        println!("form_preserved={}", clean.contains("<form"));
        println!("event_handler_preserved={}", clean.contains("onclick"));
        println!(
            "external_link_preserved={}",
            clean.contains("https://click.example.invalid")
        );
        return;
    }
    let root = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "fixtures".into()));
    let oracle_root = root.parent().unwrap_or(Path::new(".")).join("oracles");
    let mut count = 0;
    for entry in fs::read_dir(&root).expect("fixture directory") {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(|v| v.to_str()) != Some("html") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap();
        let actual = observe(&fs::read_to_string(&path).expect("fixture"));
        let expected: Observation = serde_json::from_slice(
            &fs::read(oracle_root.join(format!("{name}.expected.json"))).expect("oracle"),
        )
        .expect("oracle JSON");
        assert_eq!(actual, expected, "{name}");
        count += 1;
    }
    println!("checked={count} network_fetches=0");
}
