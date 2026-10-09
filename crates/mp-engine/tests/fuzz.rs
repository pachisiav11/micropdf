//! Mutation fuzzing: the fixtures, with bytes flipped, cut, repeated, spliced from other files
//! and numbers made hostile, go through open, render, text, links, comments, fields,
//! signatures and save. Failing to open is fine; a panic or a crash is not.
//!
//! `MICROPDF_FUZZ=n` tries n inputs instead of 200, and `MICROPDF_FUZZ_SEED` picks another
//! sequence. The input being tried is written to `micropdf-fuzz\input.pdf` in the temp
//! folder first, so a crash that takes the process down leaves it behind, as does an input
//! that takes over 30 seconds, which ends the process; it then belongs in a test.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mp_engine::{Engine, Error, render};

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) % n.max(1) as u64) as usize
    }
}

fn mutate(rng: &mut Rng, data: &mut Vec<u8>, other: &[u8]) {
    for _ in 0..1 + rng.below(4) {
        let at = rng.below(data.len() + 1);
        match rng.below(6) {
            0 => {
                if let Some(b) = data.get_mut(at) {
                    *b ^= 1 << rng.below(8);
                }
            }
            1 => data.truncate(at),
            2 => {
                let end = (at + rng.below(64)).min(data.len());
                data.drain(at..end);
            }
            3 => {
                let end = (at + rng.below(256)).min(data.len());
                let copy = data[at..end].to_vec();
                data.splice(at..at, copy);
            }
            4 => {
                let from = rng.below(other.len());
                let end = (from + rng.below(512)).min(other.len());
                data.splice(at..at, other[from..end].iter().copied());
            }
            _ => {
                if let Some(start) = (at..data.len()).find(|&i| data[i].is_ascii_digit()) {
                    let end = (start..data.len())
                        .find(|&i| !data[i].is_ascii_digit())
                        .unwrap_or(data.len());
                    let n = ["0", "-1", "4294967295", "99999999999999999999"][rng.below(4)];
                    data.splice(start..end, n.bytes());
                }
            }
        }
    }
}

/// Reads all there is to read of the file at `path`, then saves it to `out` and opens that.
fn exercise(engine: &Engine, path: &Path, out: &Path) -> Result<(), Error> {
    let info = engine.open(path)?;
    let doc = info.id;
    let result = (|| -> Result<(), Error> {
        if info.needs_password {
            engine.authenticate(doc, "user")?;
        }
        let pages = engine.page_sizes(doc)?.len();
        let _ = engine.outline(doc);
        let _ = engine.signatures(doc);
        for page in 0..pages.min(3) {
            let _ = render(&*engine.display_list(doc, page)?, 0.5);
            let _ = engine.links(doc, page);
            let _ = engine.annotations(doc, page);
            let _ = engine.fields(doc, page);
        }
        let _ = engine.words(doc);
        engine.save(doc, out, false)?;
        let saved = engine.open(out)?;
        engine.close(saved.id);
        Ok(())
    })();
    engine.close(doc);
    result
}

/// Inputs that crashed or hung once, from `fixtures/crashes`.
#[test]
fn known_crashes_fail_cleanly() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/crashes");
    let out = std::env::temp_dir().join("micropdf-crash-output.pdf");
    let engine = Engine::start();
    for file in std::fs::read_dir(dir).unwrap().flatten() {
        let result = exercise(&engine, &file.path(), &out);
        assert!(
            !matches!(result, Err(Error::Stopped)),
            "{}",
            file.path().display()
        );
    }
}

#[test]
fn damaged_files_fail_cleanly() {
    let env = |name| std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok());
    let runs = env("MICROPDF_FUZZ").unwrap_or(200);
    let mut rng = Rng(env("MICROPDF_FUZZ_SEED").unwrap_or(0x9e37_79b9_7f4a_7c15) | 1);
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let corpus: Vec<Vec<u8>> = std::fs::read_dir(fixtures)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
        .map(|p| std::fs::read(p).unwrap())
        .collect();
    let dir = std::env::temp_dir().join("micropdf-fuzz");
    std::fs::create_dir_all(&dir).unwrap();
    let (input, out) = (dir.join("input.pdf"), dir.join("output.pdf"));
    let engine = Engine::start();
    let tried = Arc::new(AtomicUsize::new(0));
    let watched = Arc::clone(&tried);
    std::thread::spawn(move || {
        loop {
            let before = watched.load(Ordering::SeqCst);
            std::thread::sleep(Duration::from_secs(30));
            if watched.load(Ordering::SeqCst) == before {
                eprintln!("input {before} took over 30 seconds; it is kept as input.pdf");
                std::process::exit(1);
            }
        }
    });
    for i in 0..runs {
        tried.store(i as usize, Ordering::SeqCst);
        let mut data = corpus[rng.below(corpus.len())].clone();
        let other = &corpus[rng.below(corpus.len())];
        mutate(&mut rng, &mut data, other);
        std::fs::write(&input, &data).unwrap();
        if let Err(Error::Stopped) = exercise(&engine, &input, &out) {
            let kept = dir.join(format!("crash-{i}.pdf"));
            std::fs::copy(&input, &kept).unwrap();
            panic!(
                "input {i} stopped the engine; it is kept at {}",
                kept.display()
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
