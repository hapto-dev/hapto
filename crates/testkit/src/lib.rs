//! Test tools shared by the Hapto crates.
//!
//! A dev-dependency only: nothing a plugin ships links it. Each module is one
//! rule of `docs/testing.md`, or one check that several crates share:
//!
//! - [`alloc`]: the zero-allocation gate for the code the audio thread runs
//!   in a [block].
//! - [`sync`]: real-thread tests wait on a latch or a condition, never on a
//!   fixed sleep.
//! - [`contract`]: a contract assertion in library code is a test failure,
//!   and a test that provokes one on purpose declares it, so a vanished
//!   assertion fails too.
//! - [`drift`]: a generated file in the repository matches what the code
//!   generates now.
//! - [`systems`]: a build script's list of systems and its manifest's target
//!   tables name the same systems.
//!
//! [block]: https://github.com/hapto-dev/hapto/blob/main/docs/glossary.md#block
//!
//! # Examples
//!
//! A test binary installs the allocation gate once, then proves its audio
//! code allocates nothing after a worker thread meets it at a latch:
//!
//! ```
//! use hapto_testkit::alloc::assert_no_allocations;
//! use hapto_testkit::sync::Latch;
//!
//! hapto_testkit::install_alloc_gate!();
//!
//! fn main() {
//!     let started = Latch::new(1);
//!     std::thread::scope(|scope| {
//!         scope.spawn(|| started.count_down());
//!         started.wait(); // no fixed sleep
//!     });
//!
//!     let mut block = [0.5_f32; 64];
//!     assert_no_allocations("the gain stage", || {
//!         for sample in &mut block {
//!             *sample *= 0.5;
//!         }
//!     });
//! }
//! ```

pub mod alloc;
pub mod contract;
pub mod drift;
pub mod sync;
pub mod systems;
