//! Read-only tour of the crate on this machine: prints what every getter sees.
//! Nothing here changes a setting.

use nimbus_system::*;
use std::time::{Duration, Instant};

fn main() {
    let t = Instant::now();
    println!("volume      {:?}  ({:?})", volume::get(), t.elapsed());
    let t = Instant::now();
    println!("brightness  {:?}  ({:?})", brightness::get(), t.elapsed());
    let t = Instant::now();
    let np = media::now_playing(true);
    println!("media       {:?}  ({:?})", np.as_ref().map(|o| o.as_ref().map(|n| (n.app.clone(), n.title.clone(), n.artist.clone(), n.playing, n.position_ms, n.duration_ms, n.artwork.as_ref().map(|a| a.len())))), t.elapsed());
    println!("wallpaper   {:?}", wallpaper::get());
    println!("battery     {:?}", power::battery());
    let t = Instant::now();
    println!("radios      {:?}  ({:?})", radio::get(), t.elapsed());
    println!("dark mode   {:?}", theme::dark());
    println!("clipboard   seq {:?}, secret {}, copied from {:?}", clipboard::sequence(), clipboard::is_secret(), clipboard::owner_app());
    meter::peak();
    std::thread::sleep(Duration::from_millis(400));
    println!("audio peak  {:.3}", meter::peak());

    let t = Instant::now();
    let apps = apps::list(64, |_| true);
    let icons = apps.iter().filter(|(_, i)| i.is_some()).count();
    println!("apps        {} found, {} icons  ({:?})", apps.len(), icons, t.elapsed());
    for (a, i) in apps.iter().step_by((apps.len() / 6).max(1)).take(6) {
        println!("   {:<32} {:?}  {}", a.name, i.as_ref().map(|i| (i.width, i.height)), a.id);
    }

    let mut m = stats::Monitor::new();
    std::thread::sleep(Duration::from_millis(1000));
    let t = Instant::now();
    let s = m.sample(5);
    println!("sample took {:?}", t.elapsed());
    println!("cpu {:.1}% ({} cores, {}, {:?} MHz, {} processes), mem {:.1}/{:.1} GB, disk {:.0}/{:.0} GB, net rx {} B/s tx {} B/s",
        s.cpu_percent, s.cpu_per_core.len(), s.cpu_name, s.cpu_mhz, s.processes, s.mem_used as f64/1e9, s.mem_total as f64/1e9,
        s.disk_used as f64/1e9, s.disk_total as f64/1e9, s.net_rx_per_s, s.net_tx_per_s);
    println!("gpu {:?} {:?}% mem {:?} GB, up {} s", s.gpu_name, s.gpu_percent, s.gpu_mem_used.map(|b| b as f64/1e9), s.uptime_s);
    println!("top by cpu:");
    for p in &s.top_cpu { println!("   {:<32} x{:<3} {:5.1}% {:.0} MB", p.name, p.count, p.cpu_percent, p.mem as f64/1e6); }
    println!("top by memory:");
    for p in &s.top_mem { println!("   {:<32} x{:<3} {:5.1}% {:.0} MB", p.name, p.count, p.cpu_percent, p.mem as f64/1e6); }
}
