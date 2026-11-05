// The entry point cannot be generic.
fn main() {}

#[deluge_macros::app]
async fn app<T>() {}
