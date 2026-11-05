// The entry point returns nothing.
fn main() {}

#[deluge_macros::app]
async fn app() -> u32 {
    0
}
