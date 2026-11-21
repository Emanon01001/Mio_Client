//! cargo run --release -p client --example bench_binds --offline --target-dir target/next
//! Uses only fixture TOML and fake key states; does not attach to Minecraft.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Instant,
};
#[allow(dead_code)]
#[path = "../src/input.rs"]
mod input;
mod bench_logging {
    #[macro_export]
    macro_rules! log {
        ($($a:tt)*) => {{ let _ = format_args!($($a)*); }};
    }
}
// Original poll algorithm retained here so normal builds do not depend on an
// ignored baseline directory. Fixture construction is outside the measurement.
mod before {
    use crate::input;
    pub struct Binds {
        list: Vec<(String, String)>,
        down: Vec<u16>,
    }
    impl Binds {
        pub fn new(count: usize) -> Self {
            Self {
                list: (0..count)
                    .map(|i| (format!("F{}", i % 24 + 1), "toggle esp".into()))
                    .collect(),
                down: Vec::new(),
            }
        }
        pub fn poll(&mut self, key_down: impl Fn(u16) -> bool, allowed: bool) -> Vec<String> {
            let mut fired = Vec::new();
            let mut now_down = Vec::new();
            for (key, command) in &self.list {
                let Some(vk) = input::parse_key(key).filter(|&vk| vk != 0) else {
                    continue;
                };
                if key_down(vk) {
                    if allowed && !self.down.contains(&vk) {
                        fired.push(command.clone());
                    }
                    now_down.push(vk);
                }
            }
            self.down = now_down;
            fired
        }
    }
}
#[allow(dead_code)]
#[path = "../src/binds.rs"]
mod after;
struct Allocator;
static TRACK: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(l.size(), Ordering::Relaxed);
        }
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(n, Ordering::Relaxed);
        }
        System.realloc(p, l, n)
    }
}
#[global_allocator]
static ALLOC: Allocator = Allocator;
fn measured(mut f: impl FnMut()) -> (f64, usize, usize) {
    let at = Instant::now();
    for _ in 0..20_000 {
        f();
    }
    let ms = at.elapsed().as_secs_f64() * 1000.;
    COUNT.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    TRACK.store(true, Ordering::Relaxed);
    for _ in 0..20_000 {
        f();
    }
    TRACK.store(false, Ordering::Relaxed);
    (
        ms,
        COUNT.load(Ordering::Relaxed),
        BYTES.load(Ordering::Relaxed),
    )
}
fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../.analysis-cache/optimization-20261116/binds_fixture");
    std::fs::create_dir_all(&dir).unwrap();
    println!("binds,held,version,round,ms,allocations,requested_bytes");
    for count in [8, 48] {
        let text = (0..count)
            .map(|i| {
                format!(
                    "[[bind]]\nkey = 'F{}'\ncommand = 'toggle esp'\n",
                    i % 24 + 1
                )
            })
            .collect::<String>();
        std::fs::write(dir.join("mcbe_client_binds.toml"), text).unwrap();
        for held in [false, true] {
            for round in 0..5 {
                let mut old = before::Binds::new(count);
                let mut new = after::Binds::load(&dir);
                let down = |k| black_box(held) && black_box(k) == 0x70;
                old.poll(down, true);
                new.poll(down, true);
                let (t, a, b) = measured(|| {
                    black_box(old.poll(down, true));
                });
                println!("{count},{held},before,{round},{t:.3},{a},{b}");
                let (t, a, b) = measured(|| {
                    black_box(new.poll(down, true));
                });
                println!("{count},{held},after,{round},{t:.3},{a},{b}");
            }
        }
    }
}
