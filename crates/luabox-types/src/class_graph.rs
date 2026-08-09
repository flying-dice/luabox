//! The declared `---@class` inheritance graph, and the cycles in it —
//! `LB0318`'s **declaration-driven** half, owned here so both surfaces that
//! report it run the same algorithm over the same graph shape.
//!
//! # Why this lives in `luabox-types` and not in the CLI
//!
//! Round 11 review R11-1 made `LB0318` declaration-driven: a
//! `---@class Widget : Widget` with zero uses anywhere in the project is a
//! cycle lua-language-server reports from the `---@class` node alone, and a
//! resolution-driven check (this crate's [`crate::TypeEnv::note_cyclic`],
//! filed from `DiamondGuard`'s back-edge) can never see it, because nothing
//! resolves a class no file references. That fix shipped inside
//! `luabox-cli`'s `check_cmd`, where the LSP — a sibling crate, not a
//! consumer of the CLI — could not reach it. The result was the editor/CLI
//! split this repo treats as blocking, inverted: `luabox check` red, the
//! editor green on the very fixture the feature targets (round 12 review
//! R12-1).
//!
//! So the algorithm, the singleton-self-edge filter, the choice of which
//! declaration sites report, and the message text all live here — one
//! owner, two callers (`check_cmd::class_ancestry_precheck` and
//! `luabox_lsp::diagnostics`), each supplying its own file table, severity
//! ladder and `---@diagnostic` suppression. A parity claim about "the
//! declaration alone" is only as true as the surfaces that run the check.
//!
//! # Union, not first-declaration-wins
//!
//! Two `---@class Widget` declarations of one name **union** their parents —
//! `docs/03-reference/02-limitations.md`'s documented semantics, and what
//! luals does. The graph this builds unions them too (round 12 review
//! R12-2): the previous first-wins harvest dropped the back-edge of a
//! reopened class outright, so
//!
//! ```lua
//! ---@class Widget
//! ---@class Widget : Widget
//! ```
//!
//! harvested `Widget -> []` and stayed silent while luals reported it.
//!
//! # Which declaration reports
//!
//! Every declaration whose **own** parent list names a member of its own
//! strongly connected component — a *cycle edge*. Measured against the
//! pinned lua-language-server 3.13.5 (2026-08-09, `--checklevel=Warning`),
//! which is exactly what it does:
//!
//! | fixture | luals `circle-doc-class` |
//! |---|---|
//! | `---@class R` then `---@class R : R`, one file | **1**, on the second declaration only |
//! | the same pair split across two files | **1**, on the file carrying `: R` |
//! | `---@class R : R` in **each** of two files | **2**, one per declaration |
//! | `---@class A : B` / `---@class B : A` | **2**, one per declaration |
//! | `---@class A : Innocent` + `---@class A : B` + `---@class B : A` | **2** — the innocent declaration of `A` is not reported |
//! | `---@class A : Plain, B` + `---@class B : A` | **2** — one per declaration, not one per cycle-closing parent |
//! | `---@class Sub : A` behind a cyclic `A`/`B` | **2** — the innocent subclass is not reported |
//! | a DAG/diamond with no cycle | **0** |
//!
//! [`ClassGraph::cycle_sites`] answers with every declaration site of every
//! cycle member — the innocent ones included, flagged
//! [`ClassCycleSite::reports`]` == false` — because a caller needs the
//! non-reporting sites too: they are declarations of a class the caller has
//! *accounted for*, and the resolver-side rediscovery of the same cycle must
//! be deduped against all of them, not only the ones that printed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::BuildHasher;
use std::ops::Range;

/// The label text on an `LB0318`'s primary span, shared by both callers so
/// the two surfaces cannot drift into describing one finding two ways.
pub const CYCLIC_CLASS_LABEL: &str = "`---@class` reachable from its own parent list";

/// One `---@class` declaration of one name: where it is, and the parents
/// **that declaration** lists.
///
/// Per declaration, not per name: a name may be declared many times, the
/// graph unions their parents, and the sites that report are the ones whose
/// own list closes the loop (see the module doc).
#[derive(Debug, Clone)]
struct Declaration {
    file: usize,
    span: Range<usize>,
    parents: Vec<String>,
}

/// Every `---@class` a project declares, keyed by name, each name carrying
/// every declaration of it in harvest order.
///
/// `BTreeMap`, not `HashMap`: [`Self::cycle_sites`]'s output order must be a
/// function of the project and nothing else, and hash iteration order is not.
#[derive(Debug, Default, Clone)]
pub struct ClassGraph {
    decls: BTreeMap<String, Vec<Declaration>>,
}

/// One declaration site of one class that belongs to a cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassCycleSite {
    /// The declared class name.
    pub name: String,
    /// The caller's index for the declaring file — whatever
    /// [`ClassGraph::declare`] was handed.
    pub file: usize,
    /// The `---@class` tag span of this declaration, in the declaring file.
    pub span: Range<usize>,
    /// The other members of this site's cycle, sorted — what
    /// [`cyclic_class_message`] names so the finding is actionable when the
    /// loop closes three files away from the one being read.
    pub others: Vec<String>,
    /// Whether this declaration is one to report: `true` when its own parent
    /// list names a member of its own cycle. A second, innocent declaration
    /// of a cyclic class is `false` — luals does not report it (module doc),
    /// but a caller still owes it a `covered` entry.
    pub reports: bool,
}

impl ClassGraph {
    /// Record one `---@class` declaration. `parents` is the names it lists
    /// by bare name; a parent expressed as anything else (a union, a table
    /// literal, a generic argument) is not an edge this graph follows and is
    /// the caller's to filter out.
    ///
    /// An empty `name` is not a declaration and is dropped — every harvest
    /// site in this crate applies the same `!name.is_empty()` guard, and
    /// folding every bare `---@class` typo in a project into one synthetic
    /// `""` node made unrelated files collide (production readiness review,
    /// finding 4).
    pub fn declare(&mut self, name: &str, file: usize, span: Range<usize>, parents: Vec<String>) {
        if name.is_empty() {
            return;
        }
        self.decls
            .entry(name.to_owned())
            .or_default()
            .push(Declaration {
                file,
                span,
                parents,
            });
    }

    /// Name -> the **union** of every declaration's named parents, first
    /// mention first, deduplicated.
    ///
    /// The shape [`class_cycles`] walks, and the same shape the CLI's
    /// ancestry-depth half relaxes over — one harvest feeds both.
    #[must_use]
    pub fn parents(&self) -> HashMap<String, Vec<String>> {
        let mut out: HashMap<String, Vec<String>> = HashMap::with_capacity(self.decls.len());
        for (name, decls) in &self.decls {
            let mut parents: Vec<String> = Vec::new();
            for parent in decls.iter().flat_map(|decl| &decl.parents) {
                if !parents.contains(parent) {
                    parents.push(parent.clone());
                }
            }
            out.insert(name.clone(), parents);
        }
        out
    }

    /// Every declaration site of every class in a cycle, in `(file, span
    /// start, name)` order so a batch of diagnostics is reproducible.
    ///
    /// Empty when the declared hierarchy is acyclic — a DAG, a diamond, a
    /// chain — however deep or wide it is.
    #[must_use]
    pub fn cycle_sites(&self) -> Vec<ClassCycleSite> {
        let parents = self.parents();
        let mut sites: Vec<ClassCycleSite> = Vec::new();
        for group in class_cycles(&parents) {
            let members: HashSet<&str> = group.iter().map(String::as_str).collect();
            for name in &group {
                let others: Vec<String> = group
                    .iter()
                    .filter(|member| *member != name)
                    .cloned()
                    .collect();
                for decl in self.decls.get(name).into_iter().flatten() {
                    sites.push(ClassCycleSite {
                        name: name.clone(),
                        file: decl.file,
                        span: decl.span.clone(),
                        others: others.clone(),
                        // This declaration's OWN parents, not the union: an
                        // innocent second declaration of a cyclic class is
                        // not a site luals reports (module doc).
                        reports: decl
                            .parents
                            .iter()
                            .any(|parent| members.contains(parent.as_str())),
                    });
                }
            }
        }
        sites.sort_by(|a, b| (a.file, a.span.start, &a.name).cmp(&(b.file, b.span.start, &b.name)));
        sites
    }
}

/// The message an `LB0318` carries for one cycle member, shared by both
/// callers.
///
/// `others` is the rest of the member's cycle, sorted; empty for the
/// degenerate `---@class A : A`, which reads differently because there is no
/// third party to name.
#[must_use]
pub fn cyclic_class_message(name: &str, others: &[String]) -> String {
    let tail = if others.is_empty() {
        ": it lists itself as its own `---@class` parent".to_owned()
    } else {
        let named: Vec<String> = others.iter().map(|other| format!("`{other}`")).collect();
        format!(": the cycle runs back to it through {}", named.join(", "))
    };
    format!(
        "`{name}`'s `---@class` ancestry is cyclic{tail} — break the cycle: a class cannot extend \
         itself, directly or through its ancestors, and members past the back-edge are not in the \
         resolved shape"
    )
}

/// One node's state in [`class_cycles`]' walk: its DFS index, the lowest
/// index reachable from its subtree, and whether it is still on the
/// component stack.
///
/// One struct, one lookup — three parallel maps keyed by the same `&str`
/// were three chances to update two of them (round 12 review, clean-code
/// note).
struct NodeState {
    index: usize,
    low: usize,
    on_stack: bool,
}

/// One node's frame on [`class_cycles`]' explicit DFS stack: which class,
/// and how many of its parents have been visited so far.
#[derive(Clone, Copy)]
struct Frame<'a> {
    name: &'a str,
    next_parent: usize,
}

/// Enter an unvisited node: index it, put it on the component stack, and
/// push its frame.
///
/// Written once and called from both the root and the interior arm of
/// [`class_cycles`] — the two used to be hand-copied, which is a drift risk
/// against an invariant (`index` == `low` == fresh, `on_stack` set) that has
/// to hold identically in both (round 12 review, clean-code note).
fn enter<'a>(
    name: &'a str,
    nodes: &mut HashMap<&'a str, NodeState>,
    next_index: &mut usize,
    component: &mut Vec<&'a str>,
    frames: &mut Vec<Frame<'a>>,
) {
    nodes.insert(
        name,
        NodeState {
            index: *next_index,
            low: *next_index,
            on_stack: true,
        },
    );
    *next_index += 1;
    component.push(name);
    frames.push(Frame {
        name,
        next_parent: 0,
    });
}

/// Every set of `---@class` names that is reachable from itself along
/// declared parent edges — the strongly connected components of `parents_of`
/// that contain a cycle, each returned sorted, the whole list sorted, so the
/// answer does not depend on `HashMap` iteration order.
///
/// Tarjan's algorithm, driven by an **explicit stack** rather than native
/// recursion: this walk runs over input a user controls the depth of, and a
/// pre-check that overflows the stack on a pathological hierarchy is worse
/// than no pre-check at all.
///
/// A single-node component counts only when the class names **itself** as
/// its own parent (`---@class A : A`); every other single node is an
/// ordinary, acyclic class, and reporting those would flag every DAG in
/// every project. The self-edge is recorded by the edge walk that already
/// visits it rather than re-scanned from the parent list afterwards.
#[must_use]
pub fn class_cycles<S: BuildHasher>(
    parents_of: &HashMap<String, Vec<String>, S>,
) -> Vec<Vec<String>> {
    let mut nodes: HashMap<&str, NodeState> = HashMap::new();
    let mut component: Vec<&str> = Vec::new();
    let mut self_edged: HashSet<&str> = HashSet::new();
    let mut next_index = 0;
    let mut groups: Vec<Vec<String>> = Vec::new();

    let mut roots: Vec<&str> = parents_of.keys().map(String::as_str).collect();
    roots.sort_unstable();
    for root in roots {
        if nodes.contains_key(root) {
            continue;
        }
        let mut frames = Vec::new();
        enter(
            root,
            &mut nodes,
            &mut next_index,
            &mut component,
            &mut frames,
        );
        while let Some(top) = frames.len().checked_sub(1) {
            let Some(&Frame { name, next_parent }) = frames.get(top) else {
                break;
            };
            let parents = parents_of.get(name).map_or(&[][..], Vec::as_slice);
            if let Some(parent) = parents.get(next_parent).map(String::as_str) {
                if let Some(frame) = frames.get_mut(top) {
                    frame.next_parent += 1;
                }
                if parent == name {
                    // The degenerate `---@class A : A`. Seen here, on the
                    // edge the walk is already looking at, so the component
                    // filter below need not re-scan the parent list.
                    self_edged.insert(name);
                }
                match nodes.get(parent) {
                    Some(state) => {
                        // A back- or cross-edge: only a node still on the
                        // component stack can lower this one's link.
                        let (parent_index, parent_on_stack) = (state.index, state.on_stack);
                        if parent_on_stack && let Some(node) = nodes.get_mut(name) {
                            node.low = node.low.min(parent_index);
                        }
                    }
                    None => enter(
                        parent,
                        &mut nodes,
                        &mut next_index,
                        &mut component,
                        &mut frames,
                    ),
                }
                continue;
            }
            // Every parent visited: this node is finished. If it is the root
            // of a component, everything above it on the component stack is
            // that component.
            let name_low = nodes.get(name).map_or(0, |node| node.low);
            let is_component_root = nodes.get(name).is_some_and(|node| node.low == node.index);
            if is_component_root {
                let mut group: Vec<&str> = Vec::new();
                while let Some(member) = component.pop() {
                    if let Some(node) = nodes.get_mut(member) {
                        node.on_stack = false;
                    }
                    group.push(member);
                    if member == name {
                        break;
                    }
                }
                if group.len() > 1 || self_edged.contains(name) {
                    group.sort_unstable();
                    groups.push(group.into_iter().map(str::to_owned).collect());
                }
            }
            frames.pop();
            if let Some(&caller) = frames.last()
                && let Some(node) = nodes.get_mut(caller.name)
            {
                node.low = node.low.min(name_low);
            }
        }
    }
    groups.sort();
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A graph literal: `[(name, [parents])]`.
    fn graph(edges: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        edges
            .iter()
            .map(|(name, parents)| {
                (
                    (*name).to_owned(),
                    parents.iter().map(|p| (*p).to_owned()).collect(),
                )
            })
            .collect()
    }

    // === the SCC shapes (round 12 review R12-3) ==========================
    //
    // The algorithm used to be pinned only on 1- and 2-node components, so a
    // lowlink or filter regression had nothing to fail against. These are
    // the shapes that catch one. Each was RED-proven against the specific
    // mutation named in its comment.

    /// The over-report guard, and the one every project depends on: a
    /// hierarchy with no cycle at all yields NOTHING. A diamond gives every
    /// node a second path to the same ancestor, which is what a lowlink
    /// mistake turns into a bogus component.
    ///
    /// RED-proven by dropping the `group.len() > 1 || self_edged` filter:
    /// every one of these five singleton components is then reported as a
    /// cycle and this test fails (it is the only one that does).
    #[test]
    fn an_acyclic_hierarchy_reports_no_cycle_at_all() {
        let dag = graph(&[
            ("Top", &[]),
            ("Left", &["Top"]),
            ("Right", &["Top"]),
            ("Bottom", &["Left", "Right"]),
            ("Loner", &[]),
        ]);
        assert!(
            class_cycles(&dag).is_empty(),
            "a diamond is not a cycle: {:?}",
            class_cycles(&dag)
        );
        // The same shape as one long chain — depth, not width.
        let chain = graph(&[("A", &[]), ("B", &["A"]), ("C", &["B"]), ("D", &["C"])]);
        assert!(class_cycles(&chain).is_empty());
    }

    /// Three nodes, one loop: all three are their own ancestors and all
    /// three are reported. A two-node pin cannot tell a correct component
    /// pop from one that stops at the first back-edge.
    #[test]
    fn a_three_node_cycle_names_all_three_members() {
        let found = class_cycles(&graph(&[("A", &["B"]), ("B", &["C"]), ("C", &["A"])]));
        assert_eq!(
            found,
            vec![vec!["A".to_owned(), "B".to_owned(), "C".to_owned()]]
        );
    }

    /// Two independent cycles in one project are two components, both
    /// reported — a walk that stopped at the first one, or merged them
    /// because the second was reached from the first's root, fails here.
    #[test]
    fn disjoint_cycles_are_each_reported() {
        let found = class_cycles(&graph(&[
            ("A", &["B"]),
            ("B", &["A"]),
            ("Y", &["Z"]),
            ("Z", &["Y"]),
        ]));
        assert_eq!(
            found,
            vec![
                vec!["A".to_owned(), "B".to_owned()],
                vec!["Y".to_owned(), "Z".to_owned()]
            ]
        );
    }

    /// An innocent subclass of a cyclic class is not itself cyclic: it can
    /// reach the cycle, but the cycle cannot reach back, so it is its own
    /// singleton component. The shape that separates "reachable from a
    /// cycle" from "in a cycle" — a walk keyed on reachability alone reports
    /// `Sub` too.
    #[test]
    fn a_cycle_behind_an_innocent_subclass_reports_only_the_cycle() {
        let found = class_cycles(&graph(&[
            ("Ring", &["Loop"]),
            ("Loop", &["Ring"]),
            ("Sub", &["Ring"]),
            ("SubSub", &["Sub"]),
        ]));
        assert_eq!(found, vec![vec!["Loop".to_owned(), "Ring".to_owned()]]);
    }

    /// Two cycles sharing a node are one component, not two — every member
    /// of both loops is reachable from every other.
    #[test]
    fn cycles_sharing_a_node_are_one_component() {
        let found = class_cycles(&graph(&[
            ("Hub", &["A", "X"]),
            ("A", &["Hub"]),
            ("X", &["Hub"]),
        ]));
        assert_eq!(
            found,
            vec![vec!["A".to_owned(), "Hub".to_owned(), "X".to_owned()]]
        );
    }

    /// The degenerate shape: a singleton component counts only with a
    /// literal self-edge. Both halves in one test, because the filter is one
    /// condition — `A : A` fires, a bare `A` never does.
    #[test]
    fn a_self_parent_is_a_cycle_and_a_bare_class_is_not() {
        assert_eq!(
            class_cycles(&graph(&[("A", &["A"])])),
            vec![vec!["A".to_owned()]]
        );
        assert!(class_cycles(&graph(&[("A", &[])])).is_empty());
    }

    /// A parent nothing in the project declares is a node with no outgoing
    /// edges, not a panic and not a cycle.
    #[test]
    fn an_undeclared_parent_is_not_a_cycle() {
        assert!(class_cycles(&graph(&[("A", &["Ambient"])])).is_empty());
    }

    /// Determinism: the answer is sorted at both levels, so it cannot depend
    /// on which key the hash map happened to hand back first.
    #[test]
    fn the_answer_is_sorted_at_both_levels() {
        let found = class_cycles(&graph(&[
            ("Zed", &["Alpha"]),
            ("Alpha", &["Zed"]),
            ("Mid", &["Nod"]),
            ("Nod", &["Mid"]),
        ]));
        let mut sorted = found.clone();
        sorted.sort();
        assert_eq!(found, sorted);
        for group in &found {
            let mut members = group.clone();
            members.sort();
            assert_eq!(group, &members);
        }
    }

    /// Deep enough to overflow a recursive walk: the explicit stack is the
    /// reason this check can run over input a user controls the depth of.
    #[test]
    fn a_very_deep_chain_ending_in_a_cycle_does_not_overflow() {
        let mut edges: Vec<(String, Vec<String>)> = Vec::new();
        for i in 0..50_000_u32 {
            edges.push((format!("C{i}"), vec![format!("C{}", i + 1)]));
        }
        // The last link closes back onto the first: one component of 50,001.
        edges.push(("C50000".to_owned(), vec!["C0".to_owned()]));
        let found = class_cycles(&edges.into_iter().collect::<HashMap<_, _>>());
        assert_eq!(found.len(), 1);
        assert_eq!(found.first().map(Vec::len), Some(50_001));
    }

    // === declaration attribution (round 12 review R12-2) =================

    fn declared(graph: &mut ClassGraph, name: &str, file: usize, start: usize, parents: &[&str]) {
        graph.declare(
            name,
            file,
            start..start + 1,
            parents.iter().map(|p| (*p).to_owned()).collect(),
        );
    }

    /// The R12-2 shape: a class declared once without parents and reopened
    /// with a back-edge onto itself. The union is what makes it a cycle at
    /// all, and only the reopening declaration reports — measured against
    /// lua-language-server 3.13.5, which reports exactly one
    /// `circle-doc-class` here, on the second declaration.
    #[test]
    fn a_reopened_class_closes_the_cycle_and_only_the_reopening_site_reports() {
        let mut graph = ClassGraph::default();
        declared(&mut graph, "Widget", 0, 0, &[]);
        declared(&mut graph, "Widget", 0, 10, &["Widget"]);

        let parents = graph.parents();
        assert_eq!(
            parents.get("Widget"),
            Some(&vec!["Widget".to_owned()]),
            "the union of both declarations, not the first one's empty list"
        );

        let sites = graph.cycle_sites();
        assert_eq!(
            sites.len(),
            2,
            "both declarations are accounted for: {sites:?}"
        );
        assert_eq!(
            sites.iter().map(|s| s.reports).collect::<Vec<_>>(),
            vec![false, true],
            "only the declaration carrying the back-edge reports"
        );
        assert!(sites.iter().all(|site| site.others.is_empty()));
    }

    /// The same cyclic class declared in two files draws two reports, one
    /// per declaration — measured: luals reports `circle-doc-class` in each
    /// file. One report per cycle would under-count against the oracle.
    #[test]
    fn a_cyclic_class_declared_in_two_files_reports_at_both() {
        let mut graph = ClassGraph::default();
        declared(&mut graph, "Ring", 0, 0, &["Ring"]);
        declared(&mut graph, "Ring", 1, 0, &["Ring"]);

        let sites = graph.cycle_sites();
        assert_eq!(sites.len(), 2);
        assert!(sites.iter().all(|site| site.reports));
        assert_eq!(sites.iter().map(|s| s.file).collect::<Vec<_>>(), vec![0, 1]);
        // The union is a SET of edges: two declarations naming the same
        // parent are one edge, not an edge repeated. A duplicated edge would
        // make the SCC walk visit it twice for no reason and put the same
        // name in a message twice.
        assert_eq!(graph.parents().get("Ring"), Some(&vec!["Ring".to_owned()]));
    }

    /// A second, innocent declaration of a cyclic class is accounted for but
    /// not reported — measured: luals reports the cyclic declaration only.
    #[test]
    fn an_innocent_declaration_of_a_cyclic_class_is_covered_but_not_reported() {
        let mut graph = ClassGraph::default();
        declared(&mut graph, "A", 0, 0, &["Plain"]);
        declared(&mut graph, "A", 0, 10, &["B"]);
        declared(&mut graph, "B", 0, 20, &["A"]);
        declared(&mut graph, "Plain", 0, 30, &[]);

        let sites = graph.cycle_sites();
        assert_eq!(
            sites
                .iter()
                .map(|site| (site.name.as_str(), site.span.start, site.reports))
                .collect::<Vec<_>>(),
            vec![("A", 0, false), ("A", 10, true), ("B", 20, true)],
            "the innocent `A : Plain` is covered, not reported; `Plain` is not a member"
        );
    }

    /// A declaration listing several parents, one of which closes the cycle,
    /// is ONE report — not one per parent. Measured against luals, which
    /// reports `---@class MultiA : Plain, MultiB` once.
    #[test]
    fn a_multi_parent_declaration_that_closes_a_cycle_reports_once() {
        let mut graph = ClassGraph::default();
        declared(&mut graph, "MultiA", 0, 0, &["Plain", "MultiB"]);
        declared(&mut graph, "MultiB", 0, 10, &["MultiA"]);
        declared(&mut graph, "Plain", 0, 20, &[]);

        let sites = graph.cycle_sites();
        assert_eq!(sites.iter().filter(|site| site.reports).count(), 2);
        assert_eq!(
            sites.iter().filter(|s| s.name == "MultiA").count(),
            1,
            "one site per declaration, not per cycle-closing parent"
        );
    }

    /// Sites come back in `(file, span, name)` order, so a caller publishing
    /// as it walks produces the same batch every run.
    #[test]
    fn cycle_sites_are_ordered_by_declaration_position() {
        let mut graph = ClassGraph::default();
        declared(&mut graph, "B", 1, 5, &["A"]);
        declared(&mut graph, "A", 0, 9, &["B"]);
        declared(&mut graph, "A", 0, 3, &["B"]);

        let sites = graph.cycle_sites();
        assert_eq!(
            sites
                .iter()
                .map(|site| (site.file, site.span.start))
                .collect::<Vec<_>>(),
            vec![(0, 3), (0, 9), (1, 5)]
        );
        assert_eq!(
            sites.first().map(|site| site.others.clone()),
            Some(vec!["B".to_owned()])
        );
    }

    /// An acyclic graph has no sites at all — the caller-facing half of the
    /// over-report guard above.
    #[test]
    fn an_acyclic_graph_has_no_cycle_sites() {
        let mut graph = ClassGraph::default();
        declared(&mut graph, "Base", 0, 0, &[]);
        declared(&mut graph, "Mid", 0, 10, &["Base"]);
        declared(&mut graph, "Leaf", 0, 20, &["Mid", "Base"]);
        assert!(graph.cycle_sites().is_empty());
    }

    /// An empty name is not a declaration: folding every bare `---@class`
    /// typo in a project into one synthetic node made unrelated files
    /// collide (production readiness review, finding 4).
    #[test]
    fn an_empty_class_name_is_not_declared() {
        let mut graph = ClassGraph::default();
        declared(&mut graph, "", 0, 0, &[""]);
        assert!(graph.parents().is_empty());
        assert!(graph.cycle_sites().is_empty());
    }

    // === the message =====================================================

    #[test]
    fn the_message_names_the_other_members_or_says_it_is_its_own_parent() {
        let alone = cyclic_class_message("Ring", &[]);
        assert!(alone.contains("`Ring`"), "{alone}");
        assert!(alone.contains("lists itself as its own"), "{alone}");

        let pair = cyclic_class_message("A", &["B".to_owned(), "C".to_owned()]);
        assert!(pair.contains("runs back to it through `B`, `C`"), "{pair}");
    }
}
