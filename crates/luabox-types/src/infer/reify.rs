//! Reification — snapshotting an inference type ([`ITy`]) as a plain
//! structural [`Ty`].
//!
//! This is the boundary every published surface crosses: the `byte-range → Ty`
//! table the annotation checker consults, the binding types behind inlay
//! hints, and the carriers' accumulated shapes. Shape reification is memoized
//! and cycle-guarded (`__index` chains are folded in, recursion bottoms out at
//! the declared class name).

use std::collections::{BTreeMap, HashSet};

use luabox_hir::BodyId;

use crate::ty::{FieldTy, FunctionTy, ParamTy, TableTy, Ty};

use super::{ITy, Infer};

impl Infer<'_> {
    /// Snapshot an inference type as a plain structural [`Ty`].
    pub(super) fn reify(&mut self, ity: &ITy) -> Ty {
        match ity {
            ITy::Ty(ty) => ty.clone(),
            ITy::Shape(id) => self.reify_shape(*id),
            ITy::Func(body) => Ty::Function(Box::new(self.reify_func(*body))),
            ITy::Union(members) => {
                let members = members.clone();
                Ty::union(members.iter().map(|m| self.reify(m)).collect())
            }
        }
    }

    /// [`Self::reify`] for the **module-export** position (#56).
    ///
    /// A returned `---@class` *carrier* crosses the `require` boundary as
    /// the class it carries — [`Ty::Named`] — rather than as its structural
    /// table, because the class is the workspace-global identity and it is
    /// what luals resolves a `require` of the module to. Only the export
    /// position does this: inside the declaring file the carrier stays
    /// structural, so its own conformance obligations are unchanged.
    /// (Instances need no arm here — an instance shape already reifies as
    /// its declared name at every annotated boundary, per
    /// [`Self::reify_shape`].)
    pub(super) fn reify_export(&mut self, ity: &ITy) -> Ty {
        match ity {
            ITy::Shape(id) => {
                if let Some(name) = self.shapes[*id].declared.clone()
                    && self.env.resolve_named(&name).is_some()
                {
                    return Ty::Named(name);
                }
                self.reify_shape(*id)
            }
            ITy::Union(members) => {
                let members = members.clone();
                Ty::union(members.iter().map(|m| self.reify_export(m)).collect())
            }
            other => self.reify(other),
        }
    }

    fn reify_func(&mut self, body: BodyId) -> FunctionTy {
        if let Some(sig) = self.funcs.get(&body).and_then(|f| f.sig.clone()) {
            return sig;
        }
        let hir_body = self.body(body);
        let params: Vec<ParamTy> = hir_body
            .params
            .iter()
            .map(|&p| ParamTy {
                name: self.binding(p).name.clone(),
                ty: Ty::Unknown,
                optional: false,
            })
            .collect();
        let varargs = hir_body.is_vararg.then_some(Ty::Unknown);
        let (returns_set, returns) = match self.funcs.get(&body) {
            Some(data) => (data.returns_set, data.returns.clone()),
            None => (false, Vec::new()),
        };
        let returns = if returns_set {
            returns.iter().map(|r| self.reify(r)).collect()
        } else {
            Vec::new()
        };
        FunctionTy {
            params,
            varargs,
            returns,
            returns_vararg: false,
            // In display mode the inferred returns are the signature: a
            // dependent file calling this exported function gets them. The
            // checker (seed_params off) keeps the conservative `false`.
            has_return_annotation: returns_set && self.mode.seeds_params(),
            // Explicitly *not* declared: this signature was read off an
            // unannotated body, so its parameters are a description and not a
            // contract. A consumer reaching this function through `require`
            // must not argument-check against it, exactly as a same-file call
            // to an unannotated function is not argument-checked (#46).
            declared: false,
            overloads: Vec::new(),
            generics: Vec::new(),
            // Inferred (unannotated) functions carry no doc-comment flags.
            deprecated: false,
            nodiscard: false,
            is_async: false,
            version: None,
        }
    }

    /// Reify a shape: own fields plus the flattened `__index` chain
    /// (nearest definition wins), skipping `__`-metafields. Cycles cut off
    /// with the catch-all table shape.
    pub(super) fn reify_shape(&mut self, id: usize) -> Ty {
        // A declared *instance* shape reifies as its declared name: the
        // result of `setmetatable(x, Carrier)` (and `self` in the carrier's
        // methods) IS the declared class/struct at annotated boundaries, so
        // constructors satisfy `---@return <Class>` (#73).
        if self.shapes[id].is_instance
            && let Some(name) = self.shapes[id].declared.clone()
            && self.env.resolve_named(&name).is_some()
        {
            return Ty::Named(name);
        }
        if let Some(ty) = self.memo.get(&id) {
            return ty.clone();
        }
        if self.reify_stack.contains(&id) {
            return Ty::any_table();
        }
        self.reify_stack.push(id);

        let mut fields: BTreeMap<String, ITy> = BTreeMap::new();
        let mut indexers: Vec<(Ty, ITy)> = Vec::new();
        let mut array: Vec<ITy> = Vec::new();
        let mut cur = Some(id);
        let mut seen: HashSet<usize> = HashSet::new();
        while let Some(s) = cur {
            if !seen.insert(s) {
                break;
            }
            for (name, ity) in &self.shapes[s].fields {
                if !name.starts_with("__") && !fields.contains_key(name) {
                    fields.insert(name.clone(), ity.clone());
                }
            }
            for entry in &self.shapes[s].indexers {
                if !indexers.contains(entry) {
                    indexers.push(entry.clone());
                }
            }
            for ity in &self.shapes[s].array {
                if !array.contains(ity) {
                    array.push(ity.clone());
                }
            }
            cur = self.index_delegate(s);
        }

        let mut table = TableTy::default();
        for (name, ity) in fields {
            let ty = self.reify(&ity);
            table.fields.insert(
                name,
                FieldTy {
                    ty,
                    optional: false,
                },
            );
        }
        for (key, ity) in indexers {
            let value = self.reify(&ity);
            table.indexers.push((key, value));
        }
        if !array.is_empty() {
            let elems: Vec<Ty> = array.iter().map(|i| self.reify(i)).collect();
            table.array = Some(Ty::union(elems));
        }
        self.reify_stack.pop();
        let ty = Ty::Table(Box::new(table));
        if self.pass == 1 {
            self.memo.insert(id, ty.clone());
        }
        ty
    }

    /// The shape a lookup delegates to via the metatable's `__index`, when
    /// it is a tracked table.
    pub(super) fn index_delegate(&self, shape: usize) -> Option<usize> {
        let meta = self.shapes[shape].metatable?;
        match self.shapes[meta].fields.get("__index") {
            Some(ITy::Shape(next)) => Some(*next),
            _ => None,
        }
    }
}
