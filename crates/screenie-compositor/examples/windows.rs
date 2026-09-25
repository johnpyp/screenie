//! Print what the detected compositor IPC reports.
fn main() {
    let c = screenie_compositor::detect();
    println!("compositor: {}", c.name());
    println!("focused output: {:?}", c.focused_output());
    for w in c.windows().unwrap_or_default() {
        println!("  {} [{}] {:?} {} floating={} focused={}", w.id, w.app_id, w.title, w.rect, w.floating, w.focused);
    }
}
