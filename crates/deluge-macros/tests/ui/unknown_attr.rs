// Only `setup = <path>` is a valid attribute argument.
fn main() {}

#[deluge_macros::app(frobnicate = bar)]
async fn app() {}
