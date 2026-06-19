// At most one argument (the `Deluge` handle) is allowed.
fn main() {}

#[deluge_macros::app]
async fn app(a: u8, b: u8) {}
