//! The card's memory this process may still take, as the aligner reads it.
fn main() {
    match nikgob_align::engine::card_free() {
        Some(free) => println!("{} MB free", free >> 20),
        None => println!("not known"),
    }
}
