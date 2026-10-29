//! Import a folder into a throwaway catalog and report what the engine sees,
//! with timings. Useful for checking a real shoot or the 10k-photo targets.
//!
//!     cargo run -p keepers-core --release --example inspect -- <folder> [--list] [--cull]

use keepers_core::{Event, Library};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let folder = std::path::PathBuf::from(args.next().expect("usage: inspect <folder> [--list] [--cull]"));
    let rest: Vec<String> = args.collect();
    let list = rest.iter().any(|a| a == "--list");
    let cull = rest.iter().any(|a| a == "--cull");
    let data = std::env::temp_dir().join(format!("keepers-inspect-{}", std::process::id()));
    let done = Arc::new(AtomicUsize::new(0));
    let total = Arc::new(AtomicUsize::new(usize::MAX));
    let (d, t) = (done.clone(), total.clone());
    let lib = Library::open(
        &data,
        Box::new(move |e| {
            if let Event::Progress { done, total } = e {
                d.store(done, Ordering::SeqCst);
                t.store(total, Ordering::SeqCst);
            }
        }),
    )?;

    let t0 = Instant::now();
    lib.import(folder.clone(), None);
    // Wait until something is catalogued.
    while lib.photos(None)?.is_empty() && t0.elapsed() < Duration::from_secs(120) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let first_grid = t0.elapsed();
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let (d, t) = (done.load(Ordering::SeqCst), total.load(Ordering::SeqCst));
        if t != usize::MAX && d >= t && lib.photos(None)?.iter().all(|p| p.processed) {
            break;
        }
    }
    let processed = t0.elapsed();
    lib.regroup()?;
    let photos = lib.photos(None)?;

    let errors: Vec<_> = photos.iter().filter(|p| p.error.is_some()).collect();
    let bursts: std::collections::BTreeMap<i64, Vec<_>> = photos.iter().filter_map(|p| p.burst_id.map(|b| (b, p))).fold(
        Default::default(),
        |mut m: std::collections::BTreeMap<i64, Vec<_>>, (b, p)| {
            m.entry(b).or_default().push(p);
            m
        },
    );
    println!("Folder:            {}", folder.display());
    println!("Catalog items:     {} ({} RAW+JPEG pairs)", photos.len(), photos.iter().filter(|p| p.pair_path.is_some()).count());
    println!("First grid after:  {:.2?}", first_grid);
    println!(
        "Previews+scores:   {:.2?} total, {:.1} ms/photo",
        processed,
        processed.as_secs_f64() * 1000.0 / photos.len().max(1) as f64
    );
    println!("Bursts:            {}", bursts.len());
    for (b, members) in &bursts {
        let names: Vec<String> = members
            .iter()
            .map(|p| format!("{}{} ({:.0})", p.file_name, if p.burst_best { "★" } else { "" }, p.sharpness.unwrap_or(0.0)))
            .collect();
        println!("  burst {b}: {}", names.join(", "));
    }
    println!("Near-duplicates:   {}", photos.iter().filter(|p| p.dup_of.is_some()).count());
    for p in photos.iter().filter(|p| p.dup_of.is_some()) {
        println!("  {} ≈ id {}", p.file_name, p.dup_of.unwrap());
    }
    println!("Unreadable:        {}", errors.len());
    for p in &errors {
        println!("  {}: {}", p.file_name, p.error.as_deref().unwrap_or(""));
    }
    if list {
        for p in &photos {
            println!(
                "{:>6} {:<28} {:<5} {:>5}x{:<5} o{} sharp {:>3.0} exp {:>3.0}% {}",
                p.id,
                p.file_name,
                p.kind.as_str(),
                p.width.unwrap_or(0),
                p.height.unwrap_or(0),
                p.orientation,
                p.sharpness.unwrap_or(0.0),
                p.exposure.unwrap_or(0.0) * 100.0,
                p.camera.as_deref().unwrap_or("-")
            );
        }
    }
    if cull {
        use keepers_core::cull::{CullOptions, Strictness, Verdict};
        let names: std::collections::HashMap<i64, &str> = photos.iter().map(|p| (p.id, p.file_name.as_str())).collect();
        for strictness in [Strictness::Gentle, Strictness::Balanced, Strictness::Strict] {
            let plan = lib.plan_cull(None, &CullOptions { strictness, ..Default::default() })?;
            let n = |v| plan.suggestions.iter().filter(|s| s.verdict == v).count();
            println!(
                "Auto-cull {strictness:?}: {} moments, {} picks, {} rejects, {} left",
                plan.moments,
                n(Verdict::Pick),
                n(Verdict::Reject),
                n(Verdict::Leave)
            );
            if list && strictness == Strictness::Balanced {
                let mut s = plan.suggestions.clone();
                s.sort_by_key(|s| (s.moment, s.id));
                for s in s {
                    println!("  m{:<3} {:<28} {:?} ({:?})", s.moment, names[&s.id], s.verdict, s.reason);
                }
            }
        }
    }
    drop(lib);
    let _ = std::fs::remove_dir_all(&data);
    Ok(())
}
