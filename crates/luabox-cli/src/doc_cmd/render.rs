//! Static-site renderer for the documentation model (SPEC.md §13).
//!
//! Zero-install output: every page is a self-contained HTML file — inline
//! CSS (rustdoc-inspired: sidebar nav, monospace signatures, light/dark via
//! `prefers-color-scheme`), vanilla inline JS, no external assets. The
//! search index is embedded in `index.html` as a JSON `<script>` block and
//! filtered client-side.
//!
//! Cross-links resolve through one global name table (`Links`): any type
//! name appearing in a rendered signature that names a documented
//! class/struct/trait/alias/enum becomes an `<a href>`; unresolved names
//! render plain.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::markdown::{self, escape};
use super::model::{
    self, ClassDoc, DocModel, FunctionDoc, Module, ShapeModule, StructDoc, TraitDoc,
};

/// The global name table: documented type name → href.
pub type Links = BTreeMap<String, String>;

/// Render the whole site: `(file name, contents)` pairs, `index.html` first.
pub fn pages(model: &DocModel) -> Vec<(String, String)> {
    let links = build_links(model);
    let sidebar = sidebar_html(model);
    let classes = model::classes_by_name(model);

    let mut out = vec![("index.html".to_string(), index_page(model, &sidebar))];
    for module in &model.modules {
        out.push((
            module_file(&module.name),
            module_page(module, &links, &sidebar),
        ));
        for class in &module.classes {
            out.push((
                class_file(&class.name),
                class_page(class, &classes, model, &links, &sidebar),
            ));
        }
    }
    for sm in &model.shape_modules {
        out.push((shape_file(&sm.name), shape_page(sm, &links, &sidebar)));
        for st in &sm.structs {
            out.push((
                struct_file(&st.name),
                struct_page(st, model, &links, &sidebar),
            ));
        }
        for tr in &sm.traits {
            out.push((
                trait_file(&tr.name),
                trait_page(tr, model, &links, &sidebar),
            ));
        }
    }
    out
}

// === File naming ==========================================================

/// A filesystem-safe page name fragment.
fn slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn module_file(name: &str) -> String {
    format!("module.{}.html", slug(name))
}
fn shape_file(name: &str) -> String {
    format!("shape.{}.html", slug(name))
}
fn class_file(name: &str) -> String {
    format!("class.{}.html", slug(name))
}
fn struct_file(name: &str) -> String {
    format!("struct.{}.html", slug(name))
}
fn trait_file(name: &str) -> String {
    format!("trait.{}.html", slug(name))
}

// === Link resolution ======================================================

/// Build the global name table from every documented type.
pub fn build_links(model: &DocModel) -> Links {
    let mut links = Links::new();
    for module in &model.modules {
        for class in &module.classes {
            links.insert(class.name.clone(), class_file(&class.name));
        }
        for alias in &module.aliases {
            links.insert(
                alias.name.clone(),
                format!("{}#alias.{}", module_file(&module.name), slug(&alias.name)),
            );
        }
        for en in &module.enums {
            links.insert(
                en.name.clone(),
                format!("{}#enum.{}", module_file(&module.name), slug(&en.name)),
            );
        }
    }
    for sm in &model.shape_modules {
        for st in &sm.structs {
            links.insert(st.name.clone(), struct_file(&st.name));
        }
        for tr in &sm.traits {
            links.insert(tr.name.clone(), trait_file(&tr.name));
        }
        for alias in &sm.aliases {
            links.insert(
                alias.name.clone(),
                format!("{}#alias.{}", shape_file(&sm.name), slug(&alias.name)),
            );
        }
    }
    links
}

/// Escape a rendered type string into HTML, wrapping every identifier that
/// names a documented type in a link. Identifiers inside string-literal
/// types (`"north"`) are never linked; unresolved names render plain.
pub fn link_types(rendered: &str, links: &Links) -> String {
    let mut out = String::with_capacity(rendered.len());
    let bytes = rendered.as_bytes();
    let mut i = 0;
    let mut in_str: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(delim) = in_str {
            out.push_str(&escape(&rendered[i..=i]));
            if b == delim {
                in_str = None;
            }
            i += 1;
        } else if b == b'"' || b == b'\'' {
            in_str = Some(b);
            out.push_str(&escape(&rendered[i..=i]));
            i += 1;
        } else if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
            {
                i += 1;
            }
            // A trailing dot is punctuation, not part of the name.
            let mut end = i;
            while end > start && bytes[end - 1] == b'.' {
                end -= 1;
            }
            let name = &rendered[start..end];
            match links.get(name) {
                Some(href) => {
                    let _ = write!(out, "<a href=\"{}\">{}</a>", escape(href), escape(name));
                }
                None => out.push_str(&escape(name)),
            }
            out.push_str(&escape(&rendered[end..i]));
        } else {
            out.push_str(&escape(&rendered[i..=i]));
            i += 1;
        }
    }
    out
}

// === Search index =========================================================

/// One JSON string literal (escapes `<` too, so the payload can be embedded
/// inside a `<script>` block verbatim).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The first line of a doc block, as the search-result summary.
fn summary(docs: &str) -> String {
    docs.lines().next().unwrap_or("").trim().to_string()
}

/// A function's search summary: its first doc line, else its signature.
fn fn_summary(func: &FunctionDoc) -> String {
    let s = summary(&func.docs);
    if s.is_empty() { func.signature() } else { s }
}

/// The client-side search index: a JSON array of
/// `{"name", "kind", "href", "summary"}` entries covering every documented
/// item.
pub fn search_index_json(model: &DocModel) -> String {
    let mut entries: Vec<(String, &'static str, String, String)> = Vec::new();
    for module in &model.modules {
        entries.push((
            module.name.clone(),
            "module",
            module_file(&module.name),
            summary(&module.docs),
        ));
        for f in &module.functions {
            entries.push((
                f.name.clone(),
                "function",
                format!("{}#fn.{}", module_file(&module.name), slug(&f.name)),
                fn_summary(f),
            ));
        }
        for class in &module.classes {
            entries.push((
                class.name.clone(),
                "class",
                class_file(&class.name),
                summary(&class.docs),
            ));
            for m in &class.methods {
                entries.push((
                    m.name.clone(),
                    "method",
                    format!("{}#fn.{}", class_file(&class.name), slug(&m.name)),
                    fn_summary(m),
                ));
            }
        }
        for alias in &module.aliases {
            entries.push((
                alias.name.clone(),
                "alias",
                format!("{}#alias.{}", module_file(&module.name), slug(&alias.name)),
                summary(&alias.docs),
            ));
        }
        for en in &module.enums {
            entries.push((
                en.name.clone(),
                "enum",
                format!("{}#enum.{}", module_file(&module.name), slug(&en.name)),
                summary(&en.docs),
            ));
        }
    }
    for sm in &model.shape_modules {
        entries.push((
            sm.name.clone(),
            "shape module",
            shape_file(&sm.name),
            String::new(),
        ));
        for st in &sm.structs {
            entries.push((
                st.name.clone(),
                "struct",
                struct_file(&st.name),
                summary(&st.docs),
            ));
        }
        for tr in &sm.traits {
            entries.push((
                tr.name.clone(),
                "trait",
                trait_file(&tr.name),
                summary(&tr.docs),
            ));
        }
        for alias in &sm.aliases {
            entries.push((
                alias.name.clone(),
                "alias",
                format!("{}#alias.{}", shape_file(&sm.name), slug(&alias.name)),
                summary(&alias.docs),
            ));
        }
    }

    let mut json = String::from("[");
    for (i, (name, kind, href, summary)) in entries.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        let _ = write!(
            json,
            "{{\"name\":{},\"kind\":{},\"href\":{},\"summary\":{}}}",
            json_str(name),
            json_str(kind),
            json_str(href),
            json_str(summary)
        );
    }
    json.push(']');
    json
}

// === Page skeleton ========================================================

const CSS: &str = r#"
:root {
  color-scheme: light dark;
  --bg: #ffffff; --fg: #1c1c1c; --muted: #67676c;
  --side-bg: #f5f5f5; --border: #e0e0e0;
  --link: #3873ad; --sig-bg: #f6f7f6; --code-bg: #f0f0f0;
  --badge-bg: #fff3d6; --badge-fg: #8f5902;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #1e1e22; --fg: #dddddd; --muted: #9a9a9f;
    --side-bg: #26262b; --border: #3a3a40;
    --link: #6fb0e8; --sig-bg: #2a2a2f; --code-bg: #333338;
    --badge-bg: #3d3320; --badge-fg: #e8c06f;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0; display: flex; min-height: 100vh;
  background: var(--bg); color: var(--fg);
  font: 15px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", sans-serif;
}
a { color: var(--link); text-decoration: none; }
a:hover { text-decoration: underline; }
nav.sidebar {
  width: 230px; flex-shrink: 0; padding: 1rem 1.2rem;
  background: var(--side-bg); border-right: 1px solid var(--border);
}
nav.sidebar .package { font-weight: 700; font-size: 1.05rem; margin-bottom: .8rem; display: block; }
nav.sidebar h3 {
  font-size: .72rem; letter-spacing: .06em; text-transform: uppercase;
  color: var(--muted); margin: 1.1rem 0 .3rem;
}
nav.sidebar ul { list-style: none; margin: 0; padding: 0; }
nav.sidebar li { margin: .15rem 0; overflow-wrap: anywhere; }
main { flex: 1; min-width: 0; padding: 1.4rem 2.2rem 3rem; max-width: 62rem; }
h1 { font-size: 1.5rem; border-bottom: 1px solid var(--border); padding-bottom: .4rem; }
h2 { font-size: 1.15rem; border-bottom: 1px solid var(--border); padding-bottom: .25rem; margin-top: 2rem; }
h3 { font-size: 1rem; margin-top: 1.4rem; }
pre, code { font: 13px/1.5 ui-monospace, "Cascadia Code", Consolas, "Liberation Mono", monospace; }
code { background: var(--code-bg); padding: .08em .3em; border-radius: 3px; }
pre { background: var(--sig-bg); padding: .6rem .8rem; border-radius: 5px; overflow-x: auto; }
pre code { background: none; padding: 0; }
pre.sig { border-left: 3px solid var(--link); }
.badge {
  display: inline-block; font-size: .72rem; font-weight: 600;
  background: var(--badge-bg); color: var(--badge-fg);
  border-radius: 3px; padding: .05em .45em; vertical-align: middle; margin-left: .5em;
}
.item { margin: 1.3rem 0 1.8rem; }
.muted { color: var(--muted); }
dl.params dt { font-family: ui-monospace, Consolas, monospace; font-size: 13px; margin-top: .4rem; }
dl.params dd { margin: 0 0 .2rem 1.4rem; color: var(--muted); }
table.fields { border-collapse: collapse; width: 100%; }
table.fields td, table.fields th {
  text-align: left; padding: .3rem .6rem; border-bottom: 1px solid var(--border);
  vertical-align: top;
}
table.fields td:first-child { font-family: ui-monospace, Consolas, monospace; font-size: 13px; white-space: nowrap; }
#search {
  width: 100%; padding: .5rem .7rem; font-size: .95rem;
  border: 1px solid var(--border); border-radius: 5px;
  background: var(--bg); color: var(--fg);
}
#search-results { list-style: none; padding: 0; }
#search-results li { padding: .35rem 0; border-bottom: 1px solid var(--border); }
#search-results .kind {
  display: inline-block; width: 7.5em; color: var(--muted); font-size: .78rem;
}
#search-results .summary { margin-left: .8em; color: var(--muted); font-size: .85rem; }
ul.item-list { list-style: none; padding: 0; }
ul.item-list li { padding: .25rem 0; }
ul.item-list .summary { margin-left: .8em; color: var(--muted); font-size: .9rem; }
"#;

const SEARCH_JS: &str = r"
(function () {
  'use strict';
  var raw = document.getElementById('search-index');
  var box = document.getElementById('search');
  var out = document.getElementById('search-results');
  var listing = document.getElementById('listing');
  if (!raw || !box || !out) { return; }
  var data = JSON.parse(raw.textContent);
  box.addEventListener('input', function () {
    var q = box.value.trim().toLowerCase();
    out.innerHTML = '';
    if (!q) { out.hidden = true; listing.hidden = false; return; }
    var hits = data.filter(function (e) {
      return e.name.toLowerCase().indexOf(q) !== -1;
    }).slice(0, 100);
    hits.forEach(function (e) {
      var li = document.createElement('li');
      var kind = document.createElement('span');
      kind.className = 'kind';
      kind.textContent = e.kind;
      var a = document.createElement('a');
      a.href = e.href;
      a.textContent = e.name;
      li.appendChild(kind);
      li.appendChild(a);
      if (e.summary) {
        var s = document.createElement('span');
        s.className = 'summary';
        s.textContent = e.summary;
        li.appendChild(s);
      }
      out.appendChild(li);
    });
    if (hits.length === 0) {
      var none = document.createElement('li');
      none.className = 'muted';
      none.textContent = 'No results.';
      out.appendChild(none);
    }
    out.hidden = false; listing.hidden = true;
  });
})();
";

/// Wrap page content in the shared skeleton (doctype, inline CSS, sidebar).
fn page(title: &str, sidebar: &str, content: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n<style>{CSS}</style>\n</head>\n<body>\n\
         <nav class=\"sidebar\">\n{sidebar}</nav>\n<main>\n{content}</main>\n</body>\n</html>\n",
        escape(title)
    )
}

/// The shared sidebar: package name plus every module and type page.
fn sidebar_html(model: &DocModel) -> String {
    let mut nav = format!(
        "<a class=\"package\" href=\"index.html\">{}</a>\n",
        escape(&model.package)
    );
    let section = |nav: &mut String, title: &str, items: &[(String, String)]| {
        if items.is_empty() {
            return;
        }
        let _ = write!(nav, "<h3>{}</h3>\n<ul>\n", escape(title));
        for (name, href) in items {
            let _ = writeln!(
                nav,
                "<li><a href=\"{}\">{}</a></li>",
                escape(href),
                escape(name)
            );
        }
        nav.push_str("</ul>\n");
    };

    let modules: Vec<(String, String)> = model
        .modules
        .iter()
        .map(|m| (m.name.clone(), module_file(&m.name)))
        .collect();
    let shapes: Vec<(String, String)> = model
        .shape_modules
        .iter()
        .map(|m| (m.name.clone(), shape_file(&m.name)))
        .collect();
    let mut classes: Vec<(String, String)> = Vec::new();
    for module in &model.modules {
        for class in &module.classes {
            classes.push((class.name.clone(), class_file(&class.name)));
        }
    }
    let mut structs: Vec<(String, String)> = Vec::new();
    let mut traits: Vec<(String, String)> = Vec::new();
    for sm in &model.shape_modules {
        for st in &sm.structs {
            structs.push((st.name.clone(), struct_file(&st.name)));
        }
        for tr in &sm.traits {
            traits.push((tr.name.clone(), trait_file(&tr.name)));
        }
    }
    section(&mut nav, "Modules", &modules);
    section(&mut nav, "Shape modules", &shapes);
    section(&mut nav, "Classes", &classes);
    section(&mut nav, "Structs", &structs);
    section(&mut nav, "Traits", &traits);
    nav
}

// === Pages ================================================================

fn index_page(model: &DocModel, sidebar: &str) -> String {
    let mut content = format!("<h1>Package {}</h1>\n", escape(&model.package));
    content.push_str(
        "<p><input id=\"search\" type=\"search\" placeholder=\"Search functions, classes, \
         structs, traits…\" autocomplete=\"off\"></p>\n\
         <ul id=\"search-results\" hidden></ul>\n<div id=\"listing\">\n",
    );

    let listing = |content: &mut String, title: &str, items: Vec<(String, String, String)>| {
        if items.is_empty() {
            return;
        }
        let _ = write!(
            content,
            "<h2>{}</h2>\n<ul class=\"item-list\">\n",
            escape(title)
        );
        for (name, href, summary) in items {
            let _ = write!(
                content,
                "<li><a href=\"{}\">{}</a>",
                escape(&href),
                escape(&name)
            );
            if !summary.is_empty() {
                let _ = write!(
                    content,
                    "<span class=\"summary\">{}</span>",
                    escape(&summary)
                );
            }
            content.push_str("</li>\n");
        }
        content.push_str("</ul>\n");
    };

    listing(
        &mut content,
        "Modules",
        model
            .modules
            .iter()
            .map(|m| (m.name.clone(), module_file(&m.name), summary(&m.docs)))
            .collect(),
    );
    listing(
        &mut content,
        "Shape modules",
        model
            .shape_modules
            .iter()
            .map(|m| (m.name.clone(), shape_file(&m.name), String::new()))
            .collect(),
    );
    let mut types: Vec<(String, String, String)> = Vec::new();
    for module in &model.modules {
        for class in &module.classes {
            types.push((
                class.name.clone(),
                class_file(&class.name),
                summary(&class.docs),
            ));
        }
    }
    for sm in &model.shape_modules {
        for st in &sm.structs {
            types.push((st.name.clone(), struct_file(&st.name), summary(&st.docs)));
        }
        for tr in &sm.traits {
            types.push((tr.name.clone(), trait_file(&tr.name), summary(&tr.docs)));
        }
    }
    listing(&mut content, "Types", types);
    let mut functions: Vec<(String, String, String)> = Vec::new();
    for module in &model.modules {
        for f in &module.functions {
            functions.push((
                f.name.clone(),
                format!("{}#fn.{}", module_file(&module.name), slug(&f.name)),
                summary(&f.docs),
            ));
        }
    }
    listing(&mut content, "Functions", functions);
    content.push_str("</div>\n");

    let _ = write!(
        content,
        "<script type=\"application/json\" id=\"search-index\">{}</script>\n\
         <script>{SEARCH_JS}</script>\n",
        search_index_json(model)
    );
    page(
        &format!("{} — luabox doc", model.package),
        sidebar,
        &content,
    )
}

/// One function/method entry: anchored signature, docs, parameter and
/// return lists.
fn function_html(func: &FunctionDoc, links: &Links) -> String {
    let mut html = format!("<div class=\"item\" id=\"fn.{}\">\n", slug(&func.name));
    let params: Vec<String> = func
        .params
        .iter()
        .map(|p| match &p.ty {
            Some(ty) => format!("{}: {}", escape(&p.name), link_types(ty, links)),
            None => escape(&p.name),
        })
        .collect();
    let mut sig = format!("function {}({})", escape(&func.name), params.join(", "));
    if !func.returns.is_empty() {
        let rets: Vec<String> = func
            .returns
            .iter()
            .map(|r| link_types(&r.ty, links))
            .collect();
        sig.push_str(": ");
        sig.push_str(&rets.join(", "));
    }
    let _ = writeln!(html, "<pre class=\"sig\">{sig}</pre>");
    if func.deprecated {
        html.push_str("<p><span class=\"badge\">deprecated</span></p>\n");
    }
    html.push_str(&markdown::to_html(&func.docs));

    let documented_params: Vec<&_> = func
        .params
        .iter()
        .filter(|p| p.ty.is_some() || p.desc.is_some())
        .collect();
    if !documented_params.is_empty() {
        html.push_str("<h3>Parameters</h3>\n<dl class=\"params\">\n");
        for p in documented_params {
            let q = if p.optional { "?" } else { "" };
            let ty =
                p.ty.as_ref()
                    .map(|t| format!(": {}", link_types(t, links)))
                    .unwrap_or_default();
            let _ = writeln!(html, "<dt>{}{q}{ty}</dt>", escape(&p.name));
            let _ = writeln!(html, "<dd>{}</dd>", escape(p.desc.as_deref().unwrap_or("")));
        }
        html.push_str("</dl>\n");
    }
    if !func.returns.is_empty() {
        html.push_str("<h3>Returns</h3>\n<dl class=\"params\">\n");
        for r in &func.returns {
            let name = r
                .name
                .as_ref()
                .map(|n| format!("{} ", escape(n)))
                .unwrap_or_default();
            let _ = writeln!(html, "<dt>{name}{}</dt>", link_types(&r.ty, links));
            let _ = writeln!(html, "<dd>{}</dd>", escape(r.desc.as_deref().unwrap_or("")));
        }
        html.push_str("</dl>\n");
    }
    html.push_str("</div>\n");
    html
}

fn module_page(module: &Module, links: &Links, sidebar: &str) -> String {
    let mut content = format!("<h1>Module <code>{}</code></h1>\n", escape(&module.name));
    content.push_str(&markdown::to_html(&module.docs));

    if !module.classes.is_empty() {
        content.push_str("<h2>Classes</h2>\n<ul class=\"item-list\">\n");
        for class in &module.classes {
            let _ = write!(
                content,
                "<li><a href=\"{}\">{}</a>",
                escape(&class_file(&class.name)),
                escape(&class.name)
            );
            let s = summary(&class.docs);
            if !s.is_empty() {
                let _ = write!(content, "<span class=\"summary\">{}</span>", escape(&s));
            }
            content.push_str("</li>\n");
        }
        content.push_str("</ul>\n");
    }

    if !module.functions.is_empty() {
        content.push_str("<h2>Functions</h2>\n");
        for func in &module.functions {
            content.push_str(&function_html(func, links));
        }
    }

    if !module.aliases.is_empty() {
        content.push_str("<h2>Aliases</h2>\n");
        for alias in &module.aliases {
            let _ = writeln!(
                content,
                "<div class=\"item\" id=\"alias.{}\">",
                slug(&alias.name)
            );
            let mut sig = format!("alias {}", escape(&alias.name));
            if let Some(ty) = &alias.ty {
                let _ = write!(sig, " = {}", link_types(ty, links));
            }
            let _ = writeln!(content, "<pre class=\"sig\">{sig}</pre>");
            content.push_str(&markdown::to_html(&alias.docs));
            if !alias.members.is_empty() {
                content.push_str("<dl class=\"params\">\n");
                for (ty, desc) in &alias.members {
                    let _ = writeln!(content, "<dt>{}</dt>", link_types(ty, links));
                    let _ = writeln!(
                        content,
                        "<dd>{}</dd>",
                        escape(desc.as_deref().unwrap_or(""))
                    );
                }
                content.push_str("</dl>\n");
            }
            content.push_str("</div>\n");
        }
    }

    if !module.enums.is_empty() {
        content.push_str("<h2>Enums</h2>\n");
        for en in &module.enums {
            let _ = writeln!(
                content,
                "<div class=\"item\" id=\"enum.{}\">",
                slug(&en.name)
            );
            let key = if en.key { " (key)" } else { "" };
            let _ = writeln!(
                content,
                "<pre class=\"sig\">enum {}{key}</pre>",
                escape(&en.name)
            );
            content.push_str(&markdown::to_html(&en.docs));
            content.push_str("</div>\n");
        }
    }

    page(&format!("Module {}", module.name), sidebar, &content)
}

/// The field table shared by class and struct pages.
fn field_table(rows: &[(String, String, String)], links: &Links) -> String {
    let mut html =
        String::from("<table class=\"fields\">\n<tr><th>Field</th><th>Type</th><th></th></tr>\n");
    for (name, ty, desc) in rows {
        let _ = writeln!(
            html,
            "<tr><td>{}</td><td>{}</td><td class=\"muted\">{}</td></tr>",
            escape(name),
            link_types(ty, links),
            escape(desc)
        );
    }
    html.push_str("</table>\n");
    html
}

fn class_page(
    class: &ClassDoc,
    classes: &BTreeMap<&str, &ClassDoc>,
    model: &DocModel,
    links: &Links,
    sidebar: &str,
) -> String {
    let mut content = format!("<h1>Class <code>{}</code>", escape(&class.name));
    if class.exact {
        content.push_str("<span class=\"badge\">exact</span>");
    }
    content.push_str("</h1>\n");
    if !class.parents.is_empty() {
        let parents: Vec<String> = class.parents.iter().map(|p| link_types(p, links)).collect();
        let _ = writeln!(content, "<p>extends {}</p>", parents.join(", "));
    }
    content.push_str(&markdown::to_html(&class.docs));

    if !class.fields.is_empty() {
        content.push_str("<h2>Fields</h2>\n");
        let rows: Vec<(String, String, String)> = class
            .fields
            .iter()
            .map(|f| {
                let scope = f
                    .scope
                    .as_ref()
                    .map(|s| format!("{s} "))
                    .unwrap_or_default();
                let q = if f.optional { "?" } else { "" };
                (
                    format!("{scope}{}{q}", f.name),
                    f.ty.clone(),
                    f.desc.clone().unwrap_or_default(),
                )
            })
            .collect();
        content.push_str(&field_table(&rows, links));
    }

    for (parent, fields) in model::inherited_fields(classes, class) {
        if fields.is_empty() {
            continue;
        }
        let _ = writeln!(
            content,
            "<h3>Fields inherited from {}</h3>",
            link_types(&parent, links)
        );
        let rows: Vec<(String, String, String)> = fields
            .iter()
            .map(|f| {
                let q = if f.optional { "?" } else { "" };
                (
                    format!("{}{q}", f.name),
                    f.ty.clone(),
                    f.desc.clone().unwrap_or_default(),
                )
            })
            .collect();
        content.push_str(&field_table(&rows, links));
    }

    if !class.methods.is_empty() {
        content.push_str("<h2>Methods</h2>\n");
        for method in &class.methods {
            content.push_str(&function_html(method, links));
        }
    }

    let implemented = traits_of(model, &class.name);
    if !implemented.is_empty() {
        content.push_str("<h2>Trait implementations</h2>\n<ul class=\"item-list\">\n");
        for tr in implemented {
            let _ = writeln!(content, "<li>{}</li>", link_types(&tr, links));
        }
        content.push_str("</ul>\n");
    }

    page(&format!("Class {}", class.name), sidebar, &content)
}

fn shape_page(sm: &ShapeModule, links: &Links, sidebar: &str) -> String {
    let mut content = format!(
        "<h1>Shape module <code>{}</code></h1>\n<p class=\"muted\">Declared in <code>{}.lb</code> \
         — analyser-only shape declarations (SHAPES.md).</p>\n",
        escape(&sm.name),
        escape(&sm.name)
    );
    if !sm.structs.is_empty() {
        content.push_str("<h2>Structs</h2>\n<ul class=\"item-list\">\n");
        for st in &sm.structs {
            let _ = write!(
                content,
                "<li><a href=\"{}\">{}</a>",
                escape(&struct_file(&st.name)),
                escape(&st.name)
            );
            let s = summary(&st.docs);
            if !s.is_empty() {
                let _ = write!(content, "<span class=\"summary\">{}</span>", escape(&s));
            }
            content.push_str("</li>\n");
        }
        content.push_str("</ul>\n");
    }
    if !sm.traits.is_empty() {
        content.push_str("<h2>Traits</h2>\n<ul class=\"item-list\">\n");
        for tr in &sm.traits {
            let _ = write!(
                content,
                "<li><a href=\"{}\">{}</a>",
                escape(&trait_file(&tr.name)),
                escape(&tr.name)
            );
            let s = summary(&tr.docs);
            if !s.is_empty() {
                let _ = write!(content, "<span class=\"summary\">{}</span>", escape(&s));
            }
            content.push_str("</li>\n");
        }
        content.push_str("</ul>\n");
    }
    if !sm.aliases.is_empty() {
        content.push_str("<h2>Type aliases</h2>\n");
        for alias in &sm.aliases {
            let _ = writeln!(
                content,
                "<div class=\"item\" id=\"alias.{}\">",
                slug(&alias.name)
            );
            let generics = if alias.generics.is_empty() {
                String::new()
            } else {
                format!("&lt;{}&gt;", escape(&alias.generics.join(", ")))
            };
            let _ = writeln!(
                content,
                "<pre class=\"sig\">type {}{generics} = {}</pre>",
                escape(&alias.name),
                link_types(&alias.ty, links)
            );
            content.push_str(&markdown::to_html(&alias.docs));
            content.push_str("</div>\n");
        }
    }
    if !sm.impls.is_empty() {
        content.push_str("<h2>Impls</h2>\n<ul class=\"item-list\">\n");
        for imp in &sm.impls {
            let _ = writeln!(
                content,
                "<li><code>impl {} for {}</code></li>",
                link_types(&imp.trait_name, links),
                link_types(&imp.struct_name, links)
            );
        }
        content.push_str("</ul>\n");
    }
    page(&format!("Shape module {}", sm.name), sidebar, &content)
}

fn generics_html(generics: &[String]) -> String {
    if generics.is_empty() {
        String::new()
    } else {
        format!("&lt;{}&gt;", escape(&generics.join(", ")))
    }
}

fn struct_page(st: &StructDoc, model: &DocModel, links: &Links, sidebar: &str) -> String {
    let seal = if st.sealed { "sealed" } else { "open" };
    let mut content = format!(
        "<h1>Struct <code>{}{}</code><span class=\"badge\">{seal}</span></h1>\n",
        escape(&st.name),
        generics_html(&st.generics)
    );
    if st.sealed {
        content.push_str(
            "<p class=\"muted\">Sealed: tables bound to this struct may not carry \
             undeclared fields (SHAPES.md §5).</p>\n",
        );
    } else {
        content.push_str(
            "<p class=\"muted\">Open (<code>..</code>): tables bound to this struct may \
             carry extra fields.</p>\n",
        );
    }
    content.push_str(&markdown::to_html(&st.docs));

    if !st.fields.is_empty() {
        content.push_str("<h2>Fields</h2>\n");
        let rows: Vec<(String, String, String)> = st
            .fields
            .iter()
            .map(|f| (f.name.clone(), f.ty.clone(), summary(&f.docs)))
            .collect();
        content.push_str(&field_table(&rows, links));
    }

    let implemented = traits_of(model, &st.name);
    if !implemented.is_empty() {
        content.push_str("<h2>Trait implementations</h2>\n<ul class=\"item-list\">\n");
        for tr in implemented {
            let _ = writeln!(content, "<li>{}</li>", link_types(&tr, links));
        }
        content.push_str("</ul>\n");
    }

    page(&format!("Struct {}", st.name), sidebar, &content)
}

fn trait_page(tr: &TraitDoc, model: &DocModel, links: &Links, sidebar: &str) -> String {
    let mut content = format!(
        "<h1>Trait <code>{}{}</code></h1>\n",
        escape(&tr.name),
        generics_html(&tr.generics)
    );
    if !tr.supertraits.is_empty() {
        let supers: Vec<String> = tr
            .supertraits
            .iter()
            .map(|s| link_types(s, links))
            .collect();
        let _ = writeln!(content, "<p>requires {}</p>", supers.join(" + "));
    }
    content.push_str(&markdown::to_html(&tr.docs));

    if !tr.fns.is_empty() {
        content.push_str("<h2>Required functions</h2>\n");
        for func in &tr.fns {
            let _ = writeln!(
                content,
                "<div class=\"item\" id=\"fn.{}\">",
                slug(&func.name)
            );
            let _ = writeln!(
                content,
                "<pre class=\"sig\">{}</pre>",
                link_types(&func.sig, links)
            );
            content.push_str(&markdown::to_html(&func.docs));
            content.push_str("</div>\n");
        }
    }

    let implementors = implementors_of(model, &tr.name);
    if !implementors.is_empty() {
        content.push_str("<h2>Implementors</h2>\n<ul class=\"item-list\">\n");
        for name in implementors {
            let _ = writeln!(content, "<li>{}</li>", link_types(&name, links));
        }
        content.push_str("</ul>\n");
    }

    page(&format!("Trait {}", tr.name), sidebar, &content)
}

/// The traits `struct_name` implements, per the project-wide impl set.
fn traits_of(model: &DocModel, struct_name: &str) -> Vec<String> {
    let mut out: Vec<String> = model
        .impls
        .iter()
        .filter(|i| i.struct_name == struct_name)
        .map(|i| i.trait_name.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The structs (or classes) implementing `trait_name`.
fn implementors_of(model: &DocModel, trait_name: &str) -> Vec<String> {
    let mut out: Vec<String> = model
        .impls
        .iter()
        .filter(|i| i.trait_name == trait_name)
        .map(|i| i.struct_name.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use luabox_syntax::Dialect;

    fn fixture_model() -> DocModel {
        let (module, mut impls) = model::lua_module(
            "main",
            "--- Entry module.\n\
             \n\
             --- Distance from origin.\n\
             ---@param p Point the point\n\
             ---@return number\n\
             local function dist(p)\n  return 0\nend\n",
            Dialect::Lua54,
        );
        let shapes = model::shape_module(
            "geometry",
            "/// A 2D point.\nstruct Point { x: number, y: number }\n\
             trait Shape {\n    fn area(self) -> number;\n}\n\
             impl Shape for Point;\n",
        );
        impls.extend(shapes.impls.clone());
        DocModel {
            package: "fixture".to_string(),
            modules: vec![module],
            shape_modules: vec![shapes],
            impls,
        }
    }

    #[test]
    fn links_cover_classes_structs_traits_and_aliases() {
        let model = fixture_model();
        let links = build_links(&model);
        assert_eq!(
            links.get("Point").map(String::as_str),
            Some("struct.Point.html")
        );
        assert_eq!(
            links.get("Shape").map(String::as_str),
            Some("trait.Shape.html")
        );
        assert!(!links.contains_key("number"));
    }

    #[test]
    fn link_types_wraps_known_names_only() {
        let mut links = Links::new();
        links.insert("Point".to_string(), "struct.Point.html".to_string());
        let html = link_types("fun(p: Point): number", &links);
        assert_eq!(
            html,
            "fun(p: <a href=\"struct.Point.html\">Point</a>): number"
        );
    }

    #[test]
    fn link_types_skips_string_literals_and_escapes() {
        let mut links = Links::new();
        links.insert("Point".to_string(), "struct.Point.html".to_string());
        assert_eq!(
            link_types("\"Point\"|Point", &links).matches("<a ").count(),
            1
        );
        assert_eq!(link_types("table<string>", &links), "table&lt;string&gt;");
    }

    #[test]
    fn search_index_is_valid_json_with_expected_entries() {
        let model = fixture_model();
        let json = search_index_json(&model);
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("search index must be valid JSON");
        let entries = parsed.as_array().expect("array");
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().expect("name"))
            .collect();
        assert!(names.contains(&"dist"));
        assert!(names.contains(&"Point"));
        assert!(names.contains(&"Shape"));
        assert!(names.contains(&"main"));
        for entry in entries {
            assert!(entry["kind"].is_string());
            assert!(entry["href"].is_string());
            assert!(entry["summary"].is_string());
        }
        // Safe to embed in a <script> block: no raw `<`.
        assert!(!json.contains('<'));
    }

    #[test]
    fn pages_cross_link_param_types_to_struct_pages() {
        let model = fixture_model();
        let pages = pages(&model);
        let module = &pages
            .iter()
            .find(|(name, _)| name == "module.main.html")
            .expect("module page")
            .1;
        assert!(module.contains("href=\"struct.Point.html\""));
        let index = &pages
            .iter()
            .find(|(name, _)| name == "index.html")
            .expect("index page")
            .1;
        assert!(index.contains("search-index"));
    }

    #[test]
    fn struct_page_carries_seal_marker_and_impls() {
        let model = fixture_model();
        let pages = pages(&model);
        let st = &pages
            .iter()
            .find(|(name, _)| name == "struct.Point.html")
            .expect("struct page")
            .1;
        assert!(st.contains("sealed"));
        assert!(st.contains("href=\"trait.Shape.html\""));
        let tr = &pages
            .iter()
            .find(|(name, _)| name == "trait.Shape.html")
            .expect("trait page")
            .1;
        assert!(
            tr.contains("fn area(self) -&gt; number") || tr.contains("fn area(self) -> number")
        );
        assert!(tr.contains("href=\"struct.Point.html\""));
    }
}
