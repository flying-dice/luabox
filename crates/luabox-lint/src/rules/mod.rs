//! The rule registry (SPEC.md §9).

mod concat_in_loop;
mod empty_then;
mod global_write;
mod nil_compare;
mod pairs_on_array;
mod shadowed_local;
mod unused_local;
mod unused_param;

use crate::rule::Rule;

/// Every built-in lint rule, in a stable order.
#[must_use]
pub fn rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(unused_local::UnusedLocal),
        Box::new(unused_param::UnusedParam),
        Box::new(shadowed_local::ShadowedLocal),
        Box::new(global_write::GlobalWrite),
        Box::new(nil_compare::NilCompare),
        Box::new(concat_in_loop::ConcatInLoop),
        Box::new(pairs_on_array::PairsOnArray),
        Box::new(empty_then::EmptyThen),
    ]
}
