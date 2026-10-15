use nimbus_system::wallpaper_engine as we;
use std::time::Instant;

fn main() {
    let t = Instant::now();
    let s = we::status();
    println!("status {:?} in {:?}", s, t.elapsed());
    let t = Instant::now();
    let items = we::list();
    println!("{} wallpapers in {:?}", items.len(), t.elapsed());
    for i in items.iter().take(4) {
        println!("  [{}] {} | {} | {:?}", i.source, i.title, i.kind, i.preview.as_deref().map(|p| &p[p.len().saturating_sub(30)..]));
    }
    if std::env::args().any(|a| a == "--switch") {
        let back = s.current.clone().expect("current");
        let other = items.iter().find(|i| i.id != back && i.kind == "scene").expect("other");
        let t = Instant::now();
        println!("open {} -> {:?} in {:?}", other.title, we::open(&other.id, 0), t.elapsed());
        let t = Instant::now();
        println!("back -> {:?} in {:?}", we::open(&back, 0), t.elapsed());
    }
}
