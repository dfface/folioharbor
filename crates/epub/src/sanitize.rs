use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

use html5ever::{
    Attribute, ExpandedName, ParseOpts, QualName, parse_document,
    tendril::{StrTendril, TendrilSink},
    tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink},
};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use crate::EpubPath;

pub trait ResourceResolver {
    /// Returns the trusted EPUB document path used as the base for relative references.
    fn base(&self) -> &EpubPath;

    /// Resolves a validated, canonical, fragment-free EPUB path to an opaque identifier.
    fn resolve(&self, reference: &EpubPath) -> Option<String>;
}

#[derive(Debug, Eq, PartialEq)]
pub struct SanitizedContent {
    pub html: String,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct SanitizerLimits {
    /// Maximum UTF-8 input size checked before HTML parsing.
    pub max_input_bytes: usize,
    /// Maximum parsed DOM depth, including HTML parser-inserted elements.
    pub max_dom_depth: usize,
    /// Maximum number of parsed DOM nodes.
    pub max_nodes: usize,
    /// Maximum serialized output size after escaping and URL rewriting.
    pub max_output_bytes: usize,
    /// Maximum bytes retained in any CSS input or sanitized CSS intermediate.
    pub max_css_bytes: usize,
    /// Maximum wall-clock duration for parsing and bounded traversal.
    pub deadline: Duration,
}

impl Default for SanitizerLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 8 * 1024 * 1024,
            max_dom_depth: 256,
            max_nodes: 100_000,
            max_output_bytes: 16 * 1024 * 1024,
            max_css_bytes: 2 * 1024 * 1024,
            deadline: Duration::from_secs(2),
        }
    }
}

pub struct ContentSanitizer;

impl Default for ContentSanitizer {
    fn default() -> Self {
        Self
    }
}

impl ContentSanitizer {
    /// Removes active content and rewrites local references to opaque resource identifiers.
    ///
    /// Limit or deadline failures are fail-closed: `html` is empty and `warnings` contains a
    /// stable reason. Partial serialized content is never returned.
    #[must_use]
    pub fn transform(html: &str, resolver: &impl ResourceResolver) -> SanitizedContent {
        Self::transform_with_limits(html, resolver, SanitizerLimits::default())
    }

    #[must_use]
    pub fn transform_with_limits(
        html: &str,
        resolver: &impl ResourceResolver,
        limits: SanitizerLimits,
    ) -> SanitizedContent {
        let started = Instant::now();
        Self::transform_with_budget(html, resolver, limits, WallClockBudget { started, limits })
    }

    fn transform_with_budget(
        html: &str,
        resolver: &impl ResourceResolver,
        limits: SanitizerLimits,
        mut budget: impl DeadlineBudget,
    ) -> SanitizedContent {
        if html.len() > limits.max_input_bytes {
            return failed(SanitizeFailure::Input);
        }
        if let Err(reason) = budget.check(BudgetPoint::Parse) {
            return failed(reason);
        }
        let dom = match parse_incrementally(html, limits, &mut budget) {
            Ok(dom) => dom,
            Err(reason) => return failed(reason),
        };
        let result = validate_dom(&dom.document, limits, &mut budget)
            .and_then(|()| serialize_bounded(&dom.document, resolver, limits, &mut budget));
        let dismantle_result = dismantle_dom(&dom.document, &mut budget);
        let result = result.and_then(|content| {
            dismantle_result?;
            budget.check(BudgetPoint::Final)?;
            Ok(content)
        });
        match result {
            Ok(content) => content,
            Err(reason) => failed(reason),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BudgetPoint {
    Parse,
    Node,
    Attribute,
    Escape,
    Css,
    Output,
    Dismantle,
    Final,
}

trait DeadlineBudget {
    fn check(&mut self, point: BudgetPoint) -> Result<(), SanitizeFailure>;
}

struct WallClockBudget {
    started: Instant,
    limits: SanitizerLimits,
}

impl DeadlineBudget for WallClockBudget {
    fn check(&mut self, _point: BudgetPoint) -> Result<(), SanitizeFailure> {
        if self.started.elapsed() >= self.limits.deadline {
            Err(SanitizeFailure::Deadline)
        } else {
            Ok(())
        }
    }
}

struct BoundedDomSink {
    dom: RcDom,
    limits: SanitizerLimits,
    started: Instant,
    nodes: Cell<usize>,
    depths: RefCell<HashMap<usize, usize>>,
    failure: Cell<Option<SanitizeFailure>>,
}

struct ParsedDom {
    dom: RcDom,
    failure: Option<SanitizeFailure>,
}

impl BoundedDomSink {
    fn new(limits: SanitizerLimits) -> Self {
        let dom = RcDom::default();
        let mut depths = HashMap::new();
        depths.insert(handle_key(&dom.document), 0);
        Self {
            dom,
            limits,
            started: Instant::now(),
            nodes: Cell::new(1),
            depths: RefCell::new(depths),
            failure: Cell::new(None),
        }
    }

    fn count_node(&self) {
        if !self.check_callback_deadline() {
            return;
        }
        let nodes = self.nodes.get().saturating_add(1);
        self.nodes.set(nodes);
        if nodes > self.limits.max_nodes {
            self.record_failure(SanitizeFailure::Nodes);
        }
    }

    fn parent_child_depth(&self, parent: &Handle) -> usize {
        self.depths
            .borrow()
            .get(&handle_key(parent))
            .copied()
            .unwrap_or(0)
            .saturating_add(1)
    }

    fn prepare_text_attachment(&self, parent: &Handle) {
        if self.parent_child_depth(parent) > self.limits.max_dom_depth {
            self.record_failure(SanitizeFailure::Depth);
        }
    }

    fn rebase_subtrees<'a>(
        &self,
        roots: impl IntoIterator<Item = &'a Handle>,
        root_depth: usize,
    ) -> bool {
        if !self.accepting() {
            return false;
        }
        let mut work = roots
            .into_iter()
            .map(|root| (root.clone(), root_depth))
            .collect::<Vec<_>>();
        let mut updates = Vec::new();
        while let Some((node, depth)) = work.pop() {
            if !self.check_callback_deadline() {
                return false;
            }
            if updates.len() >= self.limits.max_nodes {
                self.record_failure(SanitizeFailure::Nodes);
                return false;
            }
            if depth > self.limits.max_dom_depth {
                self.record_failure(SanitizeFailure::Depth);
                return false;
            }
            updates.push((handle_key(&node), depth));
            work.extend(
                node.children
                    .borrow()
                    .iter()
                    .map(|child| (child.clone(), depth.saturating_add(1))),
            );
        }
        self.depths.borrow_mut().extend(updates);
        true
    }

    fn prepare_node_attachment(&self, parent: &Handle, child: &Handle) -> bool {
        self.rebase_subtrees([child], self.parent_child_depth(parent))
    }

    fn check_callback_deadline(&self) -> bool {
        if self.started.elapsed() >= self.limits.deadline {
            self.record_failure(SanitizeFailure::Deadline);
            false
        } else {
            true
        }
    }

    fn record_failure(&self, reason: SanitizeFailure) {
        if self.failure.get().is_none() {
            self.failure.set(Some(reason));
        }
    }

    fn accepting(&self) -> bool {
        self.failure.get().is_none()
    }
}

fn handle_key(handle: &Handle) -> usize {
    Rc::as_ptr(handle) as usize
}

impl TreeSink for BoundedDomSink {
    type Handle = Handle;
    type Output = ParsedDom;
    type ElemName<'a>
        = ExpandedName<'a>
    where
        Self: 'a;

    fn finish(self) -> Self::Output {
        ParsedDom {
            dom: self.dom,
            failure: self.failure.get(),
        }
    }

    fn parse_error(&self, msg: Cow<'static, str>) {
        self.dom.parse_error(msg);
    }

    fn get_document(&self) -> Handle {
        self.dom.get_document()
    }

    fn elem_name<'a>(&'a self, target: &'a Handle) -> Self::ElemName<'a> {
        self.dom.elem_name(target)
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        self.count_node();
        if flags.template {
            self.count_node();
        }
        self.dom.create_element(name, attrs, flags)
    }

    fn create_comment(&self, text: StrTendril) -> Handle {
        self.count_node();
        self.dom.create_comment(text)
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Handle {
        self.count_node();
        self.dom.create_pi(target, data)
    }

    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        match &child {
            NodeOrText::AppendNode(handle) => {
                self.prepare_node_attachment(parent, handle);
            }
            NodeOrText::AppendText(_) => {
                self.count_node();
                self.prepare_text_attachment(parent);
            }
        }
        if self.accepting() {
            self.dom.append(parent, child);
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        prev_element: &Handle,
        child: NodeOrText<Handle>,
    ) {
        let parent = element.parent.take();
        let actual_parent = parent
            .as_ref()
            .and_then(std::rc::Weak::upgrade)
            .unwrap_or_else(|| prev_element.clone());
        element.parent.set(parent);
        match &child {
            NodeOrText::AppendNode(handle) => {
                self.prepare_node_attachment(&actual_parent, handle);
            }
            NodeOrText::AppendText(_) => {
                self.count_node();
                self.prepare_text_attachment(&actual_parent);
            }
        }
        if self.accepting() {
            self.dom
                .append_based_on_parent_node(element, prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        self.count_node();
        if self.accepting() {
            self.dom
                .append_doctype_to_document(name, public_id, system_id);
        }
    }

    fn get_template_contents(&self, target: &Handle) -> Handle {
        self.dom.get_template_contents(target)
    }

    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        self.dom.same_node(x, y)
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.dom.set_quirks_mode(mode);
    }

    fn append_before_sibling(&self, sibling: &Handle, child: NodeOrText<Handle>) {
        if let Some(parent) = sibling.parent.take() {
            if let Some(parent_handle) = parent.upgrade() {
                match &child {
                    NodeOrText::AppendNode(handle) => {
                        self.prepare_node_attachment(&parent_handle, handle);
                    }
                    NodeOrText::AppendText(_) => {
                        self.count_node();
                        self.prepare_text_attachment(&parent_handle);
                    }
                }
            }
            sibling.parent.set(Some(parent));
        }
        if self.accepting() {
            self.dom.append_before_sibling(sibling, child);
        }
    }

    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        if self.accepting() {
            self.dom.add_attrs_if_missing(target, attrs);
        }
    }

    fn remove_from_parent(&self, target: &Handle) {
        if self.rebase_subtrees([target], 0) {
            self.dom.remove_from_parent(target);
        }
    }

    fn reparent_children(&self, node: &Handle, new_parent: &Handle) {
        let children = node.children.borrow().clone();
        if self.rebase_subtrees(children.iter(), self.parent_child_depth(new_parent)) {
            self.dom.reparent_children(node, new_parent);
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, target: &Handle) -> bool {
        self.dom.is_mathml_annotation_xml_integration_point(target)
    }

    fn allow_declarative_shadow_roots(&self, intended_parent: &Handle) -> bool {
        self.dom.allow_declarative_shadow_roots(intended_parent)
    }

    fn attach_declarative_shadow(
        &self,
        location: &Handle,
        template: &Handle,
        attrs: &[Attribute],
    ) -> bool {
        self.accepting()
            && self
                .dom
                .attach_declarative_shadow(location, template, attrs)
    }
}

fn parse_incrementally(
    html: &str,
    limits: SanitizerLimits,
    budget: &mut impl DeadlineBudget,
) -> Result<RcDom, SanitizeFailure> {
    const PARSER_CHUNK_BYTES: usize = 1024;

    let mut parser = parse_document(BoundedDomSink::new(limits), ParseOpts::default());
    let mut offset = 0_usize;
    while offset < html.len() {
        let mut end = offset.saturating_add(PARSER_CHUNK_BYTES).min(html.len());
        while !html.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        parser.process(html[offset..end].into());
        offset = end;
        if let Err(reason) = budget.check(BudgetPoint::Parse) {
            let parsed = parser.finish();
            let _ = dismantle_dom(&parsed.dom.document, budget);
            return Err(reason);
        }
        if let Some(reason) = parser.tokenizer.sink.sink.failure.get() {
            let parsed = parser.finish();
            let _ = dismantle_dom(&parsed.dom.document, budget);
            return Err(reason);
        }
    }

    let parsed = parser.finish();
    if let Err(reason) = budget.check(BudgetPoint::Parse) {
        let _ = dismantle_dom(&parsed.dom.document, budget);
        return Err(reason);
    }
    if let Some(reason) = parsed.failure {
        let _ = dismantle_dom(&parsed.dom.document, budget);
        Err(reason)
    } else {
        Ok(parsed.dom)
    }
}

#[derive(Clone, Copy)]
enum SanitizeFailure {
    Input,
    Depth,
    Nodes,
    Output,
    Deadline,
}

fn failed(reason: SanitizeFailure) -> SanitizedContent {
    let reason = match reason {
        SanitizeFailure::Input => "input limit exceeded",
        SanitizeFailure::Depth => "depth limit exceeded",
        SanitizeFailure::Nodes => "nodes limit exceeded",
        SanitizeFailure::Output => "output limit exceeded",
        SanitizeFailure::Deadline => "deadline exceeded",
    };
    SanitizedContent {
        html: String::new(),
        warnings: vec![format!("sanitization failed: {reason}")],
    }
}

enum Work {
    Enter(Handle, usize),
    Exit(String),
}

struct Serialization<'a, R, B> {
    resolver: &'a R,
    work: &'a mut Vec<Work>,
    output: &'a mut BoundedOutput,
    warnings: &'a mut Vec<String>,
    limits: SanitizerLimits,
    budget: &'a mut B,
}

struct BoundedOutput {
    value: String,
    max_bytes: usize,
}

impl BoundedOutput {
    fn new(max_bytes: usize) -> Self {
        Self {
            value: String::new(),
            max_bytes,
        }
    }

    fn push_str(
        &mut self,
        value: &str,
        budget: &mut impl DeadlineBudget,
    ) -> Result<(), SanitizeFailure> {
        if self
            .value
            .len()
            .checked_add(value.len())
            .is_none_or(|length| length > self.max_bytes)
        {
            return Err(SanitizeFailure::Output);
        }
        let mut offset = 0_usize;
        while offset < value.len() {
            let mut end = offset.saturating_add(256).min(value.len());
            while !value.is_char_boundary(end) {
                end = end.saturating_sub(1);
            }
            budget.check(BudgetPoint::Output)?;
            self.value.push_str(&value[offset..end]);
            offset = end;
        }
        Ok(())
    }

    fn push(
        &mut self,
        value: char,
        budget: &mut impl DeadlineBudget,
    ) -> Result<(), SanitizeFailure> {
        let mut encoded = [0_u8; 4];
        self.push_str(value.encode_utf8(&mut encoded), budget)
    }
}

fn validate_dom(
    document: &Handle,
    limits: SanitizerLimits,
    budget: &mut impl DeadlineBudget,
) -> Result<(), SanitizeFailure> {
    let mut nodes = 0_usize;
    let mut work = vec![(document.clone(), 0_usize)];
    while let Some((node, depth)) = work.pop() {
        budget.check(BudgetPoint::Node)?;
        nodes = nodes.saturating_add(1);
        if nodes > limits.max_nodes {
            return Err(SanitizeFailure::Nodes);
        }
        if depth > limits.max_dom_depth {
            return Err(SanitizeFailure::Depth);
        }
        work.extend(
            node.children
                .borrow()
                .iter()
                .map(|child| (child.clone(), depth.saturating_add(1))),
        );
    }
    Ok(())
}

fn serialize_bounded(
    document: &Handle,
    resolver: &impl ResourceResolver,
    limits: SanitizerLimits,
    budget: &mut impl DeadlineBudget,
) -> Result<SanitizedContent, SanitizeFailure> {
    let mut work = document
        .children
        .borrow()
        .iter()
        .rev()
        .map(|child| Work::Enter(child.clone(), 1))
        .collect::<Vec<_>>();
    let mut output = BoundedOutput::new(limits.max_output_bytes);
    let mut warnings = Vec::new();
    let mut nodes = 0_usize;
    while let Some(item) = work.pop() {
        budget.check(BudgetPoint::Node)?;
        match item {
            Work::Exit(tag) => {
                output.push_str("</", budget)?;
                output.push_str(&tag, budget)?;
                output.push('>', budget)?;
            }
            Work::Enter(node, depth) => {
                nodes = nodes.saturating_add(1);
                if nodes > limits.max_nodes {
                    return Err(SanitizeFailure::Nodes);
                }
                if depth > limits.max_dom_depth {
                    return Err(SanitizeFailure::Depth);
                }
                let mut context = Serialization {
                    resolver,
                    work: &mut work,
                    output: &mut output,
                    warnings: &mut warnings,
                    limits,
                    budget,
                };
                serialize_node(&node, depth, &mut context)?;
            }
        }
    }
    Ok(SanitizedContent {
        html: output.value,
        warnings,
    })
}

fn serialize_node<R: ResourceResolver, B: DeadlineBudget>(
    node: &Handle,
    depth: usize,
    context: &mut Serialization<'_, R, B>,
) -> Result<(), SanitizeFailure> {
    match &node.data {
        NodeData::Document => push_children(node, depth, context.work),
        NodeData::Text { contents } => {
            escape_text(contents.borrow().as_ref(), context.output, context.budget)?;
        }
        NodeData::Element { name, attrs, .. } => {
            let tag = name.local.as_ref().to_ascii_lowercase();
            if is_forbidden_element(&tag) {
                context.warnings.push(format!("removed element: {tag}"));
            } else if !is_allowed_element(&tag) {
                push_children(node, depth, context.work);
            } else {
                serialize_element(node, &tag, attrs, depth, context)?;
            }
        }
        NodeData::Doctype { .. }
        | NodeData::Comment { .. }
        | NodeData::ProcessingInstruction { .. } => {}
    }
    Ok(())
}

fn serialize_element<R: ResourceResolver, B: DeadlineBudget>(
    node: &Handle,
    tag: &str,
    attrs: &std::cell::RefCell<Vec<html5ever::Attribute>>,
    depth: usize,
    context: &mut Serialization<'_, R, B>,
) -> Result<(), SanitizeFailure> {
    context.output.push('<', context.budget)?;
    context.output.push_str(tag, context.budget)?;
    for attribute in attrs.borrow().iter() {
        context.budget.check(BudgetPoint::Attribute)?;
        let name = attribute.name.local.as_ref();
        if name.starts_with("on") || !is_allowed_attribute(name) {
            continue;
        }
        let raw = attribute.value.as_ref();
        if matches!(name, "href" | "src" | "poster") {
            let Some(value) = sanitize_url(raw, context.resolver, context.limits, context.budget)?
            else {
                continue;
            };
            write_attribute(name, &value, context.output, context.budget)?;
        } else if name == "style" {
            let css = sanitize_declarations(raw, context.resolver, context.limits, context.budget)?;
            if !css.is_empty() {
                write_attribute(name, &css, context.output, context.budget)?;
            }
        } else {
            write_attribute(name, raw, context.output, context.budget)?;
        }
    }
    context.output.push('>', context.budget)?;
    if tag == "style" {
        let mut css_source = BoundedOutput::new(context.limits.max_css_bytes);
        for child in node.children.borrow().iter() {
            context.budget.check(BudgetPoint::Css)?;
            if let NodeData::Text { contents } = &child.data {
                css_source.push_str(contents.borrow().as_ref(), context.budget)?;
            }
        }
        let css = sanitize_stylesheet(
            &css_source.value,
            context.resolver,
            context.limits,
            context.budget,
        )?;
        context.output.push_str(&css, context.budget)?;
        context.work.push(Work::Exit(tag.to_owned()));
    } else {
        if !is_void_element(tag) {
            context.work.push(Work::Exit(tag.to_owned()));
        }
        push_children(node, depth, context.work);
    }
    Ok(())
}

fn write_attribute(
    name: &str,
    value: &str,
    output: &mut BoundedOutput,
    budget: &mut impl DeadlineBudget,
) -> Result<(), SanitizeFailure> {
    output.push(' ', budget)?;
    output.push_str(name, budget)?;
    output.push_str("=\"", budget)?;
    escape_attribute(value, output, budget)?;
    output.push('"', budget)
}

fn push_children(node: &Handle, depth: usize, work: &mut Vec<Work>) {
    work.extend(
        node.children
            .borrow()
            .iter()
            .rev()
            .map(|child| Work::Enter(child.clone(), depth.saturating_add(1))),
    );
}

fn dismantle_dom(
    document: &Handle,
    budget: &mut impl DeadlineBudget,
) -> Result<(), SanitizeFailure> {
    let mut nodes = vec![document.clone()];
    let mut failure = None;
    while let Some(node) = nodes.pop() {
        if let Err(reason) = budget.check(BudgetPoint::Dismantle) {
            failure.get_or_insert(reason);
        }
        nodes.extend(std::mem::take(&mut *node.children.borrow_mut()));
    }
    failure.map_or(Ok(()), Err)
}

fn sanitize_url(
    raw: &str,
    resolver: &impl ResourceResolver,
    limits: SanitizerLimits,
    budget: &mut impl DeadlineBudget,
) -> Result<Option<String>, SanitizeFailure> {
    check_string_budget(raw, BudgetPoint::Attribute, budget)?;
    budget.check(BudgetPoint::Attribute)?;
    let value = raw.trim();
    if value.starts_with('#') {
        return Ok(safe_fragment(value).then(|| value.to_owned()));
    }
    if value.starts_with("//") || value.contains('\0') || has_scheme(value) {
        return Ok(None);
    }
    let (reference, fragment) = value
        .split_once('#')
        .map_or((value, None), |(reference, fragment)| {
            (reference, Some(fragment))
        });
    let Some(decoded) = decode_safe_reference(reference, limits, budget)? else {
        return Ok(None);
    };
    let Ok(canonical) = EpubPath::resolve_from(resolver.base().as_str(), &decoded) else {
        return Ok(None);
    };
    let Some(opaque) = resolver.resolve(&canonical) else {
        return Ok(None);
    };
    check_string_budget(&opaque, BudgetPoint::Attribute, budget)?;
    if opaque.is_empty()
        || opaque.len() > limits.max_output_bytes
        || !opaque
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Ok(None);
    }
    let mut rewritten = BoundedOutput::new(limits.max_output_bytes);
    rewritten.push_str("resource:", budget)?;
    rewritten.push_str(&opaque, budget)?;
    if let Some(fragment) = fragment.filter(|fragment| safe_fragment(fragment)) {
        rewritten.push('#', budget)?;
        rewritten.push_str(fragment, budget)?;
    }
    Ok(Some(rewritten.value))
}

fn decode_safe_reference(
    reference: &str,
    limits: SanitizerLimits,
    budget: &mut impl DeadlineBudget,
) -> Result<Option<String>, SanitizeFailure> {
    if reference.is_empty() || reference.len() > limits.max_output_bytes {
        return Ok(None);
    }
    let Some(decoded) = percent_decode_once(reference, budget)? else {
        return Ok(None);
    };
    if decoded.as_bytes().windows(3).any(|window| {
        window[0] == b'%' && hex_value(window[1]).is_some() && hex_value(window[2]).is_some()
    }) {
        return Ok(None);
    }
    let unsafe_path = decoded.starts_with('/')
        || decoded.contains(['\\', '\0'])
        || decoded
            .split('/')
            .any(|component| matches!(component, "." | ".."));
    Ok((!unsafe_path).then_some(decoded))
}

fn percent_decode_once(
    value: &str,
    budget: &mut impl DeadlineBudget,
) -> Result<Option<String>, SanitizeFailure> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        budget.check(BudgetPoint::Attribute)?;
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Ok(None);
            }
            let Some(high) = hex_value(bytes[index + 1]) else {
                return Ok(None);
            };
            let Some(low) = hex_value(bytes[index + 2]) else {
                return Ok(None);
            };
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Ok(String::from_utf8(decoded).ok())
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn has_scheme(value: &str) -> bool {
    let prefix = value.split(['/', '#', '?']).next().unwrap_or(value);
    prefix.find(':').is_some_and(|colon| {
        let scheme = &prefix[..colon];
        !scheme.is_empty()
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

fn safe_fragment(fragment: &str) -> bool {
    fragment.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '#' | '_' | '-' | '.' | ':')
    })
}

fn sanitize_stylesheet(
    css: &str,
    resolver: &impl ResourceResolver,
    limits: SanitizerLimits,
    budget: &mut impl DeadlineBudget,
) -> Result<String, SanitizeFailure> {
    check_css_size(css, limits)?;
    check_string_budget(css, BudgetPoint::Css, budget)?;
    let mut output = BoundedOutput::new(limits.max_css_bytes);
    let mut remaining = css;
    while let Some(open) = remaining.find('{') {
        budget.check(BudgetPoint::Css)?;
        let selector = remaining[..open].rsplit(';').next().unwrap_or("").trim();
        let body = &remaining[open + 1..];
        let Some(close) = body.find('}') else {
            break;
        };
        if safe_selector(selector) {
            let declarations = sanitize_declarations(&body[..close], resolver, limits, budget)?;
            if !declarations.is_empty() {
                output.push_str(selector, budget)?;
                output.push('{', budget)?;
                output.push_str(&declarations, budget)?;
                output.push('}', budget)?;
            }
        }
        remaining = &body[close + 1..];
    }
    Ok(output.value)
}

fn sanitize_declarations(
    css: &str,
    resolver: &impl ResourceResolver,
    limits: SanitizerLimits,
    budget: &mut impl DeadlineBudget,
) -> Result<String, SanitizeFailure> {
    check_css_size(css, limits)?;
    check_string_budget(css, BudgetPoint::Css, budget)?;
    let mut output = BoundedOutput::new(limits.max_css_bytes);
    for declaration in css.split(';') {
        budget.check(BudgetPoint::Css)?;
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        let property = property.trim();
        if property.len() > 64 || !is_allowed_css_property(&property.to_ascii_lowercase()) {
            continue;
        }
        let Some(value) = sanitize_css_value(value.trim(), resolver, limits, budget)? else {
            continue;
        };
        if !output.value.is_empty() {
            output.push(';', budget)?;
        }
        output.push_str(&property.to_ascii_lowercase(), budget)?;
        output.push(':', budget)?;
        output.push_str(&value, budget)?;
    }
    Ok(output.value)
}

fn sanitize_css_value(
    value: &str,
    resolver: &impl ResourceResolver,
    limits: SanitizerLimits,
    budget: &mut impl DeadlineBudget,
) -> Result<Option<String>, SanitizeFailure> {
    check_string_budget(value, BudgetPoint::Css, budget)?;
    if value.contains("/*") || value.contains(['\\', '@', '{', '}', '[', ']']) {
        return Ok(None);
    }
    let mut output = BoundedOutput::new(limits.max_css_bytes);
    let mut remaining = value;
    while !remaining.is_empty() {
        budget.check(BudgetPoint::Css)?;
        if remaining.len() >= 4 && remaining[..4].eq_ignore_ascii_case("url(") {
            let Some(close) = remaining[4..].find(')') else {
                return Ok(None);
            };
            let raw = remaining[4..4 + close].trim().trim_matches(['\'', '"']);
            let Some(rewritten) = sanitize_url(raw, resolver, limits, budget)? else {
                return Ok(None);
            };
            output.push_str("url('", budget)?;
            output.push_str(&rewritten, budget)?;
            output.push_str("')", budget)?;
            remaining = &remaining[5 + close..];
        } else {
            let character = remaining.chars().next().unwrap_or('\0');
            if character == '(' || character == ')' {
                return Ok(None);
            }
            output.push(character, budget)?;
            remaining = &remaining[character.len_utf8()..];
        }
    }
    Ok((!output.value.trim().is_empty()).then_some(output.value))
}

fn check_css_size(css: &str, limits: SanitizerLimits) -> Result<(), SanitizeFailure> {
    if css.len() > limits.max_css_bytes {
        Err(SanitizeFailure::Output)
    } else {
        Ok(())
    }
}

fn check_string_budget(
    value: &str,
    point: BudgetPoint,
    budget: &mut impl DeadlineBudget,
) -> Result<(), SanitizeFailure> {
    for _ in value.as_bytes().chunks(256) {
        budget.check(point)?;
    }
    Ok(())
}

fn safe_selector(selector: &str) -> bool {
    let selector = selector.trim();
    !selector.is_empty()
        && !selector.contains(['@', '<', '>'])
        && !selector
            .as_bytes()
            .windows(4)
            .any(|window| window.eq_ignore_ascii_case(b"url("))
}

fn is_forbidden_element(tag: &str) -> bool {
    matches!(
        tag,
        "script"
            | "noscript"
            | "form"
            | "input"
            | "button"
            | "select"
            | "option"
            | "textarea"
            | "iframe"
            | "frame"
            | "frameset"
            | "object"
            | "embed"
            | "applet"
            | "meta"
            | "base"
            | "canvas"
            | "audio"
            | "video"
            | "source"
            | "track"
    )
}

fn is_allowed_element(tag: &str) -> bool {
    matches!(
        tag,
        "html"
            | "head"
            | "body"
            | "title"
            | "style"
            | "link"
            | "main"
            | "section"
            | "article"
            | "aside"
            | "nav"
            | "header"
            | "footer"
            | "div"
            | "span"
            | "p"
            | "br"
            | "hr"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "ol"
            | "ul"
            | "li"
            | "dl"
            | "dt"
            | "dd"
            | "blockquote"
            | "pre"
            | "code"
            | "em"
            | "strong"
            | "b"
            | "i"
            | "u"
            | "s"
            | "small"
            | "sub"
            | "sup"
            | "ruby"
            | "rt"
            | "rp"
            | "a"
            | "img"
            | "figure"
            | "figcaption"
            | "table"
            | "caption"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "th"
            | "td"
            | "colgroup"
            | "col"
    )
}

fn is_allowed_attribute(name: &str) -> bool {
    matches!(
        name,
        "id" | "class"
            | "title"
            | "lang"
            | "dir"
            | "role"
            | "epub:type"
            | "href"
            | "src"
            | "alt"
            | "width"
            | "height"
            | "rel"
            | "colspan"
            | "rowspan"
            | "scope"
            | "style"
    ) || name.starts_with("aria-")
}

fn is_allowed_css_property(property: &str) -> bool {
    matches!(
        property,
        "color"
            | "background-color"
            | "background-image"
            | "font"
            | "font-family"
            | "font-size"
            | "font-style"
            | "font-weight"
            | "line-height"
            | "letter-spacing"
            | "text-align"
            | "text-decoration"
            | "text-indent"
            | "text-transform"
            | "white-space"
            | "word-break"
            | "writing-mode"
            | "direction"
            | "display"
            | "margin"
            | "margin-top"
            | "margin-right"
            | "margin-bottom"
            | "margin-left"
            | "padding"
            | "padding-top"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "border"
            | "border-width"
            | "border-style"
            | "border-color"
            | "width"
            | "height"
            | "max-width"
            | "max-height"
            | "vertical-align"
            | "list-style"
            | "list-style-type"
            | "page-break-before"
            | "page-break-after"
            | "break-before"
            | "break-after"
            | "orphans"
            | "widows"
    )
}

fn is_void_element(tag: &str) -> bool {
    matches!(tag, "br" | "hr" | "img" | "link" | "col")
}

fn escape_text(
    value: &str,
    output: &mut BoundedOutput,
    budget: &mut impl DeadlineBudget,
) -> Result<(), SanitizeFailure> {
    for character in value.chars() {
        budget.check(BudgetPoint::Escape)?;
        match character {
            '&' => output.push_str("&amp;", budget)?,
            '<' => output.push_str("&lt;", budget)?,
            '>' => output.push_str("&gt;", budget)?,
            _ => output.push(character, budget)?,
        }
    }
    Ok(())
}

fn escape_attribute(
    value: &str,
    output: &mut BoundedOutput,
    budget: &mut impl DeadlineBudget,
) -> Result<(), SanitizeFailure> {
    for character in value.chars() {
        budget.check(BudgetPoint::Escape)?;
        match character {
            '&' => output.push_str("&amp;", budget)?,
            '<' => output.push_str("&lt;", budget)?,
            '"' => output.push_str("&quot;", budget)?,
            _ => output.push(character, budget)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use html5ever::{local_name, ns};

    struct EmptyResolver {
        base: EpubPath,
    }

    impl ResourceResolver for EmptyResolver {
        fn base(&self) -> &EpubPath {
            &self.base
        }

        fn resolve(&self, _reference: &EpubPath) -> Option<String> {
            None
        }
    }

    struct NeverExpires;

    impl DeadlineBudget for NeverExpires {
        fn check(&mut self, _point: BudgetPoint) -> Result<(), SanitizeFailure> {
            Ok(())
        }
    }

    struct ExpireAt {
        point: BudgetPoint,
        remaining: usize,
    }

    impl DeadlineBudget for ExpireAt {
        fn check(&mut self, point: BudgetPoint) -> Result<(), SanitizeFailure> {
            if point == self.point {
                self.remaining = self.remaining.saturating_sub(1);
                if self.remaining == 0 {
                    return Err(SanitizeFailure::Deadline);
                }
            }
            Ok(())
        }
    }

    fn empty_resolver() -> EmptyResolver {
        EmptyResolver {
            base: EpubPath::new("EPUB/chapter.xhtml").unwrap_or_else(|_| std::process::abort()),
        }
    }

    fn element(sink: &BoundedDomSink) -> Handle {
        sink.create_element(
            QualName::new(None, ns!(html), local_name!("div")),
            Vec::new(),
            ElementFlags::default(),
        )
    }

    fn sink_with_depth(max_dom_depth: usize) -> BoundedDomSink {
        BoundedDomSink::new(SanitizerLimits {
            max_dom_depth,
            max_nodes: 128,
            deadline: Duration::from_secs(30),
            ..SanitizerLimits::default()
        })
    }

    fn recorded_depth(sink: &BoundedDomSink, node: &Handle) -> Option<usize> {
        sink.depths.borrow().get(&handle_key(node)).copied()
    }

    #[test]
    fn moving_subtree_deeper_rebases_descendants_and_trips_depth_in_callback() {
        let sink = sink_with_depth(4);
        let document = sink.get_document();
        let subtree = element(&sink);
        let child = element(&sink);
        let grandchild = element(&sink);
        sink.append(&document, NodeOrText::AppendNode(subtree.clone()));
        sink.append(&subtree, NodeOrText::AppendNode(child.clone()));
        sink.append(&child, NodeOrText::AppendNode(grandchild));
        let parent = element(&sink);
        let deeper_parent = element(&sink);
        let sibling = element(&sink);
        sink.append(&document, NodeOrText::AppendNode(parent.clone()));
        sink.append(&parent, NodeOrText::AppendNode(deeper_parent.clone()));
        sink.append(&deeper_parent, NodeOrText::AppendNode(sibling.clone()));

        sink.append_before_sibling(&sibling, NodeOrText::AppendNode(subtree));

        assert!(matches!(sink.failure.get(), Some(SanitizeFailure::Depth)));
    }

    #[test]
    fn moving_subtree_shallower_rebases_descendants_without_stale_false_positive() {
        let sink = sink_with_depth(4);
        let document = sink.get_document();
        let parent = element(&sink);
        let deeper_parent = element(&sink);
        let subtree = element(&sink);
        let child = element(&sink);
        let grandchild = element(&sink);
        let sibling = element(&sink);
        sink.append(&document, NodeOrText::AppendNode(parent.clone()));
        sink.append(&parent, NodeOrText::AppendNode(deeper_parent.clone()));
        sink.append(&deeper_parent, NodeOrText::AppendNode(subtree.clone()));
        sink.append(&subtree, NodeOrText::AppendNode(child.clone()));
        sink.append(&document, NodeOrText::AppendNode(sibling.clone()));

        sink.append_before_sibling(&sibling, NodeOrText::AppendNode(subtree.clone()));
        let grandchild_for_depth = grandchild.clone();
        sink.append(&child, NodeOrText::AppendNode(grandchild));

        assert_eq!(recorded_depth(&sink, &subtree), Some(1));
        assert_eq!(recorded_depth(&sink, &child), Some(2));
        assert_eq!(recorded_depth(&sink, &grandchild_for_depth), Some(3));
        assert!(sink.failure.get().is_none());
    }

    #[test]
    fn removing_subtree_rebases_detached_depth_state() {
        let sink = sink_with_depth(3);
        let document = sink.get_document();
        let subtree = element(&sink);
        let child = element(&sink);
        let grandchild = element(&sink);
        sink.append(&document, NodeOrText::AppendNode(subtree.clone()));
        sink.append(&subtree, NodeOrText::AppendNode(child.clone()));
        sink.append(&child, NodeOrText::AppendNode(grandchild.clone()));

        sink.remove_from_parent(&subtree);
        let leaf = element(&sink);
        sink.append(&grandchild, NodeOrText::AppendNode(leaf));

        assert_eq!(recorded_depth(&sink, &subtree), Some(0));
        assert_eq!(recorded_depth(&sink, &child), Some(1));
        assert_eq!(recorded_depth(&sink, &grandchild), Some(2));
        assert!(sink.failure.get().is_none());
    }

    #[test]
    fn reparent_children_checks_every_subtree_and_failure_stays_sticky() {
        let sink = sink_with_depth(4);
        let document = sink.get_document();
        let source = element(&sink);
        let shallow_child = element(&sink);
        let deep_child = element(&sink);
        let descendant = element(&sink);
        let grandchild = element(&sink);
        sink.append(&document, NodeOrText::AppendNode(source.clone()));
        sink.append(&source, NodeOrText::AppendNode(shallow_child));
        sink.append(&source, NodeOrText::AppendNode(deep_child.clone()));
        sink.append(&deep_child, NodeOrText::AppendNode(descendant.clone()));
        sink.append(&descendant, NodeOrText::AppendNode(grandchild));
        let parent = element(&sink);
        let new_parent = element(&sink);
        sink.append(&document, NodeOrText::AppendNode(parent.clone()));
        sink.append(&parent, NodeOrText::AppendNode(new_parent.clone()));

        sink.reparent_children(&source, &new_parent);
        let child_count = document.children.borrow().len();
        sink.append(&document, NodeOrText::AppendNode(element(&sink)));

        assert!(matches!(sink.failure.get(), Some(SanitizeFailure::Depth)));
        assert_eq!(document.children.borrow().len(), child_count);
    }

    #[test]
    fn parse_sink_rejects_single_chunk_fanout_and_depth_during_construction() {
        let fanout = format!("<body>{}</body>", "<i></i>".repeat(32));
        let depth = format!("{}x{}", "<i>".repeat(32), "</i>".repeat(32));

        let fanout_result = parse_incrementally(
            &fanout,
            SanitizerLimits {
                max_nodes: 8,
                ..SanitizerLimits::default()
            },
            &mut NeverExpires,
        );
        let depth_result = parse_incrementally(
            &depth,
            SanitizerLimits {
                max_dom_depth: 8,
                ..SanitizerLimits::default()
            },
            &mut NeverExpires,
        );

        assert!(matches!(fanout_result, Err(SanitizeFailure::Nodes)));
        assert!(matches!(depth_result, Err(SanitizeFailure::Depth)));
    }

    #[test]
    fn fails_closed_when_deadline_expires_between_parser_chunks() {
        let resolver = EmptyResolver {
            base: EpubPath::new("EPUB/chapter.xhtml").unwrap_or_else(|_| std::process::abort()),
        };
        let html = format!("<p>{}</p>", "text".repeat(4_096));
        let output = ContentSanitizer::transform_with_budget(
            &html,
            &resolver,
            SanitizerLimits::default(),
            ExpireAt {
                point: BudgetPoint::Parse,
                remaining: 2,
            },
        );

        assert!(output.html.is_empty());
        assert_eq!(output.warnings, ["sanitization failed: deadline exceeded"]);
    }

    #[test]
    fn deadline_is_enforced_inside_attributes_text_css_dismantle_and_final_check() {
        let cases = [
            (BudgetPoint::Attribute, "<p title=\"value\">safe</p>"),
            (BudgetPoint::Escape, "<p>safe text</p>"),
            (BudgetPoint::Css, "<p style=\"color:red\">safe</p>"),
            (BudgetPoint::Dismantle, "<p>safe</p>"),
            (BudgetPoint::Final, "<p>safe</p>"),
        ];

        for (point, html) in cases {
            let output = ContentSanitizer::transform_with_budget(
                html,
                &empty_resolver(),
                SanitizerLimits::default(),
                ExpireAt {
                    point,
                    remaining: 1,
                },
            );
            assert!(
                output.html.is_empty(),
                "partial output escaped at {point:?}"
            );
            assert_eq!(output.warnings, ["sanitization failed: deadline exceeded"]);
        }
    }
}
