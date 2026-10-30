//! Read-only diagnostic of the same native observer used by the desktop host.
#[path = "../src/tomatodo_observer.rs"]
mod tomatodo_observer;

fn main() {
    let mut observer = tomatodo_observer::NativeObserver::default();
    match observer.snapshot() {
        Ok(sample) => println!("{}", serde_json::to_string(&sample).unwrap()),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}
