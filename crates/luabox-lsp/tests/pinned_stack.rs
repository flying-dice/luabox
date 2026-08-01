//! The one test that isolates `pin_worker_stacks`, and the only test in this
//! binary.
//!
//! # Why it needs its own binary
//!
//! `rayon::ThreadPoolBuilder::build_global` succeeds once per process. Any
//! earlier test that touches the global pool wins, and the pin's call then
//! returns `Err` and changes nothing — which would make "delete the pin and
//! watch it overflow" fail to fail. Cargo gives each file under `tests/` its
//! own binary, so this file owns pool initialisation by construction.
//!
//! # Why a synthetic recursion, and not a deep Lua file
//!
//! The obvious differential — parse something deep enough to overflow 2 MiB —
//! cannot be written. `luabox-syntax` caps recursion at `MAX_DEPTH` and
//! rejects anything past it before recursion begins, and
//! `parsing_at_the_depth_limit_fits_a_default_stack` proves that parsing at
//! exactly that limit fits 2 MiB in a *debug* build. So no accepted input
//! overflows an unpinned worker while fitting a pinned one, and the pin looked
//! untestable (wave 16 wrote that down as a property of the design; Shockwave
//! round 6 pointed out it is only a property of the parser).
//!
//! A recursion this file writes itself is under no such cap. [`burn`] is the
//! calibration:
//!
//! - each frame holds a `[u8; 8192]` that `black_box` forces into memory, so
//!   a frame costs 8 KiB plus the call's own overhead;
//! - `DEPTH` is 1000, so the recursion needs a little over **8 MiB**;
//! - rayon's unconfigured worker stack is **2 MiB** — 8 MiB overflows it four
//!   times over, at roughly a quarter of the way down;
//! - [`luabox_lsp::PINNED_STACK_BYTES`] is **16 MiB** — 8 MiB leaves close to
//!   half the stack spare, so the passing side is not marginal either.
//!
//! Both margins are deliberate. A calibration that only just overflowed 2 MiB
//! would be a flaky test on a platform with a different guard page, and one
//! that only just fitted 16 MiB would fail for reasons unrelated to the pin.
//!
//! # What it catches
//!
//! The pin is reached through [`luabox_lsp::run`], the production call site,
//! rather than by calling the (private) pin directly — so deleting the call
//! from `run` breaks this test, not just editing the constant. All three
//! failure modes leave the worker on rayon's 2 MiB default and are caught:
//!
//! 1. deleting `pin_worker_stacks()` from `run`;
//! 2. dropping `.stack_size(PINNED_STACK_BYTES)` from the builder;
//! 3. lowering `PINNED_STACK_BYTES` (both crates' copies together, which the
//!    cross-crate equality assertion otherwise permits) to 2 MiB.
//!
//! A stack overflow on a worker aborts the process, so the failure is the test
//! binary dying rather than an assertion — loud, and impossible to miss.
//! Verified by commenting the call out and watching it happen.

use std::sync::mpsc;

use lsp_server::Connection;

/// Bytes of stack each level of [`burn`] is forced to hold.
const FRAME_BYTES: usize = 8192;

/// How deep to recurse: `DEPTH * FRAME_BYTES` is a little over 8 MiB — four
/// times rayon's unpinned 2 MiB default, and half of the pinned 16 MiB.
const DEPTH: u32 = 1000;

/// Recurse `depth` levels, holding [`FRAME_BYTES`] of stack at each.
///
/// `#[inline(never)]` keeps the frames distinct, and `black_box` on the array
/// stops the optimiser from proving the buffer dead and shrinking the frame to
/// nothing. The final read is *after* the recursive call so the buffer has to
/// stay live across it, which is what makes the frame actually cost 8 KiB.
#[inline(never)]
fn burn(depth: u32) -> u64 {
    let mut frame = [1_u8; FRAME_BYTES];
    std::hint::black_box(&mut frame);
    if depth == 0 {
        return u64::from(frame[0]);
    }
    let below = burn(depth - 1);
    below + u64::from(frame[FRAME_BYTES - 1])
}

#[test]
fn a_pinned_worker_survives_a_recursion_no_default_stack_could() {
    // `RUST_MIN_STACK` raises the default stack of every thread rayon spawns,
    // so an environment setting it to 8 MiB or more would let an *unpinned*
    // worker survive `burn`: deletion modes 1 and 2 would both stop failing,
    // silently, and this file would go on passing while proving nothing. The
    // assumption was named but undischarged (Shockwave round 7) and is worth
    // one line. Asserted rather than removed, because a CI runner that sets it
    // deliberately should be told this test is incompatible with it, not
    // quietly overridden.
    assert!(
        std::env::var_os("RUST_MIN_STACK").is_none(),
        "RUST_MIN_STACK is set: it raises every thread's default stack, which \
         would mask an unpinned worker and make this test vacuous"
    );

    // Reach the pin the way production does. The client end is dropped first,
    // so `initialize_start` fails and `run` returns immediately — but
    // `pin_worker_stacks()` is the line before it, and by then the global pool
    // is configured. Reaching it through `run` is the point: a test that
    // called the pin directly would still pass with the production call site
    // deleted.
    let (server, client) = Connection::memory();
    drop(client);
    let refused = luabox_lsp::run(server);
    assert!(
        refused.is_err(),
        "a client that hung up cannot complete the handshake"
    );

    // `rayon::spawn` always runs on a pool worker — unlike `rayon::join`,
    // which may run its first closure on the calling thread and would measure
    // the test harness's stack instead of the one under test.
    let (tx, rx) = mpsc::channel();
    rayon::spawn(move || {
        let _ = tx.send(burn(DEPTH));
    });
    let reached = rx
        .recv()
        .expect("the worker completed the recursion (a worker that overflowed would have aborted the process)");

    // `burn(0)` is 1 and each level above adds 1.
    assert_eq!(reached, u64::from(DEPTH) + 1);
}
