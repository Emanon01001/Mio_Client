//! Test target for the client DLL outside the game.
//!
//! Prints how fast the hookable clock (QueryPerformanceCounter, via `Instant`)
//! runs relative to the wall clock (`SystemTime`, not hooked). Inject with:
//!   mcbe_injector inject --process timer_host.exe

use std::time::{Duration, Instant, SystemTime};

fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    println!(
        "timer_host pid {} running for {seconds}s",
        std::process::id()
    );

    for _ in 0..seconds {
        let qpc = Instant::now();
        let wall = SystemTime::now();
        std::thread::sleep(Duration::from_secs(1));
        let qpc = qpc.elapsed().as_secs_f64();
        let wall = wall.elapsed().map(|d| d.as_secs_f64()).unwrap_or(0.0);
        if wall > 0.0 {
            println!("qpc/wall ratio: {:.2}", qpc / wall);
        }
    }
}
