//! Test runner and runtime-matrix orchestration — **Execution** bounded
//! context (SPEC.md §11, §16).
//!
//! Zero-config discovery, busted-compatible shim + native flat API,
//! `--matrix` across 5.1/5.4/luajit, coverage via source-map-aware
//! instrumentation. The only context allowed to spawn runtimes.
//!
//! Status: placeholder — P4.
