//! Stress test for the threading model: many callers hitting the engine while a pool renders
//! shared display lists. Every render must match a single-threaded reference exactly.

use std::path::PathBuf;
use std::thread;

use mp_engine::{Engine, RenderPool};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

#[test]
fn concurrent_engine_calls_and_renders_are_consistent() {
    const CALLERS: usize = 6;
    const RENDERS_PER_CALLER: usize = 40;

    let engine = Engine::start();
    let pool = RenderPool::new(8);
    let doc = engine.open(fixture("hello.pdf")).unwrap();
    let reference = pool
        .render(engine.display_list(doc.id, 0).unwrap(), 1.5)
        .recv()
        .unwrap()
        .unwrap();

    thread::scope(|s| {
        for _ in 0..CALLERS {
            s.spawn(|| {
                // Each caller also opens and closes its own copy, so the engine thread
                // interleaves document lifetimes with display-list requests.
                let own = engine.open(fixture("hello.pdf")).unwrap();
                let pending: Vec<_> = (0..RENDERS_PER_CALLER)
                    .map(|i| {
                        let id = if i % 2 == 0 { doc.id } else { own.id };
                        pool.render(engine.display_list(id, 0).unwrap(), 1.5)
                    })
                    .collect();
                engine.close(own.id);
                for rx in pending {
                    assert_eq!(rx.recv().unwrap().unwrap(), reference);
                }
            });
        }
    });
}
